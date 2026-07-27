# Keyboop for Windows 0.1.2

Installer:

`C:\PRDJKT\keyboop\dist\Keyboop-0.1.2-win-x64-setup.exe`

- Size: `193278112` bytes (`184.3 MiB`)
- SHA-256:
  `738011907BD871D3345D07C02E77A1FF19FA87FB4464E707A8F86D93FC09B034`
- Installation scope: current user; administrator rights are not required.
- Code signing: unsigned.

## Dictation and insertion changes

- Hold left Ctrl + left Windows to record; releasing either key finalizes locally
  and inserts the accepted transcript.
- A physical Ctrl watchdog closes a session if the hook misses a release event.
- The suppressed Windows-key release has a bounded cleanup path.
- Control actions use a lossless queue so the stop action cannot be dropped.
- Capture is streamed to a temporary WAV file instead of unbounded memory.
- A warning appears at 19 minutes and capture stops at 20 minutes.
- Final insertion uses a guarded clipboard transaction with Unicode fallback.
- Clipboard formats are materialized before replacement and restored only if another
  application has not changed the sequence.
- The target window and focus are revalidated before insertion.
- `Shift+Alt+Z` repeats the last accepted transcript.

## Validation

- Installed `Keyboop.UI.exe`: ProductVersion `0.1.2`, FileVersion `0.1.2.0`.
- Synthetic Ctrl+Win hold/release reached `Listening` and returned to `Ready`.
- A 6.94-second Russian fixture transcribed exactly on CUDA in about 0.31 seconds:
  `Привет, мир! Это проверка локального голосового ввода на компьютере.`
- Notepad received an exact test transcript through the clipboard transaction.
- The original clipboard sentinel was restored exactly.
- A foreground change prevented insertion into the stale target.
- Temporary capture files were absent after finalization.

- Release build: 0 warnings, 0 errors.
- Automated tests: 51 passed, 0 failed.
- `dotnet format --verify-no-changes`: passed.
- Distribution manifest: 307 files, 0 missing/size/hash mismatches.
- Installed copy comparison: 307 files, 0 missing/size/hash mismatches.

## Remaining acceptance

- A human microphone test with the installed build is still required.
- `Shift+Alt+Z` still needs a clean user-level acceptance pass with Wispr Flow fully
  exited.
- Word, Chromium/Electron, VS Code, Telegram, Windows Terminal, RDP, rich-text, and
  elevated-target compatibility remain unverified.
- Wispr Flow and Keyboop cannot both own Ctrl+Win. Exit one before running the other.
