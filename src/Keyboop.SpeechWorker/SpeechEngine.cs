using System.Diagnostics;
using System.Runtime.Versioning;
using System.Text;
using Keyboop.Core.Speech;
using Keyboop.Protocol;
using Whisper.net;
using Whisper.net.LibraryLoader;

namespace Keyboop.SpeechWorker;

[SupportedOSPlatform("windows10.0.17763")]
internal sealed class SpeechEngine : IAsyncDisposable
{
    private readonly SemaphoreSlim _engineGate = new(1, 1);
    private readonly SemaphoreSlim _dictationGate = new(1, 1);
    private readonly ModelManager _models = new();
    private readonly DictationHistoryStore _history = new();
    private readonly AudioCaptureSession _capture = new();
    private readonly DictationSessionRegistry _sessions = new();
    private WhisperFactory? _factory;
    private WhisperProcessor? _processor;
    private string? _loadedModel;
    private int _processingCount;
    private readonly object _lastTranscriptGate = new();
    private TranscriptMessage _lastTranscript =
        new(string.Empty, "unknown", TimeSpan.Zero, TimeSpan.Zero, true);
    private DateTimeOffset _lastTranscriptStartedAt = DateTimeOffset.MinValue;
    private AppSettings _settings;

    internal SpeechEngine(AppSettings settings)
    {
        _settings = settings;
    }

    internal bool IsListening => _capture.IsRecording;

    internal double AudioLevel => _capture.AudioLevel;

    internal ComponentState State =>
        _capture.IsRecording
            ? ComponentState.Listening
            : Volatile.Read(ref _processingCount) > 0
                ? ComponentState.Processing
                : _capture.DurationLimitReached
                    ? ComponentState.Degraded
                    : ComponentState.Ready;

    internal bool HasLocalModel =>
        _models.HasLocalSpeechModel(_settings.SpeechModel);

    internal string StatusDetail =>
        _capture.DurationLimitReached
            ? "The 20-minute dictation limit was reached; release Ctrl+Win to transcribe."
            : _capture.DurationWarningReached
                ? "Less than one minute remains before the 20-minute dictation limit."
                : RuntimeDetail;

    internal string RuntimeDetail =>
        _factory is null
            ? "Model is not loaded."
            : $"{RuntimeOptions.LoadedLibrary?.ToString() ?? "Unknown runtime"} · "
                + (WhisperFactory.GetRuntimeInfo() ?? "Runtime information unavailable.");

    internal void UpdateSettings(AppSettings settings) => _settings = settings;

    internal async Task InitializeAsync(CancellationToken cancellationToken)
    {
        var entries = await _history.LoadAsync(cancellationToken).ConfigureAwait(false);
        if (entries.Count == 0)
        {
            return;
        }

        var latest = entries[0];
        lock (_lastTranscriptGate)
        {
            _lastTranscript = ToTranscript(latest);
            _lastTranscriptStartedAt = latest.CreatedAt;
        }
    }

    internal TranscriptMessage GetLastTranscript()
    {
        lock (_lastTranscriptGate)
        {
            return _lastTranscript;
        }
    }

    internal Task<IReadOnlyList<DictationHistoryEntry>> GetHistoryAsync(
        CancellationToken cancellationToken) =>
        _history.LoadAsync(cancellationToken);

    internal async Task DeleteHistoryEntryAsync(
        Guid id,
        CancellationToken cancellationToken)
    {
        await _history.DeleteAsync(id, cancellationToken).ConfigureAwait(false);
        await RefreshLastTranscriptFromHistoryAsync(cancellationToken)
            .ConfigureAwait(false);
    }

    internal async Task ClearHistoryAsync(CancellationToken cancellationToken)
    {
        await _history.ClearAsync(cancellationToken).ConfigureAwait(false);
        lock (_lastTranscriptGate)
        {
            _lastTranscript =
                new TranscriptMessage(
                    string.Empty,
                    "unknown",
                    TimeSpan.Zero,
                    TimeSpan.Zero,
                    true);
            _lastTranscriptStartedAt = DateTimeOffset.MinValue;
        }
    }

    internal async Task EnsureModelAsync(CancellationToken cancellationToken)
    {
        await _engineGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            await EnsureModelCoreAsync(cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            _engineGate.Release();
        }
    }

