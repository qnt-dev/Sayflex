# Keyboop for Windows 0.1.3

This release addresses live user acceptance failures found in `0.1.2`.

## Changes

- The repeat-last hotkey is corrected to `Shift+Alt+X`.
- The dictation chord accepts the generic Windows Control state as well as the
  left-specific virtual key, matching how low-level hooks report the key on
  different keyboards and injection paths.
- Repeat-last now asks SpeechWorker for its last successful live transcript.
  Generic `InsertText` tests can no longer replace that value.
- Dictation finalization runs independently from the next start request. A new
  recording may begin after the previous capture closes, while its Whisper
  recognition continues.
- SpeechWorker remembers successful transcripts by recording sequence, so
  concurrent completion cannot move "last" backwards.
- Dictation target matching uses the stable top-level foreground window instead
  of a transient child focus handle or keyboard-layout value.
- WezTerm uses direct Unicode insertion, the compatibility path already verified
  before clipboard-first insertion was introduced.
- The dictation overlay is a compact bottom-centred capsule with a live
  ten-bar microphone waveform and a separate processing state.
- The overlay receives `WS_EX_NOACTIVATE`, tool-window, and click-through styles
  so showing it cannot take focus from the target editor or terminal.
- Speech status includes a privacy-safe normalized microphone level; audio and
  transcript contents are not exposed in status polling.

## Acceptance boundary

Verified on 2026-07-24:

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 58 passed (`Core` 49, `InputReplay` 9), zero failed.
- Synthetic `Ctrl+Win` hold: SpeechWorker reported
  `Ready → Listening → Ready` with live audio-level updates.
- Two consecutive hold/release cycles both entered `Listening` without changing
  or re-clicking the foreground window.
- Listening overlay: 112×42 at the bottom centre; the foreground window remained
  active while the overlay was visible.
- WezTerm routing: direct-Unicode policy is covered by case-insensitive automated
  tests; other applications retain clipboard-first fallback.
- Fresh installed SpeechWorker returned an empty last transcript, proving that
  generic insertion probes and prior process state do not seed repeat-last.
- Installer:
  `Keyboop-0.1.3-win-x64-setup.exe`, 193,275,848 bytes,
  SHA-256
  `220E54C3199F03DDB5457E60234AC971756D9F564CA49862A6895C7ABDEDC49E`.
- Silent per-user install: exit code 0. All 307 installed product files matched
  the published files byte-for-byte by SHA-256. Installed product version:
  `0.1.3`.

Still requires the user's live acceptance:

- dictate a real phrase, release `Ctrl+Win`, and confirm insertion;
- start another phrase immediately without re-clicking the target;
- press `Shift+Alt+X` and confirm the latest successful phrase is repeated;
- repeat the insertion check in WezTerm.
