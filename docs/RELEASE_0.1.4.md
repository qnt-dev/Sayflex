# Keyboop for Windows 0.1.4

This release unifies the dictation indicator requested during live visual
acceptance.

## Changes

- Replaced the pink and purple accents with a black-and-signal-yellow system.
- Listening and processing now use the same 112×42 capsule, border, waveform,
  and status disc.
- Releasing `Ctrl+Win` no longer shrinks the indicator or swaps to a second
  visual style.
- The live microphone waveform smoothly decays into a slower processing pulse.
- Microphone levels use a square-root response curve so quiet and normal speech
  remain visibly responsive.
- The indicator remains topmost, click-through, and non-activating.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 58 passed (`Core` 49, `InputReplay` 9), zero failed.
- Listening state rendered as a black 112×42 capsule with a yellow border,
  yellow waveform, and yellow status disc.
- The indicator remained on the active monitor without taking foreground focus.
- Installer:
  `Keyboop-0.1.4-win-x64-setup.exe`, 193,274,847 bytes,
  SHA-256
  `394E52AFA455F77134AACEC535246421F1F1999AF8768B35E0C703B81384234C`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.4`.
