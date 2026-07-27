using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Security.Cryptography;
using System.Threading.Channels;
using Keyboop.Core.Input;
using Keyboop.Core.Layout;
using Keyboop.Protocol;

namespace Keyboop.InputHost;

[SupportedOSPlatform("windows10.0.17763")]
internal sealed class WindowsInputHost : IDisposable
{
    private readonly KeystrokeBuffer _buffer = new();
    private readonly KeyboardTextResolver _textResolver = new();
    private readonly LayoutDetector _detector;
    private readonly NativeMethods.LowLevelKeyboardProc _hookCallback;
    private readonly Channel<PendingReplacement> _replacementQueue;
    private readonly Channel<HostControlCommand> _controlQueue;
    private readonly CancellationTokenSource _shutdown = new();
    private readonly InputInjector _injector;
    private readonly ClipboardTextInserter _dictationInserter;
    private readonly InputHostSpeechClient _speechClient;
    private readonly SpeechHotkeySession _speechSession;
    private readonly nuint _syntheticMarker;
    private readonly nuint _replayMarker;
    private readonly Task _replacementWorker;
    private readonly System.Threading.Timer _speechHotkeyWatchdog;
    private readonly object _speechHotkeyGate = new();
    private readonly RecoveryHotkeyState _recoveryHotkey = new();
    private InputHostControlServer? _controlServer;
    private AppSettings _settings;
    private PendingReplacement? _activeReplacement;
    private nint _hook;
    private uint _hookThreadId;
    private ulong _sequence;
    private bool _suppressSpeechWinKeyUp;
    private DateTimeOffset? _speechEndedAt;
    private bool _pauseHotkeyDown;
    private TargetSnapshot _recoveryTarget;
    private bool _paused;
    private readonly bool _diagnostics =
        string.Equals(
            Environment.GetEnvironmentVariable("KEYBOOP_DIAGNOSTICS"),
            "1",
            StringComparison.Ordinal);
    private bool _disposed;

    internal WindowsInputHost(AppSettings settings)
    {
        _settings = settings;
        _syntheticMarker = CreateSyntheticMarker();
        do
        {
            _replayMarker = CreateSyntheticMarker();
        }
        while (_replayMarker == _syntheticMarker);
        _injector = new InputInjector(_syntheticMarker, _replayMarker);
        _dictationInserter = new ClipboardTextInserter(_injector);
        _detector = new LayoutDetector(LanguageData.LoadEmbedded());
        _hookCallback = HookCallback;
        _replacementQueue = Channel.CreateUnbounded<PendingReplacement>(
            new UnboundedChannelOptions
            {
                SingleReader = true,
                SingleWriter = true,
                AllowSynchronousContinuations = false
            });
        _replacementWorker = Task.Factory.StartNew(
            ReplacementWorkerAsync,
            _shutdown.Token,
            TaskCreationOptions.LongRunning,
            TaskScheduler.Default).Unwrap();
        _controlQueue = Channel.CreateUnbounded<HostControlCommand>(
            new UnboundedChannelOptions
            {
                SingleReader = true,
                SingleWriter = false,
                AllowSynchronousContinuations = false
            });
        _speechClient = new InputHostSpeechClient(
            QueueTranscriptInsertion,
            QueueTranscriptRecovery);
        _speechSession = new SpeechHotkeySession(_speechClient);
        _speechHotkeyWatchdog = new System.Threading.Timer(
            CheckSpeechHotkeyState,
            null,
            TimeSpan.FromMilliseconds(100),
            TimeSpan.FromMilliseconds(100));
    }

