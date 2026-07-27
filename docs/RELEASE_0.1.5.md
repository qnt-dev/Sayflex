# Keyboop for Windows 0.1.5

This release makes the dictation indicator feel immediate and gives Keyboop its
own visual identity.

## Changes

- Reduced the idle speech-status interval from 750 ms to 50 ms so the indicator
  appears promptly after dictation begins.
- Changed the 112×42 indicator from a pill to a moderately rounded 10 px panel.
- Applied the requested palette: `#272727` background, subtly lighter `#2C2F31`
  frame, and `#FFD400` waveform and status control.
- Kept the same waveform and dimensions while transitioning from listening to
  processing.
- Added an original Keyboop waveform icon, embedded in the executable and
  installer and used by the tray.
- Added a repeatable safe latency probe that starts SpeechWorker directly and
  does not inject text into the foreground application.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 58 passed (`Core` 49, `InputReplay` 9), zero failed.
- Five probes against the installed release showed the indicator after 33–61 ms,
  with a 46.2 ms average. This measures SpeechWorker
  begin-to-visible-indicator latency; the physical `Ctrl+Win` path still requires
  live user acceptance.
- Installer:
  `Keyboop-0.1.5-win-x64-setup.exe`, 193,288,162 bytes,
  SHA-256
  `D4D55616440E05EB89A9D6E31CAB78AEF82FCBC9846CC9A58AF1C2F4712DDFF8`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.5`.
