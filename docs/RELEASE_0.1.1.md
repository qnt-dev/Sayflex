# Keyboop for Windows 0.1.1

> Superseded by `0.1.2`. This release fixed the original unbounded recording
> failure but retained toggle-state behavior that was vulnerable to a missed or
> confusing stop action. Use `0.1.2` and its hold-to-talk `Ctrl+Win` workflow.

Installer:

`C:\PRDJKT\keyboop\dist\Keyboop-0.1.1-win-x64-setup.exe`

- Size: `184593116` bytes (`176.0 MiB`)
- SHA-256:
  `68A9E30E0C52FC3A9617AE5ED5BB8B55E85A2B16B93E31219EDD17977290F88E`
- Installation scope: current user; administrator rights are not required.

## Dictation hotkey repair

The original `0.1.0` global hotkey did reach SpeechWorker, but there was no visible
feedback. A toggle session could therefore remain active unnoticed. Because raw
WASAPI data was buffered without a hard limit, a sufficiently long abandoned
session eventually failed with `IOException: Stream was too long` and remained
stuck in the listening state.

`0.1.1` changes that behavior:

- `Ctrl+Alt+D` now shows a non-activating topmost `Слушаю…` indicator.
- stopping a session shows `Распознаю…` until final recognition completes;
- the indicator follows the monitor containing the foreground application;
- the tray also reports listening and maximum-duration transitions;
- one recording is capped at five minutes;
- stopped/error sessions always detach and dispose WASAPI, writer, and memory
  resources;
- concurrent finalization requests are serialized;
- a failed or completed recording can no longer leave the worker permanently in
  `Listening`.

## Validation

- Release build: 0 warnings, 0 errors.
- Automated tests: 51 passed, 0 failed.
- `dotnet format --verify-no-changes`: passed.
- Two consecutive `Ctrl+Alt+D` start/stop cycles returned to `Ready`.
- The `Keyboop — диктовка` indicator appeared on both start cycles and disappeared
  on completion without taking focus.
- The pinned Russian speech fixture still transcribed exactly on CUDA in about
  0.35 seconds for 6.94 seconds of audio.
