# Keyboop for Windows 0.1.12

This release makes the indicator's transparent inset geometrically symmetric.

## Design

- Replace the fractional 2.6-DIP body margin with an integral 3-DIP inset.
- Match the body corner radius to the new inset.
- Preserve the neutral outer frame, transparent gap, dark body, dimensions,
  waveform, status control, animation, transparency, and non-activating
  behavior.
- Keep the left-side spacing accepted during live review and make the right and
  bottom spacing identical.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 71 passed (`Core` 49, `InputReplay` 22), zero failed.
- Installer:
  `Keyboop-0.1.12-win-x64-setup.exe`, 193,235,676 bytes,
  SHA-256
  `B0904260854CD1250A4948C738EEA3722F6B8B9EE6771C85211AA4FC53607145`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the release manifest by SHA-256; installed product
  version is `0.1.12`.
- Installed-runtime inspection passed on contrasting desktop surfaces. At the
  horizontal and vertical center lines, the frame occupies the outside pixel,
  the transparent gap occupies exactly two physical pixels, and the body starts
  at the third pixel on all four sides:
  `x 0 | 1-2 | 3-108 | 109-110 | 111` and
  `y 0 | 1-2 | 3-38 | 39-40 | 41`.
- The final installed-runtime indicator probe appeared in 70 ms and returned to
  its baseline state. This validates the IPC indicator path, not the physical
  hotkey or microphone path.
- User visual acceptance remains to be completed.
