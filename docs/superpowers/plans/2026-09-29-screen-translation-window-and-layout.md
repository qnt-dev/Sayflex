# Screen Translation Window and Measured Image Layout Implementation Plan

> **Execution:** Use `superpowers:executing-plans` to implement this plan sequentially in the current session. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the reused compact-translation popup and external overflow column with a dedicated frameless screenshot-result window that renders complete, measured translations inside the scaled image while preserving unrelated pixels.

**Architecture:** OCR reconstruction produces semantic context and separate stable visual rows made from one or more baseline-aligned fragments. A dedicated `ScreenTranslationWindow` owns screen-result lifecycle, text measurement, drag/pin/editor behavior, and a renderer-independent layout plan; compact text previews no longer change native contracts. The image view uniformly scales to the monitor work area, masks only contributing source fragments after a full placement succeeds, and has no external translation panel.

**Tech Stack:** Rust 2024, Slint 1.17, windows-sys/Win32, existing OCR and translation workers, serde JSON persistence, Cargo tests and Clippy.

**Spec:** `docs/superpowers/specs/2026-09-27-sayflex-contextual-screen-translation-and-reliability-design.md`

## Global Constraints

- Run all Cargo commands from `rust-app`; run Git commands from the worktree root.
- Preserve `ORIG`, `DUAL`, **ON IMAGE**, copy, save-file, delete-image, pin, close, double-click correction, glossary save/confirm, expiry, and screenshot-retention behavior.
- The image view has no right/bottom translation list or dense overflow panel.
- Semantic paragraph grouping never defines an image mask.
- Only source-fragment glyph rectangles may be masked, and only after the complete translation has a readable collision-free placement.
- Icons, unrelated labels, background, and standalone counters such as `45` and `275` remain at their original coordinates.
- Completed translation text is never clipped or elided; an unplaceable row keeps its original pixels and reports an explicit exceptional state.
- The dedicated result window is frameless for its whole lifetime, is ordinary z-order until **PIN**, and never enters Windows non-client caption dragging.
- Physical installed-UI acceptance is performed by the user from explicit steps and screenshots/video; automation does not replace that acceptance.

## Review Focus

- OCR splits one visual row into overlapping fragments (`DAMAGE TYPE:I` + `Magical`): merge once, preserve both fragment rectangles, and translate one row.
- OCR emits distant same-baseline counters (`45`, `275`): never merge them through an icon or a large gap.
- A translation fits by character count but not by the actual font: renderer measurement must reject the candidate before masking.
- A dense crop cannot place every row even at the maximum work-area scale: preserve original pixels and expose an explicit `not_placeable` row instead of clipping or adding a side panel.
- Window drag, pin, editor focus, DPI change, and result refresh must not replace the HWND contract, reveal a native caption, or orphan an editor.

---

### Task 1: Canonical Visual Rows and Fragment Geometry

**Files:**
- Modify: `rust-app/crates/ocr-engine/src/reconstruct.rs`
- Modify: `rust-app/crates/ocr-engine/tests/reconstruct.rs`
- Modify: `rust-app/crates/protocol/src/lib.rs`
- Modify: `rust-app/apps/desktop/src/screen_result.rs`
- Modify: `rust-app/apps/desktop/src/screenshot_store.rs`

**Interfaces:**
- Consumes: `ReconstructedElement { text, bounds, confidence_milli }` in OCR reading order.
- Produces: `ReconstructedVisualRow { text, bounds, fragments, confidence_milli }`, persisted `ScreenTextBlock.fragments: Vec<PhysicalRect>`, and stable row IDs used by translation, display, correction, and restore.

- [ ] **Step 1: Write failing OCR row tests**

Add tests named:

- `same_baseline_fragments_form_one_visual_row_with_both_masks`
- `distant_icon_counters_never_merge_into_one_visual_row`
- `low_confidence_symbol_is_not_absorbed_into_a_label`
- `semantic_paragraphs_do_not_replace_visual_row_geometry`

Assert the Nimbus fragments `DAMAGE TYPE:I` at `(30,180,112,21)` and `Magical` at `(135,185,51,14)` produce one row with two original rectangles, while `45` and `275` remain separate.

- [ ] **Step 2: Run the focused OCR tests and verify RED**

Run: `cargo test -p rust-ocr-engine --locked --test reconstruct visual_row -- --nocapture`

Expected: FAIL because visual rows and fragment geometry do not exist.

