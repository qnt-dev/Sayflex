# SayFlex Hardening And Progress UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the approved release-hardening corrections, remove avoidable screen-capture latency, present translation in stable paragraphs with a polished `Translating` state, and keep every Pulse Matrix palette legible over light and dark applications.

**Architecture:** Preserve the already-green audit remediation as the baseline. Make DXGI wait for a real presented frame and retain reusable capture resources, activate selection input only after the frozen surfaces render, bound repeated requests and OCR work, then add paragraph-stable presentation and palette-specific dual-contrast rings without changing translation semantics.

**Tech Stack:** Rust 1.95 toolchain, Slint 1.17.1, Win32/DXGI/D3D11, llama.cpp streaming events, PP-OCR/ONNX CPU, Cargo tests, Pester release tests.

**Spec:** `docs/superpowers/specs/2026-09-20-sayflex-screen-translation-design.md`, plus the bounded visual design approved in the 2026-09-21 conversation.

## Global Constraints

- Do not discard or rewrite the existing 27-file audit-remediation diff.
- Never display unstable token fragments; publish only completed paragraphs and the final remainder.
- The processing label is always exactly `Translating`, independent of UI locale and translation languages.
- The translation card remains one native window and must not hide/recreate between paragraph updates.
- The card grows toward available work-area space, caps at its existing scroll height, and never leaves the monitor.
- Pulse Matrix remains transparent and keeps its approved motion; only ring contrast changes.
- All three Pulse Matrix palettes must remain distinguishable on both near-black and near-white backgrounds.
- No screenshot pixels, OCR text, or translation text enter diagnostics.

## Review Focus

- A fresh DXGI duplication may return `LastPresentTime == 0`; it must be released and retried within a bounded deadline without accepting a black bootstrap frame.
- Input suppression must not begin until all frozen monitor surfaces have rendered, and every failure must still release it exactly once.
- A repeated hotkey during capture must not replace the active generation or create a misleading error card.
- A translation containing several paragraphs must reveal only complete paragraphs, preserve their order, and commit the final unterminated paragraph on completion.
- Every Pulse Matrix palette must retain a visible light and dark ring component after premultiplied downsampling.

---

### Task 1: Preserve And Verify Existing Audit Remediation

**Files:** Existing modified clipboard, speech, model-manager, translation-engine, translation-worker, OCR, desktop, and release-script files.

**Interfaces:**
- Consumes: current dirty baseline.
- Produces: a named green baseline before new changes.

- [ ] Run `cargo test --workspace --locked` and retain the exact result.
- [ ] Map each earlier audit item to its current test and implementation owner.
- [ ] Do not change a green owner merely to restyle it.

### Task 2: Make Screen Capture Fast And Input-Safe

**Files:**
- Modify: `rust-app/crates/platform-windows/src/screen_capture/dxgi.rs`
- Modify: `rust-app/crates/platform-windows/src/screen_capture.rs`
- Modify: `rust-app/crates/platform-windows/tests/screen_capture_geometry.rs`
- Modify: `rust-app/apps/desktop/src/screen_translation.rs`

**Interfaces:**
- Produces: bounded bootstrap-frame retry, reusable DXGI capture state, backend timing labels, and surface-first input activation.

- [ ] Add failing pure tests for bootstrap frame classification and selection event order.
- [ ] Verify RED with the narrow platform and desktop tests.
- [ ] Release zero-present-time frames and retry until a real presentation or a 100 ms bootstrap deadline.
- [ ] Recreate cached device/duplication state only after access loss or topology change.
- [ ] Present all frozen surfaces first, then start pointer tracking and enable Esc suppression.
- [ ] Log only backend/stage/duration/output-count metadata.
- [ ] Verify GREEN with platform, desktop, and full workspace tests.

### Task 3: Bound Repeated Requests And OCR Resources

**Files:**
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/worker_supervisor.rs`
- Modify: `rust-app/apps/ocr-worker/src/main.rs`
- Modify: `rust-app/crates/ocr-engine/src/ppocr.rs`

**Interfaces:**
- Produces: active-hotkey rejection without supersession, lazy OCR worker startup, one shared detector, and one running plus newest pending OCR request.

- [ ] Add failing tests for repeated hotkey preservation, lazy worker creation, and detector/model sharing.
- [ ] Verify RED.
- [ ] Ignore a second capture request while selection is active and retain the first request.
- [ ] Spawn/load OCR on first screen-translation use, not application startup.
- [ ] Share one detector session across recognizer families and load recognizers on demand.
- [ ] Keep the already implemented one-running/one-newest-pending worker queue.
- [ ] Verify GREEN and confirm idle desktop memory no longer includes OCR model sessions.

### Task 4: Reveal Stable Paragraphs With A Smooth Processing State

**Files:**
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Modify: `rust-app/apps/desktop/ui/app.slint`

**Interfaces:**
- Produces: `StableTranslationProgress`, paragraph-only publication, animated card height, paragraph entrance, and the invariant `Translating` shimmer.

- [ ] Add failing tests proving partial tokens remain buffered until a blank-line boundary and completion flushes the remainder.
- [ ] Verify RED.
- [ ] Feed screen-origin partials into the buffer while leaving ordinary translator behavior unchanged.
- [ ] Reveal each completed paragraph once, with a 3-4 px upward settle and opacity transition.
- [ ] Animate the card to its measured target height over 220-260 ms without hiding or recreating the native window.
- [ ] Render `Translating` in a restrained red/amber/off-white travelling highlight; fade it out on terminal completion.
- [ ] Keep the current 252 px content cap and scrolling contract.
- [ ] Verify GREEN with source contracts, state tests, and preview-mode inspection.

### Task 5: Make Every Pulse Matrix Palette Background-Robust

**Files:**
- Modify: `rust-app/apps/desktop/src/pulse_matrix.rs`

**Interfaces:**
- Produces: per-palette dark under-stroke plus light/accent hairline while retaining existing geometry and motion.

- [ ] Add failing pixel tests for light- and dark-background contrast proxies in Cyan, Graphite/Silver, and Red/Amber.
- [ ] Verify RED.
- [ ] Draw every ring as a restrained two-layer stroke; do not add a panel or change dot animation.
- [ ] Preserve supersampling/downsampling and the approved ring radii.
- [ ] Verify GREEN and render all three preview palettes for user inspection.

### Task 6: Release Verification

**Files:** All files changed by Tasks 1-5.

**Interfaces:**
- Produces: source-level release evidence; no signing, publishing, or Store upload.

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- [ ] Run `cargo test --workspace --locked`.
- [ ] Run Pester under `rust-app/scripts/tests`.
- [ ] Build release binaries and record screen-capture timings from the release build only.
- [ ] Launch the real candidate and inspect paragraph growth plus all three Pulse Matrix palettes.
- [ ] Stop for user visual acceptance before signing or publication.
