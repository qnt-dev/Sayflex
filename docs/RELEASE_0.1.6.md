# Keyboop for Windows 0.1.6

This release fixes modifier-state corruption that could turn ordinary keys into
dictation or repeat-last hotkeys.

## Root cause

The keyboard-state mirror calculated each aggregate modifier from the left key,
right key, and its own previous aggregate value. After a side-specific Ctrl,
Shift, or Alt key was released, the previous aggregate value could keep that
modifier latched indefinitely.

Consequences included:

- a later standalone Windows key being misread as `Ctrl+Win`;
- a later ordinary `X` being misread as `Shift+Alt+X`;
- the latter requesting and inserting the previous successful transcript.

## Changes

- Aggregate modifiers are now derived only from the current left and right key
  states.
- Dictation, repeat-last, and pause require the exact current physical modifier
  combination in addition to the event tracker.
- Dictation release accepts generic, left, or right Ctrl.
- The push-to-talk chord accepts either Windows key.
- Right Alt is now included consistently in shortcut-modifier detection.
- Added regression coverage for all six side-specific Ctrl, Shift, and Alt
  press/release cycles, standalone Windows, exact `Ctrl+Win`, and ordinary `X`.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 67 passed (`Core` 49, `InputReplay` 18), zero failed.
- Installer:
  `Keyboop-0.1.6-win-x64-setup.exe`, 193,272,353 bytes,
  SHA-256
  `D054127D97DC4325E84B0ACA319421BBE4FCE7982B353FBC85F16A89C96A0A34`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.6`.
- Physical hotkey behavior still requires live user acceptance after installation.
