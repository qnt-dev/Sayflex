using System.Runtime.Versioning;
using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using NAudio.Wave.SampleProviders;

namespace Keyboop.SpeechWorker;

[SupportedOSPlatform("windows10.0.17763")]
internal sealed class AudioCaptureSession : IAsyncDisposable
{
    internal static readonly TimeSpan MaximumDuration = TimeSpan.FromMinutes(20);
    internal static readonly TimeSpan WarningDuration = TimeSpan.FromMinutes(19);

    private readonly object _gate = new();
    private WasapiCapture? _capture;
    private FileStream? _waveStream;
    private WaveFileWriter? _writer;
    private string? _wavePath;
    private long _maximumWaveBytes;
    private long _warningWaveBytes;
    private bool _recordingStopped;
    private bool _durationLimitReached;
    private bool _durationWarningReached;
    private bool _stopScheduled;
    private float _audioLevel;
    private TaskCompletionSource _stopped =
        new(TaskCreationOptions.RunContinuationsAsynchronously);

    internal bool IsRecording
    {
        get
        {
            lock (_gate)
            {
                return _capture is not null && !_recordingStopped;
            }
        }
    }

    internal bool HasPendingCapture
    {
        get
        {
            lock (_gate)
            {
                return _capture is not null;
            }
        }
    }

    internal double AudioLevel => Volatile.Read(ref _audioLevel);

    internal bool DurationLimitReached
    {
        get
        {
            lock (_gate)
            {
                return _durationLimitReached;
            }
        }
    }

    internal bool DurationWarningReached
    {
        get
        {
            lock (_gate)
            {
                return _durationWarningReached;
            }
        }
    }

