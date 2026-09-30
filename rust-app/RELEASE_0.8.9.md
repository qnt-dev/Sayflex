# SayFlex 0.8.9

## Screen translation candidate

- Keep translated OCR rows at a readable font size. If a translation cannot
  fit in its source row, place it in a callout on a uniform, unoccupied dark
  part of the captured image or below the image in a scrollable result.
- Draw a persistent line from every moved translation to the exact source row,
  and outline that source. Preserve icons, numbers and unrelated pixels.
- Leave native window decoration styles to Slint/winit instead of rewriting
  them during screen-result presentation. This targets the transient system
  title strip seen while dragging the frameless result window.

## Verification boundary

Automated tests cover geometry, untouched image details, rendered connector
pixels, and native window-style ownership. The installed appearance, movement,
PIN, editing and visual clarity still require the user's hands-on check with
screenshots. This local candidate is not a signed public release.

Earlier 0.8.8 changes and their separate verification status are documented
in [RELEASE_0.8.8.md](RELEASE_0.8.8.md).
