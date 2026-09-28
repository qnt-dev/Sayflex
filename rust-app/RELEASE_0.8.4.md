# SayFlex 0.8.4

## Screen-translation interaction repair

- Dense OCR output is reconstructed into bounded semantic blocks instead of
  one full-image translation surface. List items remain distinct, ordinary
  wrapped paragraphs remain intact, and long dense columns are capped at six
  source lines per editable block.
- Double-clicking any translated block opens the correction editor inside the
  same image window. The field receives keyboard focus immediately; no
  parallel editor window can remain after the result is closed.
- The screen result is an ordinary movable window until PIN is selected. PIN
  changes only its z-order and no longer switches compositor presentation or
  flashes the captured image over the desktop.
- The title strip is draggable. Close and temporary-image deletion are now
  separate actions: `×` closes the complete result, while `DEL` removes only
  the retained crop and keeps its text translation available.
- Retiring or replacing a result closes only the editor owned by that result,
  and a snapshot refresh cannot resurrect a cancelled editor.

## Upgrade

Install 0.8.4 over the existing SayFlex installation. The in-place upgrade
preserves settings, history, downloaded model packs, runtimes, and the personal
glossary stored outside the program directory.

This candidate remains unsigned until the public signing workflow is complete.
Automated and installed-artifact checks establish technical consistency, not a
claim about every external game, monitor layout, or protected-content surface.
