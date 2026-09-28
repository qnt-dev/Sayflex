# SayFlex

This directory is an independent Rust/Slint application. The existing Windows
C# application in the repository root remains a separate product and is not a
build dependency of this workspace.

## Process architecture

The release contains six failure-isolated process roles and eight executables:

- `SayFlex.exe` — Slint settings, tray, history, model selection and
  recording/selection overlays.
- `SayFlex.InputHost.exe` — Windows low-level keyboard hook, immutable target
  capture, Unicode insertion, clipboard recovery and selected-text capture.
- `SayFlex.Speech.*.exe` — CPAL microphone capture, verified model
  installation, CUDA/Vulkan/CPU `whisper.cpp` inference and durable history.
- `SayFlex.Translation.exe` — verified Qwen3.5 model/runtime installation,
  compact Firefox translation, lifecycle-isolated local llama.cpp inference,
  translation history and the personal glossary.
- `SayFlex.Ocr.exe` — CPU-only PP-OCRv5 screen-text recognition. It starts on
  first use and can be released independently.
- `SayFlex.Companion.exe` — the isolated optional Companion renderer.

When translation is enabled, its worker owns one authenticated
`llama-server.exe` sidecar from the verified optional Vulkan runtime. The
sidecar is contained by the same Windows Job Object and cannot outlive the
desktop process.

Every recording has a UUID that is carried through hotkey, audio, inference,
history, insertion and recovery. No operation reads a mutable global “last
text” value while a session is completing.

The concrete privacy and trust boundaries, including clipboard behavior,
history retention, model verification, elevated applications, and the
same-user threat model, are documented in [`SECURITY.md`](SECURITY.md).

## Windows hotkeys

- Hold `Ctrl + Alt + Space` to record; release `Space` or either required
  modifier to transcribe and insert.
- `Ctrl + Alt + V` inserts the newest dictation into the currently focused
  target. If that dictation is still processing, the request waits for that
  exact session instead of inserting an older transcript. A later cancelled
  or failed recording does not hide the newest successfully completed text.
- `Ctrl + Alt + K` captures selected text and offers explicit RU/EN keyboard
  layout candidates. Ordinary text is never changed automatically.
- `Ctrl + Alt + F` captures selected text once, translates it, and opens the
  compact preview immediately. It translates on this machine unless a
  translation service has been put above the local engine in Settings →
  Providers, which is off by default and needs a key you supply.
- `Ctrl + Alt + G` freezes the visible desktop, lets the user select a region,
  recognizes text locally and opens the screen result in ORIG, DUAL, or ON
  IMAGE mode. Escape or right-click cancels selection.

Screen translation retains only the selected crop, never a full-monitor
frame. Temporary crops are kept for up to 24 hours and 500 MB; the oldest can
be removed earlier when the size limit is reached. Pinning does not extend the
retention period. The result text and explicitly confirmed glossary entries
remain available after the temporary image expires. Double-clicking a
translated block opens its correction editor; Save or explicit Confirm writes
the personal glossary, while ordinary translation, Copy, Pin and Close do not.

Selected text is resolved through a bounded capability cascade: UI Automation
`TextPattern`, synchronous `WM_COPY` for native edit controls, then the
non-destructive Copy shortcut for the target application family. Modern
terminals generally use `Ctrl+Shift+C`; WezTerm and classic terminal clients
use the layout-independent `Ctrl+Insert`; ordinary editors use `Ctrl+C`.
Clipboard fallback accepts a result only after Unicode text is actually
readable, tolerates delayed-rendering gaps after `EmptyClipboard`, and restores
the original data object after the copy transaction becomes stable. A failed
`WM_COPY` attempt escalates to the target application's normal Copy command.
Password fields, secure desktop and higher-integrity targets fail closed.

UI Automation runs in a persistent MTA actor with a cached client and bounded
provider timeouts. Release diagnostics are written without captured text to
`%LOCALAPPDATA%\SayFlex\logs\desktop.log` and `input-host.log`, with bounded
rotation.

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

Translation offers lightweight Firefox language-pair packs and multilingual
Qwen3.5 models through a pinned llama.cpp Vulkan runtime. Translation packs are
not downloaded automatically. Users can install multiple compact language
pairs or choose a 4B Fast / 9B Quality model; the system locale only marks a
recommendation. Qwen supports structured screen blocks, automatic context and
glossary guidance. Compact packs preserve block identity but intentionally
advertise limited context/glossary capability.