- [ ] **Step 3: Implement visual-row reconstruction**

Add `pub fn reconstruct_visual_rows(elements: &[ReconstructedElement]) -> Vec<ReconstructedVisualRow>` in `reconstruct.rs`. Fragments may merge when their vertical intersection is at least 45% of the smaller height and their horizontal gap is no greater than `max(12 px, 1.5 * smaller_height)`; low-confidence non-alphabetic fragments below 650 milli-confidence cannot be absorbed. Retain ordered fragments and compute the union rectangle without using semantic paragraph bounds.

- [ ] **Step 4: Write failing protocol/document/persistence tests**

Add tests proving:

- stable row IDs round-trip with all fragment rectangles;
- old persisted JSON without `fragments` restores with `[bounds]`;
- correction and late model output still address the row ID;
- numeric preservation counts values once after fragments merge.

- [ ] **Step 5: Run the document/persistence tests and verify RED**

Run: `cargo test -p rust-protocol --locked screen_block`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_result::tests`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screenshot_store::tests`

Expected: FAIL because `ScreenTextBlock` and persisted snapshots do not carry fragments.

- [ ] **Step 6: Implement the canonical row contract**

Add `#[serde(default)] pub fragments: Vec<PhysicalRect>` to `ScreenTextBlock`, validate non-empty in-bounds rectangles when present, and normalize an empty legacy list to the row `bounds` in the document owner. Change `screen_blocks_from_ocr` to consume reconstructed visual rows and assign one stable ID per row. Increment the shared IPC protocol version from 31 to 32 in every producer/consumer and update the protocol-version tests; reject mixed-version workers using the existing compatibility path.

- [ ] **Step 7: Run focused suites and verify GREEN**

Run: `cargo test -p rust-ocr-engine --locked --test reconstruct`

Run: `cargo test -p rust-protocol --locked screen_block`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_result::tests`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screenshot_store::tests`

Expected: PASS.

- [ ] **Step 8: Commit**

```text
git add rust-app/crates/ocr-engine rust-app/crates/protocol rust-app/apps/desktop/src/screen_result.rs rust-app/apps/desktop/src/screenshot_store.rs
git commit -m "feat(screen): preserve visual row fragments"
```

### Task 2: Structured Translation Uses Visual Rows

**Files:**
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/crates/translation-engine/src/engine.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`

**Interfaces:**
- Consumes: stable `ScreenTextBlock` visual rows from Task 1 plus the existing semantic `SourceContext.nearby_text`.
- Produces: one complete `TranslatedScreenBlock` per visual-row ID; mixed-script malformed output is retryable and never presented as a completed placement.

- [ ] **Step 1: Write failing structured-translation tests**

Add tests proving the request contains merged `DAMAGE TYPE:I Magical`, keeps full semantic nearby context, returns each row ID exactly once, and rejects a target-Russian mixed-script token such as `СCEPTER` unless it is a protected identifier/URL.

- [ ] **Step 2: Run the tests and verify RED**

Run: `cargo test -p rust-translation-engine --locked screen`

Expected: FAIL for split-row input or missing mixed-script validation.

- [ ] **Step 3: Implement row-aware validation and bounded retry**

Keep the existing batch budget and per-row fallback. Add `validate_target_script(source, translation, target_language, protected_tokens)` beside the existing preservation validators. A malformed completed row gets one bounded per-row retry; a second malformed result becomes an explicit failed/incomplete row and cannot be masked.

- [ ] **Step 4: Run translation and desktop coordinator tests**

Run: `cargo test -p rust-translation-engine --locked`

Run: `cargo test -p sayflex-translation-worker --locked`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_translation::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```text
git add rust-app/apps/desktop/src/main.rs rust-app/crates/translation-engine/src/engine.rs rust-app/apps/translation-worker/src/main.rs
git commit -m "fix(screen): translate complete visual rows"
```

### Task 3: Dedicated Screenshot-Result Window

**Files:**
- Create: `rust-app/apps/desktop/src/screen_result_window.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `rust-app/apps/desktop/ui/screen_translation.slint`
- Modify: `rust-app/crates/platform-windows/src/overlay.rs`

