using System.Diagnostics;
using System.IO.Pipes;
using System.Reflection;
using System.Threading.Channels;
using Keyboop.Protocol;

namespace Keyboop.InputHost;

internal sealed class InputHostControlServer : IAsyncDisposable
{
    private readonly ChannelWriter<HostControlCommand> _commands;
    private readonly Action _wakeHost;
    private readonly CancellationTokenSource _shutdown = new();
    private readonly Task _serverTask;

    internal InputHostControlServer(
        ChannelWriter<HostControlCommand> commands,
        Action wakeHost)
    {
        _commands = commands;
        _wakeHost = wakeHost;
        _serverTask = RunServerAsync();
    }

    public async ValueTask DisposeAsync()
    {
        _shutdown.Cancel();
        try
        {
            await _serverTask.ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (_shutdown.IsCancellationRequested)
        {
        }

        _shutdown.Dispose();
    }

    private async Task RunServerAsync()
    {
        while (!_shutdown.IsCancellationRequested)
        {
            await using var pipe = new NamedPipeServerStream(
                ProtocolConstants.InputHostPipeName,
                PipeDirection.InOut,
                1,
                PipeTransmissionMode.Byte,
                PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
            try
            {
                await pipe.WaitForConnectionAsync(_shutdown.Token).ConfigureAwait(false);
                await HandleClientAsync(pipe, _shutdown.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (_shutdown.IsCancellationRequested)
            {
                return;
            }
            catch (IOException)
            {
                // A UI restart or crash simply creates a new pipe session.
            }
            catch (ProtocolException exception)
            {
                Console.Error.WriteLine($"Control protocol error: {exception.Message}");
            }
        }
    }

    private async Task HandleClientAsync(
        NamedPipeServerStream pipe,
        CancellationToken cancellationToken)
    {
        await PipeFraming.WriteAsync(
            pipe,
            ProtocolEnvelope.Create(
                MessageKind.Hello,
                new HelloMessage(
                    ComponentKind.InputHost,
                    Environment.ProcessId,
                    Assembly.GetExecutingAssembly().GetName().Version?.ToString() ?? "dev")),
            cancellationToken).ConfigureAwait(false);

        while (pipe.IsConnected && !cancellationToken.IsCancellationRequested)
        {
            var envelope = await PipeFraming.ReadAsync(pipe, cancellationToken)
                .ConfigureAwait(false);
            if (envelope is null)
            {
                return;
            }

            var response = await DispatchAsync(envelope, cancellationToken)
                .ConfigureAwait(false);
            await PipeFraming.WriteAsync(pipe, response, cancellationToken)
                .ConfigureAwait(false);
        }
    }

    private async Task<ProtocolEnvelope> DispatchAsync(
        ProtocolEnvelope envelope,
        CancellationToken cancellationToken)
    {
        HostControlCommand? command = envelope.Kind switch
        {
            MessageKind.UpdateSettings =>
                new UpdateSettingsCommand(envelope.GetPayload<AppSettings>()),
            MessageKind.InsertText =>
                new InsertTextCommand(
                    envelope.GetPayload<InsertTextMessage>(),
                    TargetTracker.Capture()),
            MessageKind.Shutdown => new ShutdownCommand(),
            _ => null
        };

        if (envelope.Kind == MessageKind.StatusRequest)
        {
            return ProtocolEnvelope.Create(
                MessageKind.Status,
                new StatusMessage(
                    ComponentKind.InputHost,
                    ComponentState.Ready,
                    "Keyboard hook is active.",
                    DateTimeOffset.UtcNow),
                envelope.RequestId);
        }

        if (command is null)
        {
            return ProtocolEnvelope.Create(
                MessageKind.Error,
                new ErrorMessage(
                    "unsupported_message",
                    $"InputHost does not accept {envelope.Kind}.",
                    false),
                envelope.RequestId);
        }

        if (!_commands.TryWrite(command))
        {
            return ProtocolEnvelope.Create(
                MessageKind.Error,
                new ErrorMessage(
                    "host_stopping",
                    "InputHost is stopping.",
                    true),
                envelope.RequestId);
        }

        _wakeHost();
        try
        {
            var result = await command.Completion.Task
                .WaitAsync(TimeSpan.FromSeconds(2), cancellationToken)
                .ConfigureAwait(false);
            return ProtocolEnvelope.Create(
                MessageKind.Acknowledgement,
                result,
                envelope.RequestId);
        }
        catch (TimeoutException)
        {
            return ProtocolEnvelope.Create(
                MessageKind.Error,
                new ErrorMessage(
                    "host_timeout",
                    "InputHost did not process the command in time.",
                    true),
                envelope.RequestId);
        }
    }
}