Translation and layout correction can be disabled independently.
`Ctrl+Alt+F` translates the selected text immediately into a compact Acrylic
preview; `Ctrl+Alt+K` corrects keyboard layout. The translation worker can
remain warm for the lowest latency or unload after every request to reduce
memory. Voice and translation history can be disabled and cleared from
Settings.

Resources reports the aggregate SayFlex process tree and separate speech,
translation, and OCR rows. Missing Windows counters are displayed as
unavailable, not zero. Manual release never deletes installed files. OCR may
be released after five idle minutes, speech after fifteen, and translation
after fifteen when Keep warm is off; active work is never interrupted. The
selected engine loads again automatically the next time its feature is used.

The universal installer ships isolated CUDA, Vulkan and CPU speech workers.
`Auto` tries CUDA, then Vulkan, then CPU, but accepts a backend only after the
selected Whisper model completes a real warm-up inference. CUDA carries its
application-local runtime and kernels for RTX 30/40/50 generations; Vulkan is
the cross-vendor GPU fallback. Settings expose `Auto`, `CUDA`, `Vulkan` and
`CPU` and report the backend that is actually active.

Release optimization defaults to **fastest** and incrementally prepares only
new audio after each eligible natural pause. Users who prefer lower sustained
GPU and battery use can select **efficient**, which limits a recording to two
prepared segments without unloading the already warmed model.

## Build, test and package

```powershell
cd .\rust-app
cargo test --workspace
.\scripts\publish.ps1
.\scripts\build-installer.ps1 -SkipPublish
```

The measured warm-path design and benchmark gate are documented in
[`docs/LATENCY_ARCHITECTURE.md`](docs/LATENCY_ARCHITECTURE.md).
The visual tokens, adaptive popup geometry and non-activating Acrylic policy
are documented in [`docs/DESIGN_SYSTEM.md`](docs/DESIGN_SYSTEM.md).

Transient windows have isolated visual preview modes that do not register the
global hook or touch another application:

```powershell
.\target\debug\sayflex-desktop.exe --indicator-preview=mic
.\target\debug\sayflex-desktop.exe --indicator-preview=circle
.\target\debug\sayflex-desktop.exe --indicator-preview=flow-sphere
.\target\debug\sayflex-desktop.exe --indicator-preview=pulse-matrix
.\target\debug\sayflex-desktop.exe --indicator-preview=writing-slate
.\target\debug\sayflex-desktop.exe --action-preview
.\target\debug\sayflex-desktop.exe --translation-preview
.\target\debug\sayflex-desktop.exe --translation-preview=long
.\target\debug\sayflex-desktop.exe --layout-preview
```

Three controlled selection fixtures exercise UI Automation, native-message and
keyboard-shortcut capture without depending on a user's document:

```powershell
$env:SAYFLEX_RUN_SELECTION_SMOKE = '1'
$env:SAYFLEX_SELECTION_SMOKE_CLIPBOARD_MARKER = 'SAYFLEX_CLIPBOARD_SENTINEL'
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tests\fixtures\selection-uia-fixture.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tests\fixtures\selection-fixture.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tests\fixtures\selection-shortcut-fixture.ps1
Get-Content .\target\uia-smoke.out
Get-Content .\target\window-message-smoke.out
Get-Content .\target\shortcut-smoke.out
```

The same fixtures can be run as one validation gate against either a debug or
published release input host:

```powershell
.\scripts\validate-windows-integration.ps1
.\scripts\validate-windows-integration.ps1 `
  -InputHost .\dist\app\SayFlex.InputHost.exe
```

The isolated smoke modes are available in release builds only when their
explicit `SAYFLEX_RUN_*_SMOKE=1` opt-in is present. They do not install the
global hook and are not reachable from the normal application lifecycle.

The installer is emitted under `rust-app\dist`; it does not overwrite the
existing C# installer or its application directory.

## License

The workspace is MIT-licensed. Slint is consumed under its royalty-free
desktop application license. A release must retain the required Slint
attribution in the About surface or on the public download page.
