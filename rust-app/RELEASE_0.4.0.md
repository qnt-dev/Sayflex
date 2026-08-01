# SayFlex 0.4.0

This release establishes the first coherent Rust/Slint product surface.

## Interface

- settings, models, selected-text actions and recent history now share one
  compact dashboard instead of three navigation tabs;
- the palette uses graphite, warm white, orange and red semantic tokens;
- corners, borders, spacing and button states use one restrained geometry
  system;
- empty worker states are explicit instead of rendering blank panels;
- the translator, recording indicator, layout chooser, action palette and
  translation preview use the same visual language.

## Transient surfaces

- action windows use the Windows 11 transient system backdrop where available;
- the dark translucent Slint surface remains the deterministic fallback;
- popup height is derived from measured text demand and capped for safety;
- fixed empty canvases were removed from action, layout and translation
  windows;
- isolated preview modes make visual QA reproducible without a global hotkey or
  user document.

## Safety retained from 0.3.1

- `Ctrl+Shift+C` remains the selected-text copy shortcut for WezTerm;
- no synthetic `Ctrl+C` is sent to WezTerm;
- action windows remain non-activating and preserve the original target;
- the C# application remains unchanged and independent.
