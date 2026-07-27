# Rust Voice 0.5.2

## Long-dictation latency

- Natural-pause preparation is incremental: completed source ranges are never
  re-decoded by later previews.
- Release processes only the remaining tail and falls back to one authoritative
  full pass when segmentation cannot be trusted.
- Snapshot copies contain only new audio, and only one inference segment can be
  in flight.

## Recording indicator

- Three persisted black-and-yellow visualizers are available: Voice Orb,
  Symmetric Spectrum, and Fluid Ribbon.
- The preview harness can render every style independently without registering
  global hotkeys.

## Selected-text translation

- `Ctrl+Alt+T` translates immediately without opening the command palette.
- The focus-safe Windows 11 Acrylic card sizes itself to short results.
- Long results remain in a capped card with mouse-wheel, touchpad, and drag
  scrolling plus a minimal dark/orange scrollbar.