    internal async Task BeginDictationAsync(
        BeginDictationMessage request,
        CancellationToken cancellationToken)
    {
        if (!_settings.DictationEnabled)
        {
            throw new InvalidOperationException("Dictation is disabled.");
        }

        await _dictationGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var startResult = _sessions.TryBegin(
                request.SessionId,
                request.StartedAt);
            if (startResult == DictationSessionStartResult.AlreadyKnown)
            {
                return;
            }

            if (startResult
                == DictationSessionStartResult.AnotherSessionIsRecording)
            {
                throw new InvalidOperationException(
                    "Another dictation session already owns audio capture.");
            }

            try
            {
                if (_capture.HasPendingCapture)
                {
                    throw new InvalidOperationException(
                        "The previous recording is still being closed.");
                }

                _capture.Start();
            }
            catch (Exception exception)
            {
                _sessions.Fail(request.SessionId, exception);
                throw;
            }
        }
        finally
        {
            _dictationGate.Release();
        }
    }

    internal async Task<TranscriptMessage> EndDictationAsync(
        DictationSessionRequest request,
        CancellationToken cancellationToken)
    {
        CapturedAudio? audio = null;
        DictationSessionEndLease lease;
        var processingStarted = false;
        await _dictationGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            lease = _sessions.BeginEnd(request.SessionId);
            if (lease.OwnsFinalization)
            {
                if (!_capture.HasPendingCapture)
                {
                    var exception = new InvalidOperationException(
                        "Audio capture is not active.");
                    _sessions.Fail(request.SessionId, exception);
                    throw exception;
                }

                Interlocked.Increment(ref _processingCount);
                processingStarted = true;
                try
                {
                    audio = await _capture.StopAsync(cancellationToken)
                        .ConfigureAwait(false);
                }
                catch (Exception exception)
                {
                    _sessions.Fail(request.SessionId, exception);
                    Interlocked.Decrement(ref _processingCount);
                    processingStarted = false;
                    throw;
                }
            }
        }
        finally
        {
            _dictationGate.Release();
        }

        if (!lease.OwnsFinalization)
        {
            return await lease.Completion.WaitAsync(cancellationToken)
                .ConfigureAwait(false);
        }

        try
        {
            var transcript = (await TranscribeCoreAsync(audio!, cancellationToken)
                .ConfigureAwait(false)) with
            {
                SessionId = request.SessionId
            };
            await RememberTranscriptAsync(
                    request.SessionId,
                    lease.StartedAt,
                    transcript,
                    cancellationToken)
                .ConfigureAwait(false);
            _sessions.Complete(request.SessionId, transcript);
            return transcript;
        }
        catch (Exception exception)
        {
            _sessions.Fail(request.SessionId, exception);
            throw;
        }
        finally
        {
            if (processingStarted)
            {
                Interlocked.Decrement(ref _processingCount);
            }
        }
    }

    internal async Task<TranscriptMessage> GetDictationTranscriptAsync(
        DictationSessionRequest request,
        CancellationToken cancellationToken)
    {
        if (_sessions.TryGetCompletion(request.SessionId) is not null)
        {
            // Recovery is a complete session operation, not a passive read.
            // If the separate End request was delayed or lost, recovery may own
            // finalization itself. Concurrent End calls share the registry task.
            return await EndDictationAsync(request, cancellationToken)
                .ConfigureAwait(false);
        }

        var entries = await _history.LoadAsync(cancellationToken)
            .ConfigureAwait(false);
        var entry = entries.FirstOrDefault(
            candidate => candidate.SessionId == request.SessionId);
        return entry is null
            ? throw new InvalidOperationException(
                $"Dictation session {request.SessionId:D} is unknown.")
            : ToTranscript(entry);
    }

    internal async Task<TranscriptMessage> TranscribeWaveFileAsync(
        string path,
        CancellationToken cancellationToken)
    {
        Interlocked.Increment(ref _processingCount);
        try
        {
            return await TranscribeCoreAsync(
                AudioCaptureSession.ReadWaveFile(path),
                cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            Interlocked.Decrement(ref _processingCount);
        }
    }

    private async Task<TranscriptMessage> TranscribeCoreAsync(
        CapturedAudio audio,
        CancellationToken cancellationToken)
    {
        if (!SpeechQualityGate.HasAudibleSignal(audio.Samples))
        {
            return new TranscriptMessage(
                string.Empty,
                "unknown",
                audio.Duration,
                TimeSpan.Zero,
                true);
        }

        await _engineGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            await EnsureModelCoreAsync(cancellationToken).ConfigureAwait(false);
            var stopwatch = Stopwatch.StartNew();
            var text = new StringBuilder();
            var language = _settings.SpeechLanguage;
            await foreach (var segment in _processor!.ProcessAsync(
                audio.Samples,
                cancellationToken))
            {
                text.Append(segment.Text);
                if (!string.IsNullOrWhiteSpace(segment.Language))
                {
                    language = segment.Language;
                }
            }

            stopwatch.Stop();
            var normalized = NormalizeTranscript(text.ToString());
            return new TranscriptMessage(
                SpeechQualityGate.HasMeaningfulTranscript(normalized)
                    ? normalized
                    : string.Empty,
                language,
                audio.Duration,
                stopwatch.Elapsed,
                true);
        }
        finally
        {
            _engineGate.Release();
        }
    }

    internal async Task CancelDictationAsync(
        DictationSessionRequest request,
        CancellationToken cancellationToken)
    {
        await _dictationGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (!_sessions.Cancel(request.SessionId))
            {
                return;
            }

            if (_capture.HasPendingCapture)
            {
                _ = await _capture.StopAsync(cancellationToken).ConfigureAwait(false);
            }
        }
        finally
        {
            _dictationGate.Release();
        }
    }

    public async ValueTask DisposeAsync()
    {
        await _capture.DisposeAsync().ConfigureAwait(false);
        if (_processor is not null)
        {
            await _processor.DisposeAsync().ConfigureAwait(false);
        }

        _factory?.Dispose();
        _dictationGate.Dispose();
        _models.Dispose();
        _history.Dispose();
        _engineGate.Dispose();
    }

    private async Task EnsureModelCoreAsync(CancellationToken cancellationToken)
    {
        var requestedModel = _settings.SpeechModel;
        if (_processor is not null
            && string.Equals(_loadedModel, requestedModel, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        var modelPath = await _models.EnsureSpeechModelAsync(
            requestedModel,
            cancellationToken).ConfigureAwait(false);
        if (_processor is not null)
        {
            await _processor.DisposeAsync().ConfigureAwait(false);
        }

        _factory?.Dispose();
        var preferCuda = string.Equals(
            _settings.PreferredSpeechBackend,
            "cuda",
            StringComparison.OrdinalIgnoreCase);
        if (preferCuda)
        {
            _ = CudaRuntimeLocator.Prepare();
        }

        RuntimeOptions.RuntimeLibraryOrder = preferCuda
                ?
                [
                    RuntimeLibrary.Cuda,
                    RuntimeLibrary.Cuda12,
                    RuntimeLibrary.Cpu,
                    RuntimeLibrary.CpuNoAvx
                ]
                :
                [
                    RuntimeLibrary.Cpu,
                    RuntimeLibrary.CpuNoAvx
                ];
        _factory = WhisperFactory.FromPath(
            modelPath,
            new WhisperFactoryOptions
            {
                UseGpu = string.Equals(
                    _settings.PreferredSpeechBackend,
                    "cuda",
                    StringComparison.OrdinalIgnoreCase),
                UseFlashAttention = true
            });
        _processor = _factory.CreateBuilder()
            .WithLanguage(_settings.SpeechLanguage)
            .WithThreads(Math.Clamp(Environment.ProcessorCount / 2, 4, 10))
            .WithGreedySamplingStrategy(static strategy => strategy.WithBestOf(1))
            .Build();
        _loadedModel = requestedModel;
    }

    private static string NormalizeTranscript(string text)
    {
        var normalized = string.Join(
            " ",
            text.Split(
                (char[]?)null,
                StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries));
        return normalized.Trim();
    }

    private async Task RememberTranscriptAsync(
        Guid sessionId,
        DateTimeOffset startedAt,
        TranscriptMessage transcript,
        CancellationToken cancellationToken)
    {
        if (string.IsNullOrWhiteSpace(transcript.Text))
        {
            return;
        }

        var settings = _settings;
        lock (_lastTranscriptGate)
        {
            if (startedAt >= _lastTranscriptStartedAt)
            {
                _lastTranscript = transcript;
                _lastTranscriptStartedAt = startedAt;
            }
        }

        if (settings.KeepDictationHistory)
        {
            await _history.AppendAsync(
                new DictationHistoryEntry(
                    sessionId,
                    startedAt,
                    transcript.Text,
                    transcript.Language,
                    transcript.AudioDuration,
                    transcript.ProcessingDuration,
                    sessionId),
                cancellationToken).ConfigureAwait(false);
        }
    }

    private async Task RefreshLastTranscriptFromHistoryAsync(
        CancellationToken cancellationToken)
    {
        var entries = await _history.LoadAsync(cancellationToken).ConfigureAwait(false);
        lock (_lastTranscriptGate)
        {
            _lastTranscript = entries.Count > 0
                ? ToTranscript(entries[0])
                : new TranscriptMessage(
                    string.Empty,
                    "unknown",
                    TimeSpan.Zero,
                    TimeSpan.Zero,
                    true);
            _lastTranscriptStartedAt = entries.Count > 0
                ? entries[0].CreatedAt
                : DateTimeOffset.MinValue;
        }
    }

    private static TranscriptMessage ToTranscript(DictationHistoryEntry entry) =>
        new(
            entry.Text,
            entry.Language,
            entry.AudioDuration,
            entry.ProcessingDuration,
            true,
            entry.SessionId);
}
