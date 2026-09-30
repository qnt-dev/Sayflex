# SayFlex 0.8.7

## Screen translation image fidelity

- Screen translation results now use a dedicated frameless window for their
  entire lifetime. Dragging never enters the Windows caption move loop, and
  PIN changes only z-order without replacing the window.
- OCR semantic context is separate from visual row geometry. Baseline-aligned
  fragments form stable rows for translation while retaining every individual
  source rectangle used for image replacement.
- The image view has no external translation column. It measures the rendered
  text at 14, 13, and 12 logical pixels and places complete translations within
  the scaled image only when they fit without colliding with another OCR row.
- Only source fragments belonging to a successfully placed translation are
  masked. Standalone values such as `45` and `275`, icons, unrelated labels,
  and all other original pixels remain visible.
- A row that cannot be placed readably keeps its original pixels and is marked
  as an exceptional editable row; text is never silently clipped or shrunk
  below 12 logical pixels.
- Double-click correction, glossary save/confirm, ORIG, DUAL, ON IMAGE, file
  save, image deletion, PIN, and close remain part of the dedicated result.

## Reliability included in this candidate

- The first completed screenshot result, closing its window, and reopening a
  saved result no longer overlap read and write borrows of the window registry.
  The two transitions have regression tests that reproduced the original panic.
- The recording visual is dismissed when recognition finishes. Clipboard paste
  acknowledgement may continue afterward without keeping the speech visual on
  screen; delivery remains tracked until the input host responds.
- Screen rows retain stable identifiers through OCR, translation, persistence,
  correction, and late worker responses.
- Mixed-script malformed target text receives one bounded retry and cannot be
  presented as a completed image replacement after the retry fails.
- Result positioning uses the selected monitor's logical work area and current
  effective DPI while preserving the screenshot aspect ratio.

## Verification status

### Verified in the source tree

- Desktop tests with screen translation enabled: 333 passed, 0 failed.
- Strict Clippy passed for the OCR engine, protocol, translation engine,
  Windows platform layer, translation worker, and desktop with warnings denied.
- The retained Nimbus crop completed the automated production-view renderer
  path. This is structural evidence only and is not visual acceptance.

### Pending physical installed-UI acceptance

- Complete Nimbus ON IMAGE result.
- Borderless appearance while the window is held and moved.
- Embedded editor after a double-click.
- PIN without a flash or second surface.
- Visibility and correct placement of `45`, `275`, icons, related labels, and
  every fully placed translation.

These checks must be performed by the user on the installed artifact. They are
not inferred from unit tests or an automated screenshot.

Public distribution still requires the separate Authenticode/SignPath step.
