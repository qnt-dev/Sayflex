# Keyboop for Windows 0.1.8

This release refines the installed dictation indicator for mixed light and dark
desktop backgrounds.

## Design

- Keep the dark `#272727` body and subtle `#2C2F31` neutral frame.
- Add four short `#FFD400` arcs that follow the existing 10 DIP corner radius.
- Use a 1.35 DIP stroke at 82% opacity so the corners read as a controlled
  black-and-yellow accent rather than a full warning border.
- Preserve the indicator dimensions, waveform, status control, transparency,
  animation, and non-activating behavior.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 71 passed (`Core` 49, `InputReplay` 22), zero failed.
- Installer:
  `Keyboop-0.1.8-win-x64-setup.exe`, 193,289,178 bytes,
  SHA-256
  `3B82D6AF115381F2855A9FA58736FA1770A0E63CB3C72681C29DE76058BCCF23`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.8`.
- Installed-runtime visual inspection confirmed that the four accents follow
  the corner radius, remain separate from the neutral frame, and do not obscure
  the waveform or status control.
- The installed indicator appeared in 76 ms and returned to its baseline state.
- User acceptance of the corner-accent balance remains to be completed.
