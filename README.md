# Keyboop for Windows

Local-first Windows keyboard-layout correction, explicit selection conversion,
and real-time dictation.

The project is an independent Windows implementation inspired by the MIT-licensed
Keyboop project for macOS. Visual parity is not a goal. Reliability, privacy, and
native Windows behavior take precedence.

## Product direction

- Manual-first RU/EN layout correction: select text, press `Ctrl+Alt+K`, inspect the
  local preview, and explicitly apply or cancel.
- Ordinary typing is not inspected, buffered, or automatically modified.
- Local real-time dictation through a persistent CUDA speech worker.
- Selected-text conversion preserves case, whitespace, and punctuation.
- WPF tray application, model manager, microphone test, and privacy-safe diagnostics.
- Portable and per-user Inno Setup distributions. Public code signing is
  intentionally not required for development or private use.

Snippets, local RU/EN translation, application profiles, and updates remain
planned capabilities rather than claims of the current `0.1.16` build. Optional
bounded local dictation history and recovery of the most recent transcript are
implemented.

## Install and use

Run `dist\Keyboop-0.1.16-win-x64-setup.exe`. Installation is per-user and does not
require administrator rights.

- Layout: select text, press `Ctrl+Alt+K`, inspect the preview, then press Enter.
- Dictation (hold): `Ctrl+Win`; release both keys to transcribe and insert.
- Recover the last successful transcript: release `Shift+Alt+X`. Keyboop copies
  the text to the clipboard. Paste it with `Shift+Insert` in WezTerm or `Ctrl+V`
  in ordinary applications.
- Pause/resume input features: `Ctrl+Alt+P`.

Start dictation while the caret is in the field that should receive the text. A
compact dark-and-yellow waveform confirms that the global hotkey reached
Keyboop. Its `#272727` background, subtly lighter `#2C2F31` frame, and moderately
rounded corners match the desktop palette without looking like a pill. A narrow
transparent gap separates the outer frame from the inset dark body and reveals
the surface behind the indicator without additional corner accents. Its integral
3-DIP inset keeps that gap geometrically equal on all four sides under WPF layout
rounding. The same
waveform smoothly settles into its processing motion after the keys are released,
without changing size or visual language. Each recording is
streamed to a temporary local WAV file instead of being accumulated in RAM. A
warning appears at 19 minutes and the recording stops at 20 minutes.

Dictation insertion uses a guarded clipboard transaction for compatibility with
web and Electron editors. Keyboop restores the previous clipboard only if no
other application changed it during the operation, and falls back to Unicode
input if the clipboard path is unavailable.

Dictation history is an explicit setting. When enabled, up to 200 successful
transcripts are stored locally in `%LOCALAPPDATA%\Keyboop\dictation-history.json`
before insertion is attempted. Raw audio is never retained. The settings window
can inspect, copy, delete, or clear these entries.

Every recording has an immutable session ID shared by capture, finalization,
history, insertion, and recovery. `Shift+Alt+X` waits for that exact session
instead of reading a mutable global “last transcript”, so overlapping processing
and reversed request timing cannot return the previous recording.

The selected Whisper model is downloaded separately to
`%LOCALAPPDATA%\Keyboop\models` and is verified by size and SHA-256. On the target
RTX 3080 Ti machine, Keyboop preloads it through CUDA so the first dictated phrase
does not pay the model-loading delay.

Wispr Flow and Keyboop must not run with the same `Ctrl+Win` hotkey at the same
time. Exit one application before testing the other.

## Solution

- `src/Keyboop.Core` — deterministic detector and input-domain state machines.
- `src/Keyboop.Protocol` — versioned IPC messages.
- `src/Keyboop.InputHost` — isolated Windows hook and replacement transaction.
- `src/Keyboop.UI` — .NET 10 WPF tray and settings application.
- `src/Keyboop.SpeechWorker` — persistent local ASR process.
- `tests/Keyboop.Core.Tests` — detector and language-data golden tests.
- `tests/Keyboop.InputReplay.Tests` — keyboard event/replacement replay tests.

See [`docs/PRODUCT_SPEC.md`](docs/PRODUCT_SPEC.md) and
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).
