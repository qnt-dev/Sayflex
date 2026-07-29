# StyleFlex interface system

## Product character

StyleFlex is a compact system utility, not a web dashboard. Its interface is
calm, task-focused and immediately scannable:

- a persistent sidebar provides predictable navigation;
- each page owns one coherent task instead of compressing every setting into
  one dashboard;
- short labels and visible hotkeys reduce recall;
- local status appears next to the control it describes;
- motion is reserved for recording or processing.

The settings information architecture is:

```text
Overview
Speech
Text tools
Translation
History
About
```

Inactive pages are conditionally instantiated, not merely hidden. They
therefore consume no layout space and cannot push active content out of place.

## Palette

The product has four semantic color families. Dark and opacity variants are
derived from them rather than introducing unrelated hues.

| Role | Token | Value |
| --- | --- | --- |
| Base | `ink` | `#101114` |
| Sidebar | `sidebar` | `#0C0D10` |
| Content | `text` | `#F3F3F4` |
| Primary action | `orange` | `#E39952` |
| Destructive/error | `red` | `#D85C5C` |

Muted text is reserved for secondary metadata and does not carry an action by
itself. Warm orange is used for current state and primary actions, not as a
large decorative fill.

## Geometry

- application panels: 9 px corner radius;
- transient window shell: 7 px;
- buttons and rows: 6–7 px;
- borders: 1 px;
- primary content spacing: 10–18 px;
- transient window outer padding: 17–18 px.

The small radii avoid both harsh square corners and oversized mobile-style
capsules.

## Settings navigation

The 214 px sidebar is persistent and the content region is stable. Page titles
use 25 px type, body copy uses 12–13 px, and controls are arranged as
description/action rows. Pages may use remaining vertical space for their
primary list, but do not introduce a second dashboard grid.

The recording indicator is intentionally outside this theme migration. Its
approved geometry and black/yellow palette are a product invariant:

```text
112 × 42 px
graphite shell
yellow waveform and record mark
bottom-center placement
```

Settings work must not alter that component.

## Materials

The long-lived settings and translator windows use an opaque graphite base.
Transient action windows request `DWMSBT_TRANSIENTWINDOW`, which maps to
Desktop Acrylic on supported Windows 11 builds. Slint always renders a dark
translucent fallback, so disabling transparency, battery saver, high contrast
or remote rendering never exposes unreadable content.

Only one Acrylic layer is used per transient window. Cards inside the window
are tinted opaque or nearly opaque; no nested blur layers are created.

References:

- <https://learn.microsoft.com/en-us/windows/apps/design/style/acrylic>
- <https://learn.microsoft.com/en-us/windows/apps/develop/ui/system-backdrops>
- <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type>
- <https://www.w3.org/WAI/WCAG21/Understanding/contrast-minimum>

## Adaptive transient geometry

Popup height is not a fixed guess. Rust estimates wrapped line demand from
Unicode character count and explicit line breaks, clamps the result to the
window-specific minimum and maximum, and passes panel heights to Slint.
The Window height is then a declarative sum of:

```text
outer padding
+ fixed header
+ measured source panel
+ measured result or option rows
+ inter-item spacing
+ fixed action row
```

This keeps short actions compact while preventing long selections from taking
over the screen.

The translation preview also adapts its width across three density bands.
Source text is capped at three context lines. Result text grows naturally until
the card reaches its maximum height; beyond that point a `Flickable` viewport
and a two-pixel dark rail with a restrained orange thumb appear on the right.
Mouse-wheel, touchpad, and drag scrolling remain available without activating
or resizing the original editor.

## Interaction invariants

- transient windows do not activate or take editor focus;
- destructive actions are red and never share the primary orange state;
- disabled actions remain visible but cannot receive their default action;
- every button has an accessibility role and label;
- system Acrylic failure never changes functionality;
- visual preview modes do not register hotkeys, launch workers or touch another
  application.
