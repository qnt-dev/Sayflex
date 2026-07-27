using Keyboop.Protocol;
using Keyboop.Core.Input;

namespace Keyboop.InputHost;

internal abstract record HostControlCommand
{
    internal TaskCompletionSource<AcknowledgementMessage> Completion { get; } =
        new(TaskCreationOptions.RunContinuationsAsynchronously);
}

internal sealed record UpdateSettingsCommand(AppSettings Settings) : HostControlCommand;

internal sealed record InsertTextCommand(
    InsertTextMessage Message,
    TargetSnapshot Target) : HostControlCommand;

internal sealed record ShutdownCommand : HostControlCommand;

internal sealed record ResetContextCommand : HostControlCommand;
