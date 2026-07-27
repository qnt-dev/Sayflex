# Keyboop for Windows 0.1.11

This release removes the experimental corner highlights while preserving the
two-layer indicator construction.

## Design

- Remove all four yellow corner-highlight paths.
- Keep the neutral outer frame.
- Keep the slightly inset dark body.
- Keep the narrow transparent gap between the frame and body on every edge.
- Preserve the indicator dimensions, waveform, status control, transparency,
  animation, and non-activating behavior.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 71 passed (`Core` 49, `InputReplay` 22), zero failed.
- Installer:
  `Keyboop-0.1.11-win-x64-setup.exe`, 193,242,133 bytes,
  SHA-256
  `AB29EDCD189A95438AD122D02A24BD343443F201E979D89B2721872ABFC6D027`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the release manifest by SHA-256; installed product
  version is `0.1.11`.
- Three installed-runtime IPC indicator probes passed: the indicator appeared
  in 57–117 ms and returned to its baseline state after every probe. These
  probes validate the indicator path, not the physical `Ctrl+Win` hotkey or
  microphone path.
- User visual acceptance of the no-highlight design remains to be completed.