    internal int Run()
    {
        _hookThreadId = NativeMethods.GetCurrentThreadId();
        Console.CancelKeyPress += OnCancelKeyPress;
        _hook = NativeMethods.SetWindowsHookEx(
            NativeMethods.WhKeyboardLl,
            _hookCallback,
            NativeMethods.GetModuleHandle(null),
            0);
        if (_hook == 0)
        {
            throw new Win32Exception(Marshal.GetLastPInvokeError(), "Could not install keyboard hook.");
        }

        _controlServer = new InputHostControlServer(
            _controlQueue.Writer,
            WakeForControlCommand);
        try
        {
            while (true)
            {
                var result = NativeMethods.GetMessage(out var message, 0, 0, 0);
                if (result == 0)
                {
                    return 0;
                }

                if (result < 0)
                {
                    throw new Win32Exception(
                        Marshal.GetLastPInvokeError(),
                        "Keyboard hook message loop failed.");
                }

                if (message.Value == NativeMethods.WmAppHostCommand)
                {
                    DrainControlCommands();
                    continue;
                }

                _ = NativeMethods.TranslateMessage(in message);
                _ = NativeMethods.DispatchMessage(in message);
            }
        }
        finally
        {
            Console.CancelKeyPress -= OnCancelKeyPress;
            RemoveHook();
        }
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        RemoveHook();
        _replacementQueue.Writer.TryComplete();
        _controlQueue.Writer.TryComplete();
        _shutdown.Cancel();
        _speechHotkeyWatchdog.Dispose();
        EndSpeechDictation();
        _speechClient.DisposeAsync().AsTask().GetAwaiter().GetResult();
        _dictationInserter.DisposeAsync().AsTask().GetAwaiter().GetResult();
        if (_controlServer is not null)
        {
            _controlServer.DisposeAsync().AsTask().GetAwaiter().GetResult();
            _controlServer = null;
        }
        try
        {
            _replacementWorker.Wait(TimeSpan.FromSeconds(2));
        }
        catch (AggregateException)
        {
            // Ordinary input is already unhooked.
        }

        _shutdown.Dispose();
    }

