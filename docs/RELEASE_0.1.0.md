# Keyboop for Windows 0.1.0

## Ready to test

Installer:

`C:\PRDJKT\keyboop\dist\Keyboop-0.1.0-win-x64-setup.exe`

- Size: `184581814` bytes (`176.0 MiB`)
- SHA-256:
  `7624F3FA3DBEC9F4EF34743FB974A56ACBF402551E6750C8B9041092BAA37F03`
- Installation scope: current user
- Default path: `%LOCALAPPDATA%\Programs\Keyboop`
- Administrator rights: not required
- Code signing: intentionally unsigned
- The Whisper model is stored separately under
  `%LOCALAPPDATA%\Keyboop\models`.

## Implemented

- Explicit selected-text RU/EN layout conversion with preview and cancellation.
- Case, punctuation, whitespace, and multiline preservation.
- Clipboard transaction with independent format snapshot and guarded restoration.
- Automatic layout replacement disabled; ordinary typing is not inspected or changed.
- Local push-to-talk and toggle dictation using WASAPI, Whisper
  `large-v3-turbo`, and CUDA.
- Persistent background model warmup with verified CPU fallback.
- Quiet-audio energy gate and punctuation-only hallucination rejection.
- Model size and SHA-256 verification before loading.
- Per-user installer, optional startup/desktop shortcuts, single-instance UI,
  tray lifecycle, and self-contained .NET runtime.

## Hotkeys

- `Ctrl+Alt+K`: preview correction for selected text.
- `Ctrl+Alt+Space`: hold to dictate.
- `Ctrl+Alt+D`: start/stop dictation.
- `Ctrl+Alt+P`: pause/resume Keyboop input features.

## Acceptance evidence

- Release build: 0 warnings, 0 errors.
- Automated tests: 51 passed, 0 failed.
- Portable manifest: 305 files, 0 size/hash mismatches.
- Manual installed-app test in Notepad:
  `ghbdtn, vbh!` → preview → `привет, мир!`.
- Default typing pass-through verified with ordinary spaces.
- Empty microphone recording left the target sentinel unchanged.
- Russian speech fixture:
  `Привет, мир! Это проверка локального голосового ввода на компьютере.`
  transcribed exactly.
- Fixture audio: 6.94 seconds; CUDA inference: approximately 0.35 seconds.
- Installed worker status: `Ready`, backend `Cuda`.

## Known limits

- The installer is unsigned, so Windows may show a SmartScreen warning on another
  machine.
- Input into elevated applications is unsupported by Windows UIPI unless Keyboop
  runs at the same integrity level; Keyboop does not elevate itself.
- Selected text currently uses a guarded clipboard compatibility path.
- The first release has been exercised in Notepad. Word, Chromium/Electron,
  VS Code, Telegram, Windows Terminal, RDP, and unusual rich-text editors still
  need compatibility-matrix runs.
- A live human-microphone acceptance pass is still required for microphone choice,
  input level, accent, punctuation preference, and hotkey ergonomics.
- Snippets, local translation, searchable dictation history, updates, and
  per-application profiles remain planned work.