    internal void Start()
    {
        lock (_gate)
        {
            if (_capture is not null)
            {
                return;
            }

            _stopped = new TaskCompletionSource(
                TaskCreationOptions.RunContinuationsAsynchronously);
            var captureDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Keyboop",
                "capture");
            Directory.CreateDirectory(captureDirectory);
            _wavePath = Path.Combine(
                captureDirectory,
                $"dictation-{DateTime.UtcNow:yyyyMMdd-HHmmss}-{Guid.NewGuid():N}.wav");
            _waveStream = new FileStream(
                _wavePath,
                FileMode.CreateNew,
                FileAccess.ReadWrite,
                FileShare.Read,
                bufferSize: 1024 * 1024,
                FileOptions.SequentialScan);
            _capture = new WasapiCapture();
            _writer = new WaveFileWriter(_waveStream, _capture.WaveFormat);
            _maximumWaveBytes = checked(
                (long)(_capture.WaveFormat.AverageBytesPerSecond
                    * MaximumDuration.TotalSeconds)
                + 4_096);
            _warningWaveBytes = checked(
                (long)(_capture.WaveFormat.AverageBytesPerSecond
                    * WarningDuration.TotalSeconds)
                + 4_096);
            _recordingStopped = false;
            _durationLimitReached = false;
            _durationWarningReached = false;
            _stopScheduled = false;
            Volatile.Write(ref _audioLevel, 0);
            _capture.DataAvailable += OnDataAvailable;
            _capture.RecordingStopped += OnRecordingStopped;
            _capture.StartRecording();
        }
    }

    internal async Task<CapturedAudio> StopAsync(CancellationToken cancellationToken)
    {
        WasapiCapture capture;
        Task stoppedTask;
        var requestStop = false;
        lock (_gate)
        {
            capture = _capture
                ?? throw new InvalidOperationException("Audio capture is not active.");
            requestStop = !_recordingStopped && !_stopScheduled;
            stoppedTask = _stopped.Task;
        }

        Exception? stopException = null;
        try
        {
            if (requestStop)
            {
                capture.StopRecording();
            }

            await stoppedTask.WaitAsync(TimeSpan.FromSeconds(5), cancellationToken)
                .ConfigureAwait(false);
        }
        catch (Exception exception)
        {
            stopException = exception;
        }

        WaveFileWriter? writer;
        FileStream? waveStream;
        string? wavePath;
        lock (_gate)
        {
            writer = _writer;
            waveStream = _waveStream;
            wavePath = _wavePath;
            capture.DataAvailable -= OnDataAvailable;
            capture.RecordingStopped -= OnRecordingStopped;
            _writer = null;
            _waveStream = null;
            _wavePath = null;
            _capture = null;
            _recordingStopped = false;
            _durationLimitReached = false;
            _durationWarningReached = false;
            _stopScheduled = false;
            _maximumWaveBytes = 0;
            _warningWaveBytes = 0;
            Volatile.Write(ref _audioLevel, 0);
        }

        try
        {
            if (stopException is not null)
            {
                ExceptionDispatchInfo.Capture(stopException).Throw();
            }

            var duration = writer?.TotalTime ?? TimeSpan.Zero;
            writer?.Flush();
            writer?.Dispose();
            writer = null;
            waveStream?.Dispose();
            waveStream = null;
            if (string.IsNullOrWhiteSpace(wavePath))
            {
                return new CapturedAudio([], duration);
            }

            var captured = ReadWaveFile(wavePath);
            return captured with { Duration = duration };
        }
        finally
        {
            writer?.Dispose();
            waveStream?.Dispose();
            capture.Dispose();
            TryDelete(wavePath);
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (!HasPendingCapture)
        {
            return;
        }

        try
        {
            _ = await StopAsync(CancellationToken.None).ConfigureAwait(false);
        }
        catch
        {
            lock (_gate)
            {
                var wavePath = _wavePath;
                _writer?.Dispose();
                _capture?.Dispose();
                _writer = null;
                _waveStream = null;
                _wavePath = null;
                _capture = null;
                TryDelete(wavePath);
            }
        }
    }

    internal static CapturedAudio ReadWaveFile(string path)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        using var reader = new WaveFileReader(path);
        return new CapturedAudio(
            ConvertToWhisperSamples(reader),
            reader.TotalTime);
    }

    private void OnDataAvailable(object? sender, WaveInEventArgs eventArgs)
    {
        WasapiCapture? captureToStop = null;
        var capture = _capture;
        if (capture is not null)
        {
            var measured = MeasureAudioLevel(
                capture.WaveFormat,
                eventArgs.Buffer.AsSpan(0, eventArgs.BytesRecorded));
            var previous = Volatile.Read(ref _audioLevel);
            Volatile.Write(
                ref _audioLevel,
                Math.Clamp((previous * 0.62f) + (measured * 0.38f), 0, 1));
        }

        lock (_gate)
        {
            if (_writer is null || _waveStream is null)
            {
                return;
            }

            var remaining = Math.Max(0, _maximumWaveBytes - _waveStream.Length);
            var bytesToWrite = (int)Math.Min(eventArgs.BytesRecorded, remaining);
            if (bytesToWrite > 0)
            {
                _writer.Write(eventArgs.Buffer, 0, bytesToWrite);
            }

            if (_waveStream.Length >= _warningWaveBytes)
            {
                _durationWarningReached = true;
            }

            if (bytesToWrite < eventArgs.BytesRecorded
                || _waveStream.Length >= _maximumWaveBytes)
            {
                _durationLimitReached = true;
                if (!_stopScheduled)
                {
                    _stopScheduled = true;
                    captureToStop = _capture;
                }
            }
        }

        if (captureToStop is not null)
        {
            _ = Task.Run(
                () =>
                {
                    try
                    {
                        captureToStop.StopRecording();
                    }
                    catch (Exception exception)
                    {
                        _stopped.TrySetException(exception);
                    }
                });
        }
    }

    private void OnRecordingStopped(object? sender, StoppedEventArgs eventArgs)
    {
        lock (_gate)
        {
            _recordingStopped = true;
        }

        if (eventArgs.Exception is not null)
        {
            _stopped.TrySetException(eventArgs.Exception);
        }
        else
        {
            _stopped.TrySetResult();
        }
    }

    private static float[] ConvertToWhisperSamples(WaveFileReader reader)
    {
        if (reader.Length == 0)
        {
            return [];
        }

        ISampleProvider samples = reader.ToSampleProvider();
        if (samples.WaveFormat.Channels > 1)
        {
            samples = new DownmixToMonoSampleProvider(samples);
        }

        if (samples.WaveFormat.SampleRate != 16_000)
        {
            samples = new WdlResamplingSampleProvider(samples, 16_000);
        }

        var output = new List<float>(
            Math.Max(16_000, (int)(reader.TotalTime.TotalSeconds * 16_000)));
        var buffer = new float[16_000];
        int read;
        while ((read = samples.Read(buffer, 0, buffer.Length)) > 0)
        {
            output.AddRange(buffer.AsSpan(0, read).ToArray());
        }

        return output.ToArray();
    }

    private static float MeasureAudioLevel(WaveFormat format, ReadOnlySpan<byte> bytes)
    {
        if (bytes.IsEmpty)
        {
            return 0;
        }

        double sum = 0;
        var samples = 0;
        if (format.BitsPerSample == 32)
        {
            var aligned = bytes[..(bytes.Length - (bytes.Length % sizeof(float)))];
            foreach (var sample in MemoryMarshal.Cast<byte, float>(aligned))
            {
                if (!float.IsFinite(sample))
                {
                    continue;
                }

                var normalized = Math.Clamp(sample, -1, 1);
                sum += normalized * normalized;
                samples++;
            }
        }
        else if (format.BitsPerSample == 16)
        {
            var aligned = bytes[..(bytes.Length - (bytes.Length % sizeof(short)))];
            foreach (var sample in MemoryMarshal.Cast<byte, short>(aligned))
            {
                var normalized = sample / 32768f;
                sum += normalized * normalized;
                samples++;
            }
        }

        if (samples == 0)
        {
            return 0;
        }

        var rootMeanSquare = Math.Sqrt(sum / samples);
        return (float)Math.Clamp(rootMeanSquare * 5.5, 0, 1);
    }

    private static void TryDelete(string? path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return;
        }

        try
        {
            File.Delete(path);
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}

internal sealed record CapturedAudio(float[] Samples, TimeSpan Duration);

internal sealed class DownmixToMonoSampleProvider : ISampleProvider
{
    private readonly ISampleProvider _source;
    private readonly int _channels;
    private float[] _sourceBuffer = [];

    internal DownmixToMonoSampleProvider(ISampleProvider source)
    {
        _source = source;
        _channels = source.WaveFormat.Channels;
        WaveFormat = WaveFormat.CreateIeeeFloatWaveFormat(
            source.WaveFormat.SampleRate,
            1);
    }

    public WaveFormat WaveFormat { get; }

    public int Read(float[] buffer, int offset, int count)
    {
        var required = count * _channels;
        if (_sourceBuffer.Length < required)
        {
            _sourceBuffer = new float[required];
        }

        var sourceRead = _source.Read(_sourceBuffer, 0, required);
        var frames = sourceRead / _channels;
        for (var frame = 0; frame < frames; frame++)
        {
            var sum = 0f;
            for (var channel = 0; channel < _channels; channel++)
            {
                sum += _sourceBuffer[(frame * _channels) + channel];
            }

            buffer[offset + frame] = sum / _channels;
        }

        return frames;
    }
}
