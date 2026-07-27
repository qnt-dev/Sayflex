using System.IO.Pipes;
using System.IO;
using Keyboop.Protocol;

namespace Keyboop.UI;

internal sealed class ComponentClient : IAsyncDisposable
{
    private readonly string _pipeName;
    private readonly SemaphoreSlim _requestGate = new(1, 1);
    private NamedPipeClientStream? _pipe;

    internal ComponentClient(string pipeName)
    {
        _pipeName = pipeName;
    }

    internal bool IsConnected => _pipe?.IsConnected == true;

    internal async Task<bool> TryConnectAsync(
        TimeSpan timeout,
        CancellationToken cancellationToken = default)
    {
        if (IsConnected)
        {
            return true;
        }

        await DisconnectAsync().ConfigureAwait(false);
        var pipe = new NamedPipeClientStream(
            ".",
            _pipeName,
            PipeDirection.InOut,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        try
        {
            await pipe.ConnectAsync(timeout, cancellationToken).ConfigureAwait(false);
            var hello = await PipeFraming.ReadAsync(pipe, cancellationToken)
                .ConfigureAwait(false);
            if (hello?.Kind != MessageKind.Hello)
            {
                throw new ProtocolException("Component did not send a hello message.");
            }

            _pipe = pipe;
            return true;
        }
        catch (Exception) when (!cancellationToken.IsCancellationRequested)
        {
            await pipe.DisposeAsync().ConfigureAwait(false);
            return false;
        }
    }

    internal async Task<ProtocolEnvelope> RequestAsync(
        ProtocolEnvelope request,
        TimeSpan timeout,
        CancellationToken cancellationToken = default)
    {
        await _requestGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            using var requestTimeout = CancellationTokenSource.CreateLinkedTokenSource(
                cancellationToken);
            requestTimeout.CancelAfter(timeout);
            if (!await TryConnectAsync(TimeSpan.FromSeconds(1), requestTimeout.Token)
                .ConfigureAwait(false))
            {
                throw new IOException($"Could not connect to {_pipeName}.");
            }

            try
            {
                await PipeFraming.WriteAsync(_pipe!, request, requestTimeout.Token)
                    .ConfigureAwait(false);
                return await PipeFraming.ReadAsync(_pipe!, requestTimeout.Token)
                    .ConfigureAwait(false)
                    ?? throw new EndOfStreamException("Component closed the pipe.");
            }
            catch
            {
                await DisconnectAsync().ConfigureAwait(false);
                throw;
            }
        }
        finally
        {
            _requestGate.Release();
        }
    }

    public async ValueTask DisposeAsync()
    {
        await DisconnectAsync().ConfigureAwait(false);
        _requestGate.Dispose();
    }

    private async Task DisconnectAsync()
    {
        var pipe = Interlocked.Exchange(ref _pipe, null);
        if (pipe is not null)
        {
            await pipe.DisposeAsync().ConfigureAwait(false);
        }
    }
}
