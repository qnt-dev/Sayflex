# Keyboop for Windows 0.1.14

This release fixes stale transcript recovery when `Shift+Alt+X` is pressed
immediately after dictation.

## Root cause

Speech finalization runs asynchronously so ordinary input remains responsive.
The recovery action could request `GetLastTranscript` while the immediately
preceding recording was still being recognized. SpeechWorker then correctly
returned the previous completed transcript, which became stale as soon as the
new recognition finished.

## Changes

- Track every pending speech-finalization operation.
- Make recovery wait until all earlier finalizations have completed.
- Request the last transcript only after that synchronization point.
- Preserve clipboard-only recovery: `Shift+Alt+X`, then `Shift+Insert` in
  WezTerm or `Ctrl+V` elsewhere.
- Add regression tests for waiting, concurrent additions, and already-completed
  operations.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification passed.
- Automated tests: 80 passed (`Core` 49, `InputReplay` 31), zero failed.
- Installer:
  `Keyboop-0.1.14-win-x64-setup.exe`, 193,280,418 bytes,
  SHA-256
  `D751243AD7522D1CE4878F560D4F07F2E203879D4B9E580A5C0DFE12A41AED22`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.14`.
- An installed hook-level recovery probe replaced a sentinel clipboard value
  with the exact newest history entry. No transcript text was emitted into
  diagnostics.

The automated operation-tracker tests cover the original immediate-recovery
race. Physical user acceptance after a real dictation remains the final
interaction check.
