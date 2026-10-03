# SayFlex 0.8.11

Follow-up to 0.8.10 after it was installed and used: the settings navigation
in a short window, and how screen translations look on the picture, compared
against a dota2.com capture and the original page. Plan and status:
[docs/superpowers/plans/2026-10-02-sayflex-0.8.10-review-fixes.md](docs/superpowers/plans/2026-10-02-sayflex-0.8.10-review-fixes.md)
(sections 8 and 9).

## Settings

- The page list on the left scrolls with the mouse wheel when the window is
  too short to show every page; About and the pages below it are reachable
  again.

## Screen translation: what is read

- The text detector works at up to 2560 px instead of 960. Small,
  letter-spaced captions that were missed are found (on the dota2.com capture:
  LATEST NEWS, VIEW ALL, DOWNLOAD ON STEAM, and a whole date where only
  "AUGUST" was read). It costs about 0.25 s per capture on the build machine.
- Text on one line joins into one row only across a gap of at most half its
  height. Titles of two neighbouring cards, and a caption with the link beside
  it, were joined and translated as one phrase.

## Screen translation: how it looks

- ON IMAGE draws each translation straight onto the picture. The source text
  under it is rebuilt from the background around it; there are no plates.
  ORIG still shows the capture as it was.
- Each translation takes its look from the source letters in the capture: the
  capital or lowercase height sets the size, the stroke width the weight, the
  text keeps its colour where that stays readable, and the first line sits on
  the source's baseline. A label is as wide as its text.
- A longer translation steps down in size (to 70 %) before it wraps at the
  source's width, and may run into plain background beside the source; it
  stops at a card or button edge and never covers the next row.
- Text set in capitals is translated in capitals; Cyrillic text gets «».
- Quotes, a closing dot or a descender just outside the OCR box are cleared
  with the text instead of being left behind. Text the OCR did not return on
  the same line stays visible and is not drawn over.
- DUAL shows source and translation on a plate of the background's colour,
  starting at the top of the source line; a following row moves down rather
  than hide it.

## Known limits

- Text the OCR does not return stays in the original language (on the
  dota2.com capture: "27, 2026" after "AUGUST" on a busy photo).
- The rebuilt background is smooth. On busy textures (confetti, foliage) the
  rebuilt strip can be seen as a softer area.
- Each visual row is translated on its own, so a title on two lines is two
  translations.
- In DUAL the plates cover what lies under them.

## Verification boundary

Automated: workspace `cargo fmt`, Clippy `-D warnings`, `cargo test`
(including new tests for glyph measurement, background rebuild, row joining
and the settings navigation), `cargo deny check`, the 8-locale UI translation
check, and the opt-in OCR probe on the real dota2.com capture. Offscreen
renders of four captures (dota2.com, the Dota 2 Zeus page, a Nimbus card, a
light settings page) in ON IMAGE and DUAL were compared with the originals.
For rows the earlier capture did not contain, the render used hand-written
translations; the live translation model was not run on them. This local
candidate is not Authenticode-signed and is not a public release.
