# Product specification

Status: current implementation baseline

## Product intent

Build a Windows-first, local-first typing assistant with the useful behavior of
Keyboop and Wispr Flow without requiring a Mac, a cloud subscription, or visual
parity with either product.

Typed text and microphone audio stay on the machine by default. Network access is
allowed only for explicit model/update downloads or a future user-enabled remote
provider.

## Release-one capabilities

### Layout correction

- Manual-first is the default and supported product mode.
- The user explicitly selects text and presses `Ctrl+Alt+K`.
- A local preview shows deterministic RU/EN physical-layout alternatives. Nothing is
  changed until the user chooses a candidate with Enter, Space, or its number.
- Escape cancels without editing the target.
- With automatic correction disabled, ordinary typed text is not resolved, buffered,
  or modified by Keyboop.
- Automatic correction code is not exposed in the product UI and remains disabled.
  It can only return after separate compatibility and false-positive gates are
  independently satisfied.
- Passwords, URLs, code, commands, invented words, transliteration, and deliberate
  wrong-layout strings are therefore never guessed at in the default mode.
- Future smart ranking may order manual candidates, but low confidence must never
  trigger an edit.

### Input reliability

- The low-level keyboard hook runs on a dedicated thread and does no file, database,
  network, UI, or model work.
- Own synthetic events carry a per-process marker.
- Global chords require their exact physical modifier state at trigger time.
  Side-specific Ctrl, Shift, and Alt releases must clear the corresponding
  aggregate state; remembered modifier state alone never authorizes a hotkey.
- A bounded replacement transaction verifies foreground window, thread, layout, and
  focus before committing.
- Timeout, focus change, layout change, queue overflow, unsupported field, or worker
  failure returns original input in order.
- Elevated and protected targets are reported as unsupported; the application never
  elevates itself to bypass UIPI.

### Real-time dictation

- Push-to-talk using the Windows-native `Ctrl+Win` chord. Releasing either key
  ends the session; a physical-key watchdog closes a session if a key-up event
  is missed.
- WASAPI capture, 16 kHz mono normalization, an energy gate, and stable final
  utterances.
- Default quality profile for the development machine: OpenAI Whisper
  `large-v3-turbo`, CUDA, RTX 3080 Ti, persistent loaded model.
- CPU fallback is available but not the preferred profile.
- Quiet recordings and punctuation-only Whisper hallucinations produce no insertion.
- A non-activating dark-and-signal-yellow waveform confirms global listening and
  processing states on the monitor containing the foreground application. Its
  `#272727` background, `#2C2F31` frame, size, colour system, and waveform remain
  continuous across the transition. The idle status probe is fast enough for the
  indicator to appear perceptually immediately after dictation begins.
- Capture streams to a temporary local WAV file instead of an unbounded RAM
  buffer. A warning appears at 19 minutes and recording stops at 20 minutes.
- Final text uses a guarded clipboard-paste transaction with Unicode input as a
  fallback.
- `Shift+Alt+X` is recognized from tracked physical modifier events and triggers
  only after Shift, Alt, and X are all released. Every recording has an immutable
  session ID. Recovery waits for that exact session's idempotent finalization and
  leaves its transcript in the clipboard; it never resolves a pending recording
  through a mutable global “last transcript”. The user then pastes with
  `Shift+Insert` in WezTerm or `Ctrl+V` elsewhere.
- Optional local history stores at most 200 successful text transcripts before
  insertion is attempted. It survives restarts and supports inspect, copy,
  delete, and clear operations. It never stores raw audio.
- Speech worker failure cannot break ordinary keyboard input.
- Temporary raw audio is deleted after it is converted for local recognition.

### Selected text and translation

- The current compatibility path reads selection through a guarded clipboard
  transaction after the global hotkey has been released.
- UI Automation reading can be added where it is measurably more reliable, but is not
  treated as a universal editing API.
- Restore clipboard best-effort only if its sequence still belongs to the operation.
- Local RU/EN translation is replaceable behind the worker protocol.

### Desktop experience

- Tray status, pause/resume, current mode, microphone state, and quick actions.
- A product-owned waveform icon is embedded in the executable and installer and
  is also used by the tray icon.
- Settings for hotkeys, correction modes, applications, language, microphone,
  models, history, privacy, and diagnostics.
- Portable build and an Inno Setup installer with optional autostart.

## Non-goals for the first stable release

- Pixel parity with macOS Keyboop or Wispr Flow.
- An LLM in the keyboard hot path.
- Universal editing through TSF.
- Secure-desktop, UAC-screen, anti-cheat, game, or elevated-process injection.
- Silent public distribution without Store signing or a paid public certificate.

## Acceptance targets

- Detector golden corpus passes deterministically.
- Manual layout preview maps RU→EN and EN→RU while preserving case, punctuation, and
  multi-word selections.
- Apply replaces only the explicit selection; Escape leaves it unchanged.
- Default-mode typing and all boundary keys pass through unchanged.
- Replay tests show no lost, duplicated, or reordered physical events.
- Input transaction remains bounded and fail-open under injected delays and crashes.
- Notepad, Windows text controls, Chromium, Electron, VS Code, Telegram, and Windows
  Terminal have recorded compatibility results.
- Ordinary detector decisions complete without model inference or IPC.
- Dictation reaches stable final text faster than real time on the target RTX 3080 Ti.
- No typed text or raw audio appears in normal logs or crash metadata.
