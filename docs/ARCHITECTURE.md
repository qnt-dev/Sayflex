# Architecture

SayFlex is a Rust application for Windows (`rust-app/`). The interface uses
Slint. Every job that can block, crash or hold a large model runs in its own
worker process; the desktop process supervises them.

```text
SayFlex.exe  (desktop: Slint UI, tray, settings, overlays, supervisor)
  |  JSON lines over stdin/stdout, protocol version checked at startup
  |  (crates/protocol, PROTOCOL_VERSION)
  +-- SayFlex.InputHost.exe       hotkeys (WH_KEYBOARD_LL), text insertion,
  |                               selection capture, clipboard transactions
  +-- SayFlex.Speech.{Cuda,Vulkan,Cpu,CpuCompat}.exe
  |                               microphone (WASAPI via cpal), Silero VAD,
  |                               whisper.cpp, history, online speech providers
  +-- SayFlex.Translation.exe     compact packs (fxtranslate), llama.cpp server
  |                               for contextual models, online providers
  +-- SayFlex.Ocr.exe             PP-OCRv5 through ONNX Runtime (CPU)
  +-- SayFlex.Companion.exe       optional desktop companion visualizer
```

All workers run inside one Windows Job Object, so they end with the desktop.
A worker that exits is restarted by the supervisor; a protocol mismatch stops
the worker and is shown in Settings.

## Dictation

1. InputHost sees the dictation chord and captures the target: the foreground
   top-level window, its process and thread, the keyboard layout and the
   pointer. Password fields are refused.
2. The speech worker records while the key is held. During pauses it may
   decode the stable part early (speculation) so the final pass is short.
3. On release the final audio is resampled to 16 kHz. **Silero VAD** decides
   whether it contains speech; without speech nothing is decoded and nothing is
   inserted. Whisper decodes the rest. A segment with OpenAI's no-speech
   signature (no-speech probability > 0.6 and average log-probability < -1) is
   dropped. With *Auto*, the language is chosen among the user's Windows
   languages (display languages and keyboard layouts).
4. The desktop sends the transcript to InputHost. InputHost checks that the
   same top-level window is in front (a short grace period covers a tooltip or
   notification taking the foreground) and delivers the text by the first
   route that applies:
   - WezTerm: its CLI;
   - standard Win32 Edit/RichEdit controls: `EM_REPLACESEL`;
   - with *Insertion → Type the text*: Unicode keystrokes (`SendInput`);
   - otherwise a clipboard transaction: the previous clipboard is saved, the
     text is offered with delayed rendering and privacy formats, the paste
     shortcut of that application is sent, and the previous clipboard is
     restored after the target read it or after 2.5 s.
5. As soon as the paste reaches the target, InputHost reports
   `InsertionDispatched` and the visualizer disappears. A failed insertion
   shows a notice with **Insert** (into the window now in front) and **Copy**.

The text is never inserted into another window on SayFlex's own initiative.

## Translation

The text path translates selected text with the local engine or, when the user
configured one, an online provider. The local engine checks that numbers,
dates, times and URLs survived; if not, it translates again with each value
replaced by a marker (`[1]`, `[2]`, …) and restores the values. What still
differs is flagged in the result ("check the numbers").

Compact packs (Firefox Translations models, MPL-2.0) run in-process through
`fxtranslate`; its AVX2 kernel is used only on processors that have AVX2.
Contextual models run in `llama-server` on the loopback interface with a random
API key; the worker verifies that the listening port belongs to its own child
process. An idle contextual model is released after ten minutes.

## Screen translation

The desktop captures the selected area, the OCR worker finds text lines and
reads each with the language family that reads it best among the user's
languages, fixes look-alike Latin/Cyrillic letters, and retries weak vertical
lines upside down. The desktop rebuilds rows and paragraphs
(`crates/ocr-engine/src/reconstruct.rs`), translates the blocks and draws them
over a repaired copy of the image.

## Models and runtimes

`crates/model-manager` downloads every artifact over HTTPS, resumes partial
downloads, and verifies size and SHA-256 before a file is used. Data lives in
`%LOCALAPPDATA%\SayFlex` (see `rust-app/PRIVACY.md`).

The installer contains no NVIDIA files. The CUDA worker links `cudart64_13.dll`
dynamically; the NVIDIA runtime (cuBLAS and cudart from CUDA 13.3.1) is
downloaded inside the app only after the user accepts the NVIDIA EULA
(`crates/model-manager/src/cuda_runtime.rs`).

## Processor support

The speech, translation and Vulkan/CUDA workers are built for x86-64-v3 (AVX2,
FMA, F16C, BMI2). `SayFlex.Speech.CpuCompat.exe` is built for SSE4.2; the
desktop selects it, and only it, on processors without AVX2. Setup refuses a
processor without SSE4.2.

## Building and releasing

`rust-app/scripts/build-installer.ps1` builds every worker, stages the OCR and
VAD models, the Visual C++ runtime and the notices, validates the payload
(`validate-release.ps1`) and compiles the Inno Setup installer
(`rust-app/installer/SayFlex.iss`). `.github/workflows/rust.yml` runs format,
lint and tests on every push.
