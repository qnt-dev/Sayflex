# Rust Voice 0.5.0

## Interface

- Replaced the compressed single-page dashboard with six focused settings
  pages and persistent sidebar navigation.
- Added dedicated Overview, Speech, Text tools, Translation, History and About
  surfaces.
- Increased text and control sizes, reduced decorative accent fills and moved
  related controls into description/action rows.
- Kept the approved black/yellow recording indicator unchanged.
- Fixed page layout by conditionally instantiating active content, so inactive
  pages cannot reserve space or push a page to the edge of the window.

## Compatibility

The release changes the Slint presentation layer only. Worker protocols,
hotkeys, insertion paths, model management and the latency-critical recording
pipeline are unchanged.
