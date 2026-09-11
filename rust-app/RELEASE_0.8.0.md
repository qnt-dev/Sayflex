# SayFlex 0.8.0

This release candidate completes the model lifecycle and interface-quality
work needed before broader Windows testing.

## What changed

- Installed speech and translation models can be removed from the Models page
  after an explicit confirmation. SayFlex removes only catalog-owned model,
  receipt, partial-download and backup paths; the shared local-translation
  runtime is removed when its final model pack is removed.
- Switching to a cloud-only provider chain now unloads the local speech or
  translation engine. The Models page distinguishes `In use` from
  `Selected · unloaded` so RAM/VRAM residency is not implied by an on-disk
  selection.
- The complete settings surface is localized for Russian, Ukrainian, Spanish,
  Brazilian Portuguese, German, French, Italian and Korean. The language
  control lives on About, while dictation and translation language support
  remains independent from the interface language.
- Hotkey changes use a visible capture field and an explicit Save boundary.
  Windows registration is reconciled per action: a conflicting replacement
  keeps the previous working shortcut without rolling back unrelated changes.
- The standalone translator now follows the SayFlex sheet design, keeps source
  and result visible, and exposes localized status and accessibility labels.
- Companion visual settings explain the complete move gesture: between
  dictations, point at the Companion, hold Ctrl+Alt, then drag with the left
  mouse button. Its saved position is restored.
- Protocol 23 adds typed model lifecycle events and per-action hotkey failures.
  Wire-shape fingerprints cover every process direction and nested payload.
- Store-oriented MSIX packaging and exact payload validation are available for
  Partner Center testing. This does not replace Microsoft account enrollment,
  Store certification, live packaged-app testing or Authenticode signing for
  the separate direct-download installer.

## Verification boundary

Automated tests and artifact validation establish technical consistency. A
release remains a candidate until the exact installed bytes pass real
dictation, translation, hotkey, model lifecycle, Companion and update smoke
tests on supported Windows systems.