    private nint HookCallback(int code, nuint message, nint data)
    {
        if (code < NativeMethods.HcAction)
        {
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        var input = Marshal.PtrToStructure<NativeMethods.KbdLlHookStruct>(data);
        if (input.ExtraInfo == _syntheticMarker)
        {
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }
        var replayed = input.ExtraInfo == _replayMarker;

        var keyUp = message is NativeMethods.WmKeyUp or NativeMethods.WmSysKeyUp;
        var keyDown = message is NativeMethods.WmKeyDown or NativeMethods.WmSysKeyDown;
        if (!keyDown && !keyUp)
        {
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        var physical = ToPhysicalEvent(input, keyUp);
        if (!replayed)
        {
            while (true)
            {
                var active = Volatile.Read(ref _activeReplacement);
                if (active is null)
                {
                    break;
                }

                if (keyUp && input.VirtualKey == active.Plan.Boundary.VirtualKey)
                {
                    return 1;
                }

                if (active.TryQueue(physical))
                {
                    return 1;
                }

                // The worker marked the transaction complete just before clearing
                // the published reference. Clear only that stale instance and then
                // process this key normally (or queue it into a newer transaction).
                _ = Interlocked.CompareExchange(
                    ref _activeReplacement,
                    null,
                    active);
            }
        }

        var target = TargetTracker.Capture();
        _textResolver.Track(input, keyUp);
        if (TryHandleGlobalHotkey(input.VirtualKey, keyUp))
        {
            return 1;
        }

        if (keyUp)
        {
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        if (_paused)
        {
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        if (!_settings.AutoCorrectionEnabled)
        {
            // Manual-first privacy invariant: with automatic correction disabled,
            // ordinary typed text is neither resolved nor appended to the buffer.
            // Keep modifier state intact so the independent speech/pause hotkeys
            // can still recognize multi-key chords.
            _buffer.Clear();
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        var characters = _textResolver.ResolveText(
            input,
            (nint)target.KeyboardLayout);
        if (TryHandleNavigationOrShortcut(input.VirtualKey))
        {
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        if (TryBoundary(input.VirtualKey, out var whitespace))
        {
            var settings = _settings;
            var boundaryEnabled = input.VirtualKey switch
            {
                NativeMethods.VkSpace => settings.CorrectOnSpace,
                NativeMethods.VkTab => settings.CorrectOnTab,
                NativeMethods.VkReturn => settings.CorrectOnEnter,
                _ => false
            };
            var word = _buffer.CurrentWord;
            var previous = _buffer.ContextWord(forCurrent: true);
            if (settings.AutoCorrectionEnabled && boundaryEnabled && word.Length > 0)
            {
                var decision = _detector.Decide(word, previousWord: previous);
                if (_diagnostics)
                {
                    Console.WriteLine(
                        $"Boundary observed: vk={input.VirtualKey}, hkl=0x{target.KeyboardLayout:x}, "
                        + $"length={word.Length}, "
                        + $"convert={decision.ShouldConvert}, reason={decision.Reason}.");
                }

                if (decision.ShouldConvert)
                {
                    var converted = _detector.ConvertForDecision(word, decision);
                    var pending = new PendingReplacement(
                        new ReplacementPlan(
                            _buffer.WordForConversion()?.DeleteCount ?? word.Length,
                            converted,
                            physical),
                        target,
                        converted);
                    Volatile.Write(ref _activeReplacement, pending);
                    if (_replacementQueue.Writer.TryWrite(pending))
                    {
                        _buffer.Boundary(whitespace);
                        _buffer.ApplyConversion(converted);
                        return 1;
                    }

                    _ = Interlocked.CompareExchange(
                        ref _activeReplacement,
                        null,
                        pending);
                }
            }

            _buffer.Boundary(whitespace);
            return NativeMethods.CallNextHookEx(_hook, code, message, data);
        }

        if (characters.Length > 0
            && characters.All(static character =>
                !char.IsControl(character)
                && character != '\u007f'))
        {
            _buffer.Append(characters);
        }

        return NativeMethods.CallNextHookEx(_hook, code, message, data);
    }

    private async Task ReplacementWorkerAsync()
    {
        try
        {
            await foreach (var pending in _replacementQueue.Reader.ReadAllAsync(_shutdown.Token))
            {
                ProcessReplacement(pending);
            }
        }
        catch (OperationCanceledException) when (_shutdown.IsCancellationRequested)
        {
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(
                $"Replacement worker failed: {exception.GetType().Name}: {exception.Message}");
        }
    }

    private void ProcessReplacement(PendingReplacement pending)
    {
        var committed = false;
        try
        {
            if (TargetTracker.Capture() == pending.Target && !pending.IsExpired())
            {
                _injector.Replace(
                    pending.Plan.DeleteCount,
                    pending.Converted,
                    pending.Plan.Boundary);
                committed = true;
            }

            if (!committed)
            {
                _injector.ReplayBoundaryAndEvents(
                    pending.Plan.Boundary,
                    pending.Drain());
                QueueContextReset();
                _ = Interlocked.CompareExchange(
                    ref _activeReplacement,
                    null,
                    pending);
                return;
            }

            while (true)
            {
                var queued = pending.Drain();
                if (queued.Count > 0)
                {
                    // Replayed physical keys use their own marker: the hook processes
                    // them into the word buffer and may start the next correction, but
                    // never mistakes them for brand-new input overtaking this transaction.
                    _injector.Replay(queued);
                    continue;
                }

                if (pending.TryFinishWhenEmpty())
                {
                    _ = Interlocked.CompareExchange(
                        ref _activeReplacement,
                        null,
                        pending);
                    return;
                }
            }
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(
                $"Replacement transaction failed: {exception.GetType().Name}: {exception.Message}");
            try
            {
                _injector.ReplayBoundaryAndEvents(
                    pending.Plan.Boundary,
                    pending.Drain());
            }
            catch (Exception replayException)
            {
                Console.Error.WriteLine(
                    $"Input replay failed: {replayException.GetType().Name}: {replayException.Message}");
            }
            finally
            {
                QueueContextReset();
                _ = Interlocked.CompareExchange(
                    ref _activeReplacement,
                    null,
                    pending);
            }
        }
    }

    private bool TryHandleNavigationOrShortcut(uint virtualKey)
    {
        if (KeyboardTextResolver.IsModifierKey(virtualKey))
        {
            return true;
        }

        if (virtualKey == NativeMethods.VkBack)
        {
            if (_textResolver.HasShortcutModifier)
            {
                ResetContext();
            }
            else
            {
                _buffer.Backspace();
            }

            return true;
        }

        if (_textResolver.HasShortcutModifier)
        {
            ResetContext();
            return true;
        }

        if (virtualKey is NativeMethods.VkLeft
            or NativeMethods.VkRight
            or NativeMethods.VkUp
            or NativeMethods.VkDown
            or NativeMethods.VkEscape
            or NativeMethods.VkDelete
            or NativeMethods.VkHome
            or NativeMethods.VkEnd
            or NativeMethods.VkPrior
            or NativeMethods.VkNext)
        {
            ResetContext();
            return true;
        }

        return false;
    }

    private bool TryHandleGlobalHotkey(
        uint virtualKey,
        bool keyUp)
    {
        var speechWindowsKey = GlobalHotkeyRules.IsWindowsKey(virtualKey);
        var speechControlKey = GlobalHotkeyRules.IsControlKey(virtualKey);
        var speechChordKey = speechWindowsKey || speechControlKey;
        var physicalModifiers = new PhysicalModifierState(
            _textResolver.IsControlDown,
            _textResolver.IsShiftDown,
            _textResolver.IsAltDown,
            _textResolver.IsWindowsDown);
        var recoveryDecision = _recoveryHotkey.Handle(
            virtualKey,
            keyUp,
            physicalModifiers);
        if (_diagnostics
            && (virtualKey == NativeMethods.VkX
                || virtualKey is NativeMethods.VkShift
                    or NativeMethods.VkLshift
                    or NativeMethods.VkRshift
                    or NativeMethods.VkMenu
                    or NativeMethods.VkLmenu
                    or NativeMethods.VkRmenu))
        {
            Console.WriteLine(
                $"Recovery input: vk={virtualKey}, up={keyUp}, "
                + $"ctrl={physicalModifiers.ControlDown}, "
                + $"shift={physicalModifiers.ShiftDown}, "
                + $"alt={physicalModifiers.AltDown}, "
                + $"win={physicalModifiers.WindowsDown}, "
                + $"capture={recoveryDecision.CaptureTarget}, "
                + $"trigger={recoveryDecision.Trigger}, "
                + $"suppress={recoveryDecision.Suppress}.");
        }

        if (recoveryDecision.CaptureTarget)
        {
            _recoveryTarget = TargetTracker.CaptureTopLevel();
            _buffer.Clear();
        }

        if (recoveryDecision.Trigger)
        {
            if (_speechSession.TryRecoverLast(_recoveryTarget))
            {
                lock (_speechHotkeyGate)
                {
                    _speechEndedAt = DateTimeOffset.UtcNow;
                }
            }
        }

        if (recoveryDecision.Suppress)
        {
            return true;
        }

        if (!keyUp
            && speechChordKey
            && GlobalHotkeyRules.CanBeginDictation(physicalModifiers))
        {
            lock (_speechHotkeyGate)
            {
                // Suppress the matching Windows key-up only when its key-down was
                // also suppressed. If Win was pressed first, it has already
                // reached Windows and its key-up must pass through as well.
                _suppressSpeechWinKeyUp = speechWindowsKey;
            }

            BeginSpeechDictation(TargetTracker.CaptureTopLevel());
            _buffer.Clear();
            return speechWindowsKey;
        }

        if (speechWindowsKey && keyUp)
        {
            bool suppress;
            lock (_speechHotkeyGate)
            {
                suppress = _suppressSpeechWinKeyUp;
                _suppressSpeechWinKeyUp = false;
                _speechEndedAt = null;
            }

            if (IsSpeechDictationActive())
            {
                EndSpeechDictation();
            }

            if (suppress)
            {
                return true;
            }
        }

        if (keyUp
            && speechControlKey
            && IsSpeechDictationActive())
        {
            EndSpeechDictation();
            return false;
        }

        if (virtualKey == NativeMethods.VkP
            && (_pauseHotkeyDown
                || (!keyUp
                    && GlobalHotkeyRules.CanTogglePause(physicalModifiers))))
        {
            if (!keyUp && !_pauseHotkeyDown)
            {
                _pauseHotkeyDown = true;
                _paused = !_paused;
                ResetContext();
                Console.WriteLine(_paused ? "Input processing paused." : "Input processing resumed.");
            }
            else if (keyUp)
            {
                _pauseHotkeyDown = false;
            }

            return true;
        }

        return false;
    }

    private void BeginSpeechDictation(TargetSnapshot target)
    {
        if (_speechSession.TryBegin(target))
        {
            lock (_speechHotkeyGate)
            {
                _speechEndedAt = null;
            }
        }
    }

    private void EndSpeechDictation()
    {
        if (!_speechSession.TryEnd())
        {
            return;
        }

        lock (_speechHotkeyGate)
        {
            _speechEndedAt = DateTimeOffset.UtcNow;
        }
    }

    private bool IsSpeechDictationActive() => _speechSession.IsActive;

    private void CheckSpeechHotkeyState(object? state)
    {
        if (!IsSpeechDictationActive())
        {
            lock (_speechHotkeyGate)
            {
                if (_suppressSpeechWinKeyUp
                    && _speechEndedAt is { } endedAt
                    && DateTimeOffset.UtcNow - endedAt > TimeSpan.FromSeconds(1))
                {
                    _suppressSpeechWinKeyUp = false;
                    _speechEndedAt = null;
                }
            }

            return;
        }

        if (PhysicalModifierState.IsPhysicalControlDown())
        {
            return;
        }

        EndSpeechDictation();
    }

    private void QueueTranscriptInsertion(
        TranscriptMessage transcript,
        TargetSnapshot target)
    {
        _ = _dictationInserter.TryQueue(
            transcript.Text,
            addTrailingSpace: true,
            target);
    }

    private void QueueTranscriptRecovery(
        TranscriptMessage transcript,
        TargetSnapshot _)
    {
        _dictationInserter.TryQueueRecovery(transcript.Text);
    }

    private void QueueContextReset()
    {
        var command = new ResetContextCommand();
        if (_controlQueue.Writer.TryWrite(command))
        {
            WakeForControlCommand();
        }
    }

    private PhysicalKeyEvent ToPhysicalEvent(
        NativeMethods.KbdLlHookStruct input,
        bool keyUp) =>
        new(
            input.VirtualKey,
            input.ScanCode,
            keyUp,
            input.Flags,
            ++_sequence,
            input.Time);

    private static bool TryBoundary(uint virtualKey, out string whitespace)
    {
        whitespace = virtualKey switch
        {
            NativeMethods.VkSpace => " ",
            NativeMethods.VkTab => "\t",
            NativeMethods.VkReturn => "\n",
            _ => string.Empty
        };
        return whitespace.Length > 0;
    }

    private void ResetContext()
    {
        _buffer.Clear();
        _textResolver.Reset();
    }

    private void WakeForControlCommand()
    {
        var threadId = Volatile.Read(ref _hookThreadId);
        if (threadId != 0)
        {
            _ = NativeMethods.PostThreadMessage(
                threadId,
                NativeMethods.WmAppHostCommand,
                0,
                0);
        }
    }

    private void DrainControlCommands()
    {
        while (_controlQueue.Reader.TryRead(out var command))
        {
            try
            {
                switch (command)
                {
                    case UpdateSettingsCommand update:
                        _settings = update.Settings;
                        command.Completion.TrySetResult(
                            new AcknowledgementMessage(true, "Settings applied."));
                        break;
                    case InsertTextCommand insert:
                        var currentTarget = TargetTracker.Capture();
                        if (insert.Message.RestoreTarget && currentTarget != insert.Target)
                        {
                            command.Completion.TrySetResult(
                                new AcknowledgementMessage(
                                    false,
                                    "Focused application changed before insertion."));
                            break;
                        }

                        if (!_dictationInserter.TryQueue(
                            insert.Message.Text,
                            insert.Message.AddTrailingSpace,
                            insert.Target))
                        {
                            _injector.InsertUnicode(
                                insert.Message.Text,
                                insert.Message.AddTrailingSpace);
                        }

                        ResetContext();
                        command.Completion.TrySetResult(
                            new AcknowledgementMessage(true, "Text insertion queued."));
                        break;
                    case ShutdownCommand:
                        command.Completion.TrySetResult(
                            new AcknowledgementMessage(true, "InputHost is stopping."));
                        _ = NativeMethods.PostThreadMessage(
                            _hookThreadId,
                            NativeMethods.WmQuit,
                            0,
                            0);
                        break;
                    case ResetContextCommand:
                        ResetContext();
                        command.Completion.TrySetResult(
                            new AcknowledgementMessage(true, "Context reset."));
                        break;
                }
            }
            catch (Exception exception)
            {
                command.Completion.TrySetResult(
                    new AcknowledgementMessage(
                        false,
                        $"{exception.GetType().Name}: {exception.Message}"));
            }
        }
    }

    private void OnCancelKeyPress(object? sender, ConsoleCancelEventArgs eventArgs)
    {
        eventArgs.Cancel = true;
        _ = NativeMethods.PostThreadMessage(
            _hookThreadId,
            NativeMethods.WmQuit,
            0,
            0);
    }

    private void RemoveHook()
    {
        var hook = Interlocked.Exchange(ref _hook, 0);
        if (hook != 0)
        {
            _ = NativeMethods.UnhookWindowsHookEx(hook);
        }
    }

    private static nuint CreateSyntheticMarker()
    {
        Span<byte> bytes = stackalloc byte[sizeof(ulong)];
        RandomNumberGenerator.Fill(bytes);
        var value = BitConverter.ToUInt64(bytes);
        return (nuint)(value == 0 ? 0x4b42_4f50UL : value);
    }
}
