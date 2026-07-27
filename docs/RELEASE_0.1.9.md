# Keyboop for Windows 0.1.9

This release replaces the prominent yellow corner brackets from 0.1.8 with a
subtle inner-edge treatment.

## Design

- Remove the four outer yellow corner accents.
- Separate the neutral outer frame from a slightly inset dark body.
- Leave the narrow space between them transparent along every straight edge.
- Add four short light-grey highlights only within that space at the corners.
- Follow the inset body's radius without extending onto the straight horizontal
  or vertical edges.
- Use a 0.55 DIP stroke at 72% opacity so the highlight stays below a physical
  pixel at common display scales.
- Preserve the indicator dimensions, waveform, status control, transparency,
  animation, and non-activating behavior.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification: passed.
- Automated tests: 71 passed (`Core` 49, `InputReplay` 22), zero failed.
- Package, installed-runtime visual inspection, and user acceptance remain to
  be completed.
