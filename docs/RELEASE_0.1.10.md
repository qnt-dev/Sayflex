# Keyboop for Windows 0.1.10

This release keeps the inset-body and transparent-gap construction from 0.1.9
but changes the corner highlights after live design review.

## Design

- Keep the neutral outer frame and slightly inset dark body.
- Keep the gap transparent along every straight edge.
- Replace the light-grey corner highlights with the waveform's `#FFD400`.
- Reduce the corner-highlight stroke from 0.55 DIP to 0.35 DIP.
- Preserve the short inner-radius geometry without extending onto straight
  horizontal or vertical edges.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 71 passed (`Core` 49, `InputReplay` 22), zero failed.
- Installer:
  `Keyboop-0.1.10-win-x64-setup.exe`, 193,241,178 bytes,
  SHA-256
  `34FFE8EBA81F6340B5A3D8698B28502DFB49628400D94AF666B7E080B6ADBC1C`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.10`.
- Installed-runtime indicator probes passed. After the warm-up probe, the
  indicator appeared in 32–33 ms and returned to its baseline state.
- User acceptance of the final corner-highlight balance remains to be
  completed.