**Interfaces:**
- Consumes: `ScreenResultSnapshot`, `RuntimeEvent`, `OverlayPresenter`, and the existing screen correction/save/delete commands.
- Produces: `ScreenResultWindowSlot`, `ScreenResultWindowRegistry`, `create_screen_result_window() -> Result<ScreenResultWindowSlot, String>`, `ScreenResultWindowRegistry::slot_for_request(RequestId) -> Option<ScreenResultWindowSlot>`, `ScreenResultWindowRegistry::slot_by_handle(u64) -> Option<ScreenResultWindowSlot>`, and a production `ScreenTranslationWindow` whose HWND never shares compact-popup state.

- [ ] **Step 1: Write failing ownership and native-contract tests**

Add tests proving:

- the compact `TranslationPreview` type has no `screen-image-active` branch;
- a screen request is owned only by `ScreenResultWindowRegistry`;
- pinning changes only z-order and never swaps HWND/component identity;
- close, expiry, delete-all, supersede, and shutdown retire the matching screen window and editor once;
- repeated `SetWindowPos` movement preserves `WS_POPUP` and excludes `WS_CAPTION`/`WS_THICKFRAME`.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_result_window`

Run: `cargo test -p rust-platform-windows --locked screen_result_contract`

Expected: FAIL because screen results still reuse `TranslationPreviewSlot`.

- [ ] **Step 3: Add the dedicated Slint window and Rust owner**

Export `ScreenTranslationWindow inherits Window` from `screen_translation.slint` with the production properties/callbacks currently routed through `TranslationPreview`. Implement `create_screen_result_window`, `present_screen_result`, `pin_screen_result`, `retire_screen_result`, `cloak_screen_results_for_capture`, and request/HWND lookup in `screen_result_window.rs`.

- [ ] **Step 4: Remove screen mode from the compact preview**

Delete the screen-result properties/branch from `TranslationPreview`, remove `screen_result_active`/`screen_result_pinned` from `TranslationPreviewSlot`, and route all screen runtime events through `ScreenResultWindowRegistry` without changing compact translation behavior.

- [ ] **Step 5: Enforce the lifetime-long native contract**

Keep `configure_screen_result_window` dedicated to the new HWND. Client-owned drag calls only `SetWindowPos`; before and after movement, verify the native style remains frameless. Do not call `winit::Window::drag_window()` for this surface. Log contract violations with HWND and style bits without exposing captured content.

- [ ] **Step 6: Run focused and lifecycle tests**

Run: `cargo test -p rust-platform-windows --locked`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_result_window`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_translation_edit_borrow_tests`

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_translation_privacy_tests`

Expected: PASS.

- [ ] **Step 7: Commit**

```text
git add rust-app/apps/desktop/src/screen_result_window.rs rust-app/apps/desktop/src/main.rs rust-app/apps/desktop/src/translation_view.rs rust-app/apps/desktop/ui/app.slint rust-app/apps/desktop/ui/screen_translation.slint rust-app/crates/platform-windows/src/overlay.rs
git commit -m "refactor(screen): own results in a dedicated window"
```

### Task 4: Renderer-Measured In-Image Layout Without an External Panel

**Files:**
- Modify: `rust-app/apps/desktop/src/screen_result_view.rs`
- Modify: `rust-app/apps/desktop/src/screen_result_window.rs`
- Modify: `rust-app/apps/desktop/ui/screen_translation.slint`
- Modify: `rust-app/apps/desktop/src/screen_translation_preview.rs`
- Modify: `rust-app/crates/platform-windows/src/overlay.rs`

**Interfaces:**
- Consumes: visual rows/fragments, monitor logical work-area bounds, and renderer measurements for candidate width/font/wrap settings.
- Produces: `ScreenResultLayoutPlan { image_size, rows, masks, unplaceable_rows }`; no dense/overflow panel fields.

- [ ] **Step 1: Write failing Nimbus and adversarial layout tests**

Use the exact 675x458 persisted Nimbus geometry from request `28b48e74-9284-4215-a76d-b45183734fb6`. Assert:

