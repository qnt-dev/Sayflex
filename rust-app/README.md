# Rust-first desktop application

This directory is an independent Rust/Slint application. The existing Windows
C# application in the repository root remains a separate product and is not a
build dependency of this workspace.

## Process architecture

The release contains four failure-isolated executables:

- `rust-desktop.exe` — Slint settings, tray, history, model selection and
  recording/selection overlays.
- `rust-input-host.exe` — Windows low-level keyboard hook, immutable target
  capture, Unicode insertion, clipboard recovery and selected-text capture.
- `rust-speech-worker-*.exe` — CPAL microphone capture, verified model
  installation, CUDA/CPU `whisper.cpp` inference and durable history.
- `rust-translation-worker.exe` — pure-Rust Candle/Marian inference,
  verified multi-file translation model installation and translation history.

Every recording has a UUID that is carried through hotkey, audio, inference,
history, insertion and recovery. No operation reads a mutable global “last
text” value while a session is completing.

## Windows hotkeys

- Hold `Ctrl + Alt + Space` to record; release `Space` or either required
  modifier to transcribe and insert.
- `Ctrl + Alt + V` inserts the newest dictation into the currently focused
  target. If that dictation is still processing, the request waits for that
  exact session instead of inserting an older transcript.
- `Ctrl + Alt + K` captures selected text and offers explicit RU/EN keyboard
  layout candidates. Ordinary text is never changed automatically.
- `Ctrl + Alt + T` captures selected text once. With one enabled action it
  runs directly; with several enabled actions it opens the text command
  palette.

The indicator is pre-created and shown from the input-host event pump, before
model work begins. It is topmost, click-through and non-activating, so it does
not replace the recorded target window.

## Models and output

The built-in catalog recommends `large-v3-turbo` and also contains `small`,
`medium`, and `large-v3`. Model downloads are streamed to a temporary file,
checked for exact length and SHA-256, then atomically installed. The worker can
adopt the already verified C# model using an NTFS hard link without modifying
or duplicating it.

Speech output remains in the detected language for the recommended
`large-v3-turbo` model. Turbo is deliberately not advertised as a translation
model: upstream does not train it for that task. Direct speech-to-English mode
is available only for compatible non-Turbo Whisper models.

Selected-text translation and the full translator use a separate local Marian
engine. No translation pack is preinstalled or downloaded automatically.
Users can choose a compact Fast pack or the original higher-quality Teacher
pack; the system locale is used only to mark a recommendation. Translation
can preview, copy or replace the selection.

Translation and layout correction can be disabled independently.
`Ctrl+Alt+T` can adapt when only one action is enabled or always show the
palette. The translation worker can remain warm for the lowest latency or
unload after every request to reduce memory. Voice and translation history
can be disabled and cleared from Settings.

CUDA is the preferred backend on the current RTX 3080 Ti machine. A separately
built CPU worker is shipped as the fallback for systems without the required
CUDA runtime.

## Build, test and package

```powershell
cd C:\PRDJKT\keyboop\rust-app
cargo test --workspace
.\scripts\publish.ps1
.\scripts\build-installer.ps1 -Version 0.5.0 -SkipPublish
```

The measured warm-path design and benchmark gate are documented in
[`docs/LATENCY_ARCHITECTURE.md`](docs/LATENCY_ARCHITECTURE.md).
The visual tokens, adaptive popup geometry and non-activating Acrylic policy
are documented in [`docs/DESIGN_SYSTEM.md`](docs/DESIGN_SYSTEM.md).

Transient windows have isolated visual preview modes that do not register the
global hook or touch another application:

```powershell
.\target\debug\rust-desktop.exe --indicator-preview
.\target\debug\rust-desktop.exe --action-preview
.\target\debug\rust-desktop.exe --translation-preview
.\target\debug\rust-desktop.exe --layout-preview
```

The installer is emitted under `rust-app\dist`; it does not overwrite the
existing C# installer or its application directory.

## License

The workspace is MIT-licensed. Slint is consumed under its royalty-free
desktop application license. A release must retain the required Slint
attribution in the About surface or on the public download page.
