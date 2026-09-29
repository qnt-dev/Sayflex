# SayFlex 0.8.6

## Screen translation image fidelity

- OCR paragraph reconstruction and image replacement geometry are now separate.
  The translation model still receives the complete reading context, while the
  image view retains every detected row's own crop-local rectangle and stable
  identifier.
- The image view masks only text that is actually replaced. Standalone numeric
  values and unchanged elements remain part of the original image instead of
  being covered by a grouped paragraph background.
- A translation that cannot fit in its detected text rectangle is rendered in
  a scrollable area beside the image. It is never moved over an unrelated icon,
  number, illustration, or other part of the screenshot.
- Replacement masks are opaque only inside the detected text rectangle, so the
  original glyphs cannot bleed through while untouched pixels remain visible.
- The crop and its OCR coordinates share the same explicit origin when the
  side area is present. This prevents the panel and replacements from shifting
  back over the image.

## Window interaction

- The permanent editing hint was removed from the result header. Double-click
  editing remains available on each translated element.
- Screen-result windows use client-owned borderless dragging instead of a
  native `HTCAPTION` move loop. Windows therefore does not paint a transient
  system title bar over the SayFlex header during the gesture.

## Verification status

### Verified in the source tree

- OCR reconstruction tests: 13 passed.
- Windows platform tests: 43 passed, 2 interactive-only tests ignored; capture
  geometry tests: 9 passed, 1 interactive-only test ignored.
- Desktop tests with screen translation enabled: 324 passed.
- Strict Clippy for the OCR engine, Windows platform, and desktop completed
  with warnings denied.
- The retained Nimbus capture was decoded by the shipped OCR worker into 16
  crop-local rows. A production-view render kept the icon values `45` and
  `275` in place and reserved overflow space outside the image.

### Pending installed-artifact evidence

- Installer identity, checksum, in-place installation, installed file set, and
  live drag/PIN/edit/close checks are recorded after the final installer is
  built and installed. They are not claimed by the source-tree checks above.

Public distribution still requires the separate Authenticode/SignPath step.