- `content_width == image_width` and there is no panel width/state;
- every completed alphabetic row is either fully placed inside the image or explicitly `not_placeable`;
- every mask belongs to a source fragment of a placed row;
- `45`, `275`, their icon neighborhoods, and unrelated pixels have no masks;
- a fake renderer measurement larger than the candidate box prevents masking;
- a small work area yields explicit unplaceable rows rather than clipping.

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p sayflex-desktop --features screen-translation --locked screen_result_view::tests::nimbus`

Expected: FAIL because the current plan creates an external dense panel and estimates fit by character count.

- [ ] **Step 3: Add renderer-backed measurement**

Expose hidden measurement properties on `ScreenTranslationWindow` and implement `ScreenTextMeasurer::measure(text: &str, width: f32, font_size: f32) -> LogicalSize` using Slint `Text.preferred-width/preferred-height` with the same family, weight, wrapping, and DPI as the visible label. Candidate sizes are 14, 13, and 12 logical pixels; image translation never renders below 12 logical pixels.

- [ ] **Step 4: Implement work-area scaling and placement**

Change `plan_screen_result_layout` to accept logical work-area limits and a `ScreenTextMeasurer`. Uniform scale is capped at 1.75 and by `(work_width - 32) / crop_width` and `(work_height - 70) / crop_height`, preserving aspect ratio. Construct collision occupancy from all OCR fragments, try measured in-row expansion first and collision-free in-image callouts second, and emit `not_placeable` without masks when neither fits.

- [ ] **Step 5: Remove the external panel UI and contracts**

Delete `dense-panel-active`, `dense-panel-width`, `overflow-panel`, `panel-order`, the right-side `Flickable`, and dense preview state. Render flat mask rectangles separately from translated rows; a row is visible/masked only when its layout state is placed.

- [ ] **Step 6: Render the recorded Nimbus artifact automatically**

Run the existing environment-gated renderer test with the latest persisted crop and write original/result PNGs to `%LOCALAPPDATA%\Temp\sayflex-nimbus-regression-0.8.7`. This is technical evidence only, not user visual acceptance.

- [ ] **Step 7: Run desktop, format, and lint gates**

Run: `cargo test -p sayflex-desktop --features screen-translation --locked`

Run: `cargo fmt --all -- --check`

Run: `cargo clippy -p rust-ocr-engine -p rust-protocol -p rust-translation-engine -p rust-platform-windows -p sayflex-translation-worker -p sayflex-desktop --features sayflex-desktop/screen-translation --locked --all-targets -- -D warnings`

Expected: PASS with no warnings.

- [ ] **Step 8: Commit**

```text
git add rust-app/apps/desktop/src/screen_result_view.rs rust-app/apps/desktop/src/screen_result_window.rs rust-app/apps/desktop/src/screen_translation_preview.rs rust-app/apps/desktop/ui/screen_translation.slint rust-app/crates/platform-windows/src/overlay.rs
git commit -m "fix(screen): measure translations inside the image"
```

### Task 5: Integrated Lifecycle, Upgrade, and User Acceptance

**Files:**
- Modify: `rust-app/README.md`
- Modify: `rust-app/RELEASE_0.8.7.md`
- Modify: version declarations and installer metadata selected by the existing release scripts.

**Interfaces:**
- Consumes: completed Tasks 1-4 and existing release scripts.
- Produces: a validated installed build plus explicit user-run visual acceptance evidence.

- [ ] **Step 1: Run complete automated regression gates**

Run workspace tests required by `scripts/validate-release.ps1`, screen/OCR/translation suites, `cargo fmt --check`, and strict Clippy. Record exact pass/ignore/failure counts; do not convert ignored interactive tests into passes.

- [ ] **Step 2: Build and validate the installer**

Build the next patch release with `scripts/build-installer.ps1`, install it in place only after stopping processes whose executable paths are under `%LOCALAPPDATA%\Programs\SayFlex`, and run `scripts/validate-release.ps1 -InstallerPath <installer> -Installed`. Start the installed application with `Start-Process -WindowStyle Hidden`.

- [ ] **Step 3: Ask the user to perform physical visual acceptance**

Provide these exact checks:

1. translate the complete Nimbus card and send **ON IMAGE** screenshot;
2. hold and move the header, sending a short recording if any system title strip appears;
3. double-click an in-image translation and send the embedded editor screenshot;
4. toggle **PIN** and report any flash or second surface;
5. confirm `45`, `275`, icons, source relationships, and all fully placed translations remain visible and unclipped.

Expected: user-provided evidence; mark every unperformed check `not verified`.

- [ ] **Step 4: Update release evidence and commit**

Record installer SHA-256, installed EXE SHA-256, source commit, automated results, user-provided visual results, and remaining unverified checks in `RELEASE_0.8.7.md`.

```text
git add rust-app/README.md rust-app/RELEASE_0.8.7.md rust-app/Cargo.toml rust-app/Cargo.lock rust-app/installer
git commit -m "release: prepare SayFlex 0.8.7"
```
