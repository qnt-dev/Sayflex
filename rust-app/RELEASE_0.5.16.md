# SayFlex 0.5.16

This release promotes the voice-reactive Writing Slate visualizer from an
isolated design preview into the production recording experience.

## Recording visualizers

- Writing Slate is selectable and persisted from the recording settings page;
- its compact vertical tablet uses a uniform graphite bezel and transparent
  surrounding window;
- red, yellow and warm-white strokes fill in response to live microphone
  energy, pause with speech and scroll after the usable screen area fills;
- the balanced cadence avoids both the original sluggish motion and excessive
  multi-stroke bursts for a single word;
- visualizer selection now updates the native overlay type before the next
  indicator is shown.

Flow Sphere also remains available with its Reference and Black + Yellow
palettes.

## Validation

The release gate includes workspace tests, Clippy with warnings denied, the
release publisher, release file-set and SHA-256 validation, installer creation,
and a live microphone smoke test of the packaged desktop application.
