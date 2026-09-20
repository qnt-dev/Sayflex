# SayFlex Screen Translation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add private, local screen-region OCR translation to the current SayFlex release candidate while reducing the compact result toolbar to the approved `ORIG`, `DUAL`, and `PIN` controls.

**Architecture:** A new configurable input-host action starts one request generation. The desktop captures immutable physical-pixel frames before showing per-monitor selection surfaces, crops the chosen cross-monitor rectangle, sends it to a supervised local OCR worker, reconstructs readable text, and passes that text once through the existing translation worker and result-card pipeline. Capture, OCR, translation, overlay teardown, cancellation, and late-result rejection all use the same `RequestId`.

**Tech Stack:** Rust 1.88 / edition 2024, Slint 1.17.1, Win32 low-level input hooks, DXGI Desktop Duplication with bounded GDI fallback, ONNX Runtime CPU candidate, Tesseract 5 benchmark candidate, Windows Media OCR packaged benchmark candidate, line-delimited JSON worker protocols, PowerShell/Pester release gates.

**Spec:** `docs/superpowers/specs/2026-09-20-sayflex-screen-translation-design.md`

## Global Constraints

- The default configurable binding is exactly `Ctrl+Alt+G` and uses the existing atomic hotkey validation contract.
- Capture every active monitor before any SayFlex dim or selection surface is shown.
- Selection surfaces are non-activating; `Esc`, right-click, zero-area selection, teardown failure, disconnect, and watchdog timeout must all clear capture mode.
- Production capture is DXGI Desktop Duplication per output; GDI `BitBlt` is only a bounded ordinary-desktop fallback.
- Coordinates crossing process and platform boundaries are physical virtual-desktop pixels, including negative origins and rotated outputs.
- No screenshot, OCR source text, translation, API key, window title, or foreground document identity may enter diagnostics; screenshots never enter disk, clipboard, history, or crash metadata.
- OCR runs locally in a supervised `SayFlex.Ocr.exe`; cloud OCR is out of scope and CUDA is not part of the first OCR path.
- The portable OCR engine must run in both the direct installer and Store MSIX, be compatible with the final OSS license, beat Tesseract aggregate character accuracy on the checked-in corpus, and stay below 500 ms p95 on the release test machine.
- If no portable candidate meets all OCR gates, public packaging remains disabled for this feature; there is no silent MSIX-only or cloud-only fallback.
- Only recognized text may reach the already configured translation provider route; pixels never leave the machine.
- The compact card toolbar contains exactly `ORIG`, `DUAL`, and `PIN`; it contains no language label, swap, retry, or replace-selection control.
- Final admission requires automated gates, installed direct-installer and MSIX checks, and user visual approval of dimming, selection border, processing state, and result placement.

## Review Focus

- A selection spanning a `-1920` monitor origin and a `150%` monitor must crop the exact physical pixels once; Task 3 adds the geometry fixture.
- Pressing `Esc` during capture and then using `Esc` in the previously focused app must prove suppression ended without focus theft; Task 4 adds the hook/coordinator integration test.
- A worker result arriving after cancellation or after a newer capture must be ignored without changing the visible card; Task 8 adds stale-generation tests.
- Blank, protected, all-low-confidence, and mixed-column crops must produce distinct safe outcomes rather than garbage translation; Tasks 5 and 7 add corpus and reconstruction tests.
- Repeating large capture/cancel cycles must release frame buffers and never serialize pixels or OCR text into logs or files; Tasks 3, 6, and 9 add bounded-memory and privacy checks.

---

## File Map

- `rust-app/apps/desktop/ui/app.slint`: approved result toolbar plus the new frozen-frame selection surface.
- `rust-app/apps/desktop/src/screen_translation.rs`: request state machine and capture/OCR/translation orchestration.
- `rust-app/apps/desktop/src/translation_view.rs`: result-card origin policy and anchor placement only.
- `rust-app/apps/desktop/src/worker_supervisor.rs`: OCR worker lifecycle, restart, shutdown, and Job Object ownership.
- `rust-app/apps/input-host/src/hotkeys.rs`: `Ctrl+Alt+G` dispatch and temporary `Esc` capture mode.
- `rust-app/crates/platform-windows/src/screen_capture.rs`: DXGI/GDI frame acquisition and physical-pixel cropping.
- `rust-app/crates/platform-windows/src/region_selection.rs`: global pointer tracking without activation.
- `rust-app/crates/ocr-engine/src/lib.rs`: engine-neutral OCR result, preprocessing, inference adapter, and reading-order reconstruction.
- `rust-app/apps/ocr-worker/src/main.rs`: private OCR JSON process boundary.
- `rust-app/crates/protocol/src/lib.rs`: typed hotkey, OCR command/event, rectangle, line, and failure schemas.
- `rust-app/tools/ocr-benchmark/`: reproducible three-candidate corpus runner; it is not shipped.
- `rust-app/tests/fixtures/ocr-screen/`: checked-in PNG corpus and exact expected Unicode text.
- `rust-app/scripts/publish.ps1`, `rust-app/scripts/validate-release.ps1`, `rust-app/installer/SayFlex.iss`: package and validate the admitted OCR executable/runtime/model assets.

### Task 1: Remove the Unapproved Result-Card Controls

**Files:**
- Modify: `rust-app/apps/desktop/ui/app.slint:515-865`
- Modify: `rust-app/apps/desktop/src/translation_view.rs:197-280`
- Modify: `rust-app/apps/desktop/src/main.rs:158-190,1310-1460`
- Modify: `rust-app/apps/desktop/src/localization.rs:2610-2640`

**Interfaces:**
- Consumes: existing `TranslationPreview` view modes and pin callback.
- Produces: a compact toolbar whose rendered permanent controls are exactly `ORIG`, `DUAL`, and `PIN`.

- [ ] **Step 1: Add a source-contract test before changing Slint**

```rust
#[test]
fn translation_preview_toolbar_contains_only_approved_controls() {
    let source = include_str!("../ui/app.slint");
    let preview = source.split("export component TranslationPreview").nth(1).unwrap();
    assert!(preview.contains("text: @tr(\"ORIG\")"));
    assert!(preview.contains("text: @tr(\"DUAL\")"));
    assert!(preview.contains("text: @tr(\"PIN\")"));
    for forbidden in ["source-language-label", "target-language-label", "retry-translation", "text: \"⇄\"", "text: \"↻\""] {
        assert!(!preview.contains(forbidden), "unapproved toolbar token: {forbidden}");
    }
}
```

- [ ] **Step 2: Run the narrow test and verify it fails on the current language/swap/retry UI**

Run: `cargo test -p sayflex-desktop translation_preview_toolbar_contains_only_approved_controls -- --nocapture`

Expected: FAIL naming `source-language-label` or `retry-translation`.

- [ ] **Step 3: Remove the label, swap and retry rectangles, their Slint properties/callback, and the now-unreachable Rust callback wiring**

Keep the left positions and sizes of `ORIG` and `DUAL`, keep `PIN` aligned at the right edge, and do not replace the removed controls with spacing ornaments or new glyphs. Retain translation retry domain data only if another non-toolbar flow consumes it; remove dead `RuntimeEvent::RetryTranslation` and `on_retry_translation` wiring when `rg` proves no remaining caller.

- [ ] **Step 4: Compile and run result-card tests**

Run: `cargo test -p sayflex-desktop translation_preview -- --nocapture`

Expected: PASS and Slint compilation succeeds.

- [ ] **Step 5: Launch the existing preview mode and visually inspect the exact toolbar**

Run: `cargo run -p sayflex-desktop -- --preview-translation`

Expected: only `ORIG`, `DUAL`, and `PIN` are visible; no language text and no square/missing glyph remain.

- [ ] **Step 6: Commit the approved cleanup**

```powershell
git add rust-app/apps/desktop/ui/app.slint rust-app/apps/desktop/src/translation_view.rs rust-app/apps/desktop/src/main.rs rust-app/apps/desktop/src/localization.rs
git commit -m "fix(desktop): keep only approved translation card controls"
```

### Task 2: Add the Typed Screen-Capture and OCR Protocol

**Files:**
- Modify: `rust-app/Cargo.toml`
- Modify: `rust-app/crates/protocol/src/lib.rs:1-20,650-1050,1300-2720`
- Modify: `rust-app/crates/app-core/src/hotkeys.rs`
- Modify: `rust-app/apps/input-host/src/hotkeys.rs:1-980`
- Modify: `rust-app/apps/desktop/src/settings_view.rs:540-730,3330-3420`
- Modify: `rust-app/apps/desktop/src/localization.rs`
- Modify: `rust-app/apps/desktop/src/settings_document.rs`

**Interfaces:**
- Consumes: `RequestId`, `HotkeyAction`, `HotkeyBindings`, existing complete-schema fingerprint tests.
- Produces: `HotkeyAction::ScreenTranslate`, `InputHostEvent::ScreenCaptureRequested`, `InputHostCommand::SetScreenCaptureMode`, `OcrWorkerCommand`, `OcrWorkerEvent`, `PhysicalRect`, `OcrLine`, and `OcrFailureKind`.

- [ ] **Step 1: Extend protocol tests first**

```rust
assert_eq!(HotkeyBindings::default().screen_translate.spec, HotkeySpec::control_alt(u16::from(b'G')));
assert_eq!(HotkeyAction::ALL.len(), 6);
round_trip(InputHostEvent::ScreenCaptureRequested { request_id });
round_trip(InputHostCommand::SetScreenCaptureMode { request_id, active: true });
round_trip(OcrWorkerCommand::Recognize {
    request_id,
    width: 2,
    height: 1,
    stride: 8,
    pixel_format: OcrPixelFormat::Rgba8,
    language_policy: OcrLanguagePolicy::Auto,
    rgba_base64: "AAAAAAAAAAA=".into(),
});
round_trip(OcrWorkerEvent::Cancelled { request_id });
```

Define exact serializable types:

```rust
pub struct PhysicalRect { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }
pub enum OcrLanguagePolicy { Auto, Prefer(Vec<String>) }
pub enum OcrPixelFormat { Rgba8 }
pub struct OcrLine { pub text: String, pub bounds: PhysicalRect, pub confidence_milli: u16 }
pub enum OcrFailureKind { ModelUnavailable, LanguageUnavailable, NoText, InvalidImage, Engine, Cancelled }
```

- [ ] **Step 2: Run the protocol and hotkey tests and verify compile failures name the new variants/types**

Run: `cargo test -p rust-protocol -p rust-app-core -p sayflex-input-host`

Expected: FAIL until every exhaustive match and schema inventory is updated.

- [ ] **Step 3: Implement the new variants, bump `PROTOCOL_VERSION` once, update full wire inventories, and preserve old settings with `#[serde(default)]`**

`HotkeyBindings::default().screen_translate` is enabled at `Ctrl+Alt+G`. Add localized settings copy equivalent to “Screen translation / Select an area of the screen”. Old settings documents deserialize with the new default field and are rewritten only by the existing settings save path.

- [ ] **Step 4: Add input-host dispatch and temporary cancellation mode tests**

```rust
assert_eq!(hook_action_for(HotkeyAction::ScreenTranslate), HookAction::ScreenCaptureRequested);
set_screen_capture_mode(Some(request_id));
assert_eq!(classify_key(VK_ESCAPE, KeyState::Down), HookDecision::ConsumeAndCancel(request_id));
set_screen_capture_mode(None);
assert_eq!(classify_key(VK_ESCAPE, KeyState::Down), HookDecision::PassThrough);
```

- [ ] **Step 5: Implement action registration and `SetScreenCaptureMode` handling without involving selection clipboard capture**

Use a dedicated registration ID and mask for `G`. `ScreenTranslate` emits only `ScreenCaptureRequested`; it never calls `begin_selection_capture()` and never snapshots an editable target.

- [ ] **Step 6: Run all affected tests and commit**

Run: `cargo test -p rust-protocol -p rust-app-core -p sayflex-input-host -p sayflex-desktop`

```powershell
git add rust-app/Cargo.toml rust-app/crates/protocol rust-app/crates/app-core rust-app/apps/input-host rust-app/apps/desktop/src/settings_view.rs rust-app/apps/desktop/src/settings_document.rs rust-app/apps/desktop/src/localization.rs
git commit -m "feat(protocol): add screen translation request contract"
```

### Task 3: Capture and Crop Immutable Multi-Monitor Frames

**Files:**
- Create: `rust-app/crates/platform-windows/src/screen_capture.rs`
- Create: `rust-app/crates/platform-windows/tests/screen_capture_geometry.rs`
- Modify: `rust-app/crates/platform-windows/src/lib.rs`
- Modify: `rust-app/crates/platform-windows/Cargo.toml`
- Modify: `rust-app/Cargo.toml`

**Interfaces:**
- Consumes: `rust_protocol::PhysicalRect`.
- Produces: `DesktopCaptureBackend::capture() -> Result<DesktopCapture, CaptureError>` and `DesktopCapture::crop(PhysicalRect) -> Result<RgbaFrame, CaptureError>`.

- [ ] **Step 1: Write deterministic geometry tests with no live monitor dependency**

```rust
#[test]
fn crop_composes_negative_origin_and_rotated_outputs_in_physical_pixels() {
    let capture = fixture_capture([
        solid_output(rect(-1920, 0, 1920, 1080), Rotation::Identity, [255, 0, 0, 255]),
        coordinate_output(rect(0, 0, 2160, 3840), Rotation::Rotate90),
    ]);
    let crop = capture.crop(rect(-4, 2, 8, 3)).unwrap();
    assert_eq!(crop.dimensions(), (8, 3));
    assert_eq!(crop.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(crop.pixel(7, 2), expected_rotated_pixel(3, 4));
}
```

Also test clamping outside the virtual desktop, overlapping output rejection, zero area, stride overflow, and an output reported at `150%` DPI without rescaling its pixels.

- [ ] **Step 2: Run the geometry target and verify it fails because the module does not exist**

Run: `cargo test -p rust-platform-windows --test screen_capture_geometry`

Expected: FAIL to compile on `screen_capture` imports.

- [ ] **Step 3: Implement owned frame types and crop composition before live capture**

```rust
pub struct RgbaFrame { pub rect: PhysicalRect, pub stride: u32, pub pixels: Vec<u8> }
pub struct DesktopCapture { generation: u64, outputs: Vec<CapturedOutput> }
pub trait DesktopCaptureBackend: Send + Sync { fn capture(&self) -> Result<DesktopCapture, CaptureError>; }
```

Use checked multiplication for every allocation and reject a virtual desktop beyond the configured byte cap. `Drop` must release owned buffers naturally; no global frame cache is allowed.

- [ ] **Step 4: Implement DXGI Desktop Duplication and the bounded GDI fallback**

Add only the required `windows` features for D3D11/DXGI/GDI. Normalize duplication textures into top-down RGBA and apply output rotation during normalization. Invoke GDI only for ordinary composited outputs after a typed temporary DXGI-unavailable error; map access-lost, protected/black capture, secure desktop, and unsupported rotation distinctly.

- [ ] **Step 5: Add backend-order and repeated-release tests with injectable fake backends**

```rust
let capture = capture_with_fallback(&dxgi_failing_temporarily, &gdi_success).unwrap();
assert_eq!(calls(), ["dxgi", "gdi"]);
for _ in 0..200 { drop(capture_backend.capture().unwrap()); }
assert_eq!(tracked_live_frame_bytes(), 0);
```

- [ ] **Step 6: Run tests and commit**

Run: `cargo test -p rust-platform-windows screen_capture -- --nocapture`

```powershell
git add rust-app/Cargo.toml rust-app/crates/platform-windows
git commit -m "feat(windows): capture immutable desktop frames"
```

### Task 4: Build the Non-Activating Cross-Monitor Selection Surface

**Files:**
- Create: `rust-app/crates/platform-windows/src/region_selection.rs`
- Create: `rust-app/apps/desktop/src/screen_translation.rs`
- Modify: `rust-app/crates/platform-windows/src/lib.rs`
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/overlay_presenter.rs`

**Interfaces:**
- Consumes: `DesktopCapture`, `InputHostCommand::SetScreenCaptureMode`, and `PhysicalRect`.
- Produces: `ScreenSelectionCoordinator::begin(RequestId)`, `SelectionTerminal::{Selected,Cancelled,Failed}`, and immediate teardown of every monitor surface.

- [ ] **Step 1: Add a coordinator test proving capture happens before any surface is shown**

```rust
let events = run_with_fakes([PointerEvent::Down(-10, 10), PointerEvent::Move(20, 30), PointerEvent::Up(20, 30)]);
assert_eq!(events, ["capture", "esc:on", "show:-1920", "show:0", "teardown", "esc:off", "selected:-10,10,30,20"]);
```

Add terminal-path cases for `Esc`, right-click, zero-area click, presenter failure, desktop disconnect, and 15-second watchdog expiry. Each must contain exactly one `esc:off` and no surface after termination.

- [ ] **Step 2: Run the narrow desktop test and verify missing coordinator types fail compilation**

Run: `cargo test -p sayflex-desktop screen_translation::tests -- --nocapture`

- [ ] **Step 3: Implement `ScreenRegionOverlay` in Slint**

The exported component accepts a frozen `image`, monitor physical rectangle, dim opacity, and global selection rectangle. It renders the frozen image, a uniform dim layer outside the selection, and the existing SayFlex signal red border around the clear region. It has no title chrome, text toolbar, keyboard focus, or result controls.

- [ ] **Step 4: Implement global physical pointer tracking without window activation**

`RegionSelectionMonitor` reports left down/move/up and right down in virtual-desktop coordinates. Acquire and release pointer tracking as one RAII lease, reject a second concurrent lease, and always unhook in `Drop`.

- [ ] **Step 5: Wire input-host `Esc` cancellation and teardown acknowledgements**

Call `SetScreenCaptureMode { active: true }` only after capture succeeds and before surfaces show. Call the matching `active: false` on every terminal path. Stale `ScreenCaptureCancelled` events whose request ID is not current are ignored.

- [ ] **Step 6: Run tests, manual focus smoke, and commit**

Run: `cargo test -p rust-platform-windows region_selection -- --nocapture`

Run: `cargo test -p sayflex-desktop screen_translation -- --nocapture`

Manual check: begin selection over Notepad, cancel with `Esc`, type immediately, and verify Notepad still owns focus and receives the next `Esc` normally.

```powershell
git add rust-app/crates/platform-windows rust-app/apps/desktop
git commit -m "feat(desktop): add frozen screen region selection"
```

### Task 5: Establish the Reproducible OCR Engine Gate

**Files:**
- Create: `rust-app/tools/ocr-benchmark/Cargo.toml`
- Create: `rust-app/tools/ocr-benchmark/src/main.rs`
- Create: `rust-app/tools/ocr-benchmark/src/metrics.rs`
- Create: `rust-app/tests/fixtures/ocr-screen/manifest.json`
- Create: `rust-app/tests/fixtures/ocr-screen/*.png`
- Create: `rust-app/scripts/benchmark-ocr.ps1`
- Create: `docs/evidence/screen-translation-ocr-benchmark.md`
- Modify: `rust-app/Cargo.toml`

**Interfaces:**
- Consumes: RGBA fixtures and candidate adapters for PP-OCR/ONNX CPU, Tesseract 5, and packaged Windows Media OCR.
- Produces: machine-readable JSON rows with `engine`, `fixture`, normalized character distance, exact text, elapsed milliseconds, availability, and license/source metadata; selects no engine unless all gates pass.

- [ ] **Step 1: Check in a manifest with exact expected strings and categories**

```json
{
  "schema_version": 1,
  "fixtures": [
    {"file":"light-en.png","language":"en","category":"light","expected":"SayFlex keeps the source text."},
    {"file":"dark-ru.png","language":"ru","category":"dark","expected":"Перевод выделенной области"},
    {"file":"small-mixed.png","language":"auto","category":"small","expected":"Status: готово — build 42"},
    {"file":"two-columns.png","language":"en","category":"columns","expected":"Left one\nLeft two\n\nRight one\nRight two"},
    {"file":"blank.png","language":"auto","category":"blank","expected":""}
  ]
}
```

Generate the PNGs once from deterministic checked-in rendering code, inspect them, then keep the PNGs and manifest as the immutable benchmark input. Add representative fixtures for every SayFlex-supported source language before running the release gate.

- [ ] **Step 2: Add metric tests before candidate adapters**

```rust
assert_eq!(normalized_cer("abc", "abc"), 0.0);
assert_eq!(normalized_cer("abc", "axc"), 1.0 / 3.0);
assert_eq!(paragraph_normalize("A\r\nB  "), "A\nB");
```

- [ ] **Step 3: Implement all three adapters without placing any candidate in the shipped graph**

Pin `ort = 2.0.0-rc.13` for PP-OCR CPU. Use RapidOCR's versioned PP-OCRv5 ONNX artifact metadata and Apache-2.0 attribution, a Tesseract 5 executable/tessdata supplied explicitly to the script, and `Windows.Media.Ocr` only when the benchmark process has package identity and the language pack exists. Missing candidates produce `available:false`; they do not silently disappear from results.

- [ ] **Step 4: Run the benchmark on the release test machine and write measured evidence**

Run: `powershell -NoProfile -File scripts/benchmark-ocr.ps1 -Iterations 30 -Output docs/evidence/screen-translation-ocr-benchmark.md`

Expected admission rule: the same portable candidate supports direct and MSIX identities, is OSS-compatible, has lower aggregate CER than Tesseract, and has normal-crop p95 below `500` ms. The evidence file records machine CPU, Windows build, exact model/runtime hashes, per-category CER, p50/p95, failures, and the admitted engine name.

- [ ] **Step 5: Enforce the gate**

If the PP-OCR candidate passes, set the evidence decision to `PP-OCR/ONNX CPU admitted` and continue Task 6. If it fails any gate, set `screen_translation_packaging=false`, keep Tasks 6-8 buildable only behind the disabled development feature, and skip public packaging changes in Task 9. Do not substitute Windows Media OCR as the public engine.

- [ ] **Step 6: Commit benchmark code, corpus, and measured evidence**

```powershell
git add rust-app/tools/ocr-benchmark rust-app/tests/fixtures/ocr-screen rust-app/scripts/benchmark-ocr.ps1 docs/evidence/screen-translation-ocr-benchmark.md rust-app/Cargo.toml rust-app/Cargo.lock
git commit -m "test(ocr): establish portable engine admission gate"
```

### Task 6: Implement the Supervised OCR Worker and Model Lifecycle

**Files:**
- Create: `rust-app/crates/ocr-engine/Cargo.toml`
- Create: `rust-app/crates/ocr-engine/src/lib.rs`
- Create: `rust-app/crates/ocr-engine/src/ppocr.rs`
- Create: `rust-app/apps/ocr-worker/Cargo.toml`
- Create: `rust-app/apps/ocr-worker/build.rs`
- Create: `rust-app/apps/ocr-worker/src/main.rs`
- Modify: `rust-app/Cargo.toml`
- Modify: `rust-app/crates/model-manager/src/bundle.rs`
- Modify: `rust-app/apps/desktop/src/worker_supervisor.rs`

**Interfaces:**
- Consumes: admitted Task 5 model/runtime hashes and `OcrWorkerCommand`.
- Produces: `OcrEngine::recognize(&RgbaImage, &OcrLanguagePolicy, &CancellationToken) -> Result<OcrDocument, OcrError>` and a supervised `JsonWriter` for `SayFlex.Ocr.exe`.

- [ ] **Step 1: Add engine contract tests with a deterministic fake inference adapter**

```rust
let document = engine_with(fake_lines()).recognize(&image, &OcrLanguagePolicy::Auto, &token).unwrap();
assert_eq!(document.lines[0].bounds, rect(10, 12, 80, 16));
token.cancel();
assert_eq!(engine_with(fake_lines()).recognize(&image, &OcrLanguagePolicy::Auto, &token), Err(OcrError::Cancelled));
```

Also test invalid dimensions/base64, byte-cap rejection before allocation, unsupported language, and no pixel/text content in `Display`/`Debug` errors.

- [ ] **Step 2: Implement the admitted ONNX CPU adapter with model paths supplied by the model manager**

Do not enable `ort` auto-download in the shipped crate. Load the staged ONNX Runtime DLL and verified detector/recognizer/dictionary files from catalog-owned paths. Keep model choice explicit by language family and return `LanguageUnavailable` when the required verified pack is absent.

- [ ] **Step 3: Add an OCR bundle descriptor and full removal accounting**

The catalog entry records exact URLs, SHA-256 values, byte lengths, Apache-2.0 license, attribution, and owned relative paths from Task 5 evidence. Reuse `BundleInstaller` resume, receipt, atomic replacement, and `remove_complete`; add a test asserting runtime, detector, recognizers, dictionaries, partial downloads, receipts, and empty parent directories are counted and removed.

- [ ] **Step 4: Implement the worker loop and cancellation**

The worker emits `Ready { protocol_version, engine }`, accepts `Recognize`, `Cancel`, and `Shutdown`, decodes base64 after validating dimensions/stride/cap, and emits `Recognized`, typed `Failed`, or `Cancelled`. Diagnostic lines contain request ID, dimensions, engine, confidence summary, and timing only.

- [ ] **Step 5: Add OCR worker supervision beside existing workers**

Add `DEV_OCR_WORKER_EXE`, `WorkerKind::Ocr`, child/writer fields, startup handshake, restart budget, Job Object assignment, protocol mismatch handling, graceful shutdown, and crash recovery. A crash fails the current OCR request once; restart does not replay pixel payloads.

- [ ] **Step 6: Run tests and commit**

Run: `cargo test -p rust-ocr-engine -p sayflex-ocr-worker -p sayflex-desktop worker_supervisor -- --nocapture`

```powershell
git add rust-app/Cargo.toml rust-app/Cargo.lock rust-app/crates/ocr-engine rust-app/crates/model-manager rust-app/apps/ocr-worker rust-app/apps/desktop/src/worker_supervisor.rs
git commit -m "feat(ocr): add supervised local OCR worker"
```

### Task 7: Reconstruct Reading Order and Safe OCR Outcomes

**Files:**
- Create: `rust-app/crates/ocr-engine/src/reconstruct.rs`
- Create: `rust-app/crates/ocr-engine/tests/reconstruct.rs`
- Modify: `rust-app/crates/ocr-engine/src/lib.rs`
- Modify: `rust-app/apps/desktop/src/localization.rs`
- Modify: `rust-app/apps/desktop/translations/*/LC_MESSAGES/sayflex-desktop.po`

**Interfaces:**
- Consumes: ordered or unordered `Vec<OcrLine>`.
- Produces: `reconstruct_text(lines: &[OcrLine], min_confidence_milli: u16) -> Result<ReconstructedText, OcrFailureKind>`.

- [ ] **Step 1: Write exact reconstruction fixtures**

```rust
assert_eq!(reconstruct_text(&two_columns_shuffled(), 650).unwrap().text, "Left one\nLeft two\n\nRight one\nRight two");
assert_eq!(reconstruct_text(&hyphenated("trans-", "lation"), 650).unwrap().text, "translation");
assert_eq!(reconstruct_text(&hyphenated("Ctrl-", "Enter"), 650).unwrap().text, "Ctrl-\nEnter");
assert_eq!(reconstruct_text(&low_confidence_noise(), 650), Err(OcrFailureKind::NoText));
```

- [ ] **Step 2: Run the test and verify it fails before implementation**

Run: `cargo test -p rust-ocr-engine --test reconstruct`

- [ ] **Step 3: Implement geometry clustering, column order, paragraph gaps, lexical hyphen joining, and confidence filtering**

Use median line height for thresholds, stable top/left tie-breaking, non-overlapping X clusters for columns, and Unicode-aware alphabetic continuation. Do not flatten separate columns into alternating rows.

- [ ] **Step 4: Add localized typed outcomes**

Add strings for capture unavailable, protected content, empty selection, no text found, OCR model unavailable, OCR language unavailable, OCR worker failure, and cancellation in every shipped locale. Extend localization completeness tests so untranslated English leakage fails the suite.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p rust-ocr-engine -p sayflex-desktop localization -- --nocapture`

```powershell
git add rust-app/crates/ocr-engine rust-app/apps/desktop/src/localization.rs rust-app/apps/desktop/translations
git commit -m "feat(ocr): reconstruct screen text and localize outcomes"
```

### Task 8: Route OCR Text Through Existing Translation and Result Card

**Files:**
- Modify: `rust-app/apps/desktop/src/screen_translation.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Modify: `rust-app/apps/desktop/ui/app.slint`

**Interfaces:**
- Consumes: `OcrWorkerEvent::Recognized`, existing `TranslationWorkerCommand::Translate`, and `TranslationPreviewRegistry`.
- Produces: a screen-origin translation request anchored to the selected `PhysicalRect`, with stale output rejected by the common `RequestId`.

- [ ] **Step 1: Add state-machine tests for success, supersession, close, pin, and structured errors**

```rust
let first = machine.begin(rect(10, 20, 300, 120));
let second = machine.begin(rect(20, 30, 300, 120));
machine.on_ocr(recognized(first, "stale"));
assert!(translations().is_empty());
machine.on_ocr(recognized(second, "hello"));
assert_eq!(translations(), [translate_command(second, "hello")]);
```

Assert that closing an unpinned processing/result card sends OCR/translation cancellation, pinning completed content retains text but no bitmap, provider quota/credential/timeout messages use the existing typed status path, and translation history obeys the existing preference.

- [ ] **Step 2: Run the state-machine test and verify failures name missing screen-origin handling**

Run: `cargo test -p sayflex-desktop screen_translation::tests -- --nocapture`

- [ ] **Step 3: Implement the single-route handoff**

On usable OCR completion, drop the crop buffer before sending exactly one existing `TranslationWorkerCommand::Translate`. Add `TranslationOrigin::ScreenRegion { anchor: PhysicalRect }` only in desktop state; do not create a second provider router or send pixels to translation-worker.

- [ ] **Step 4: Anchor processing and result cards safely**

Place the card adjacent to the selected rectangle, clamp it to the nearest monitor work area, and flip above/below when needed. Screen-origin cards never expose replace-selection. Permanent controls remain exactly the Task 1 set.

- [ ] **Step 5: Add privacy assertions around serializers, diagnostics, history, clipboard, and temp directories**

Use a unique canary embedded in fixture pixels and a separate OCR-text canary. After success, failure, cancellation, worker crash, and history-disabled paths, scan captured diagnostics, history storage, clipboard test double, and task-owned temporary directory; the pixel canary appears nowhere, and OCR text appears only in the active translation request/result permitted by the history preference.

- [ ] **Step 6: Run integration tests and commit**

Run: `cargo test -p sayflex-desktop screen_translation -- --nocapture`

```powershell
git add rust-app/apps/desktop
git commit -m "feat(desktop): translate recognized screen regions"
```

### Task 9: Package, Validate, and Exercise Both Windows Identities

**Files:**
- Modify: `rust-app/scripts/publish.ps1`
- Modify: `rust-app/scripts/validate-release.ps1`
- Modify: `rust-app/scripts/smoke-installed.ps1`
- Modify: `rust-app/scripts/tests/release-validation.Tests.ps1`
- Modify: `rust-app/installer/SayFlex.iss`
- Modify: `rust-app/installer/msix/AppxManifest.xml` only if the admitted worker requires an existing declared capability; do not add broad screen-capture capability without evidence.
- Modify: `rust-app/THIRD_PARTY_NOTICES.md`
- Create: `docs/evidence/screen-translation-installed-smoke.md`

**Interfaces:**
- Consumes: admitted `SayFlex.Ocr.exe`, ONNX Runtime/model assets, build manifest, direct installer, and MSIX layout.
- Produces: hash-bound staged artifacts and an installed-smoke receipt for both identities.

- [ ] **Step 1: Add failing release-layout assertions first**

```powershell
$publish | Should -Match "SayFlex\.Ocr\.exe"
$installer | Should -Match "SayFlex\.Ocr\.exe"
$validator | Should -Match "SayFlex\.Ocr\.exe"
$notices | Should -Match "ONNX Runtime"
$notices | Should -Match "PaddleOCR|RapidOCR"
```

Also assert that a missing or unexpected OCR DLL/model makes release validation fail and that an OCR-disabled admission decision forbids those files from silently entering the payload.

- [ ] **Step 2: Run Pester and verify the new assertions fail**

Run: `Invoke-Pester rust-app/scripts/tests/release-validation.Tests.ps1 -Output Detailed`

- [ ] **Step 3: Add OCR build/staging/installer entries and exact third-party notices**

Publish `sayflex-ocr-worker.exe` as `SayFlex.Ocr.exe`, stage only the admitted CPU runtime and verified catalog assets, include all of them in `build-manifest.json`, and let the existing MSIX layout copy from that same authoritative stage. Do not add Python, CUDA, cloud OCR credentials, or benchmark-only Tesseract/WinRT binaries.

- [ ] **Step 4: Run source and release gates**

Run: `cargo fmt --all -- --check`

Run: `cargo clippy --workspace --all-targets --locked -- -D warnings`

Run: `cargo test --workspace --locked`

Run: `Invoke-Pester rust-app/scripts/tests -Output Detailed`

Run: `powershell -NoProfile -File rust-app/scripts/publish.ps1`

Run: `powershell -NoProfile -File rust-app/scripts/validate-release.ps1`

- [ ] **Step 5: Perform installed direct-installer and MSIX smoke checks**

For each identity, record: mixed `100%/150%` monitors including a negative origin, cross-monitor drag, browser text, Electron text, PDF text, ordinary game window, protected video, `Esc` and right-click cancellation, worker crash/restart, model removal, and 50 repeated capture/cancel cycles. Confirm foreground focus remains unchanged and Task Manager/private-byte sampling returns to the established steady-state band.

- [ ] **Step 6: Obtain visual approval before release admission**

Show the real installed dim layer, selection border, processing card, empty/error state, successful `ORIG`, `DUAL`, and translation-only states, plus result placement at every work-area edge. Record the user's approval in `docs/evidence/screen-translation-installed-smoke.md`; without approval, keep the feature excluded from the signed release candidate.

- [ ] **Step 7: Commit packaging and evidence**

```powershell
git add rust-app/scripts rust-app/installer rust-app/THIRD_PARTY_NOTICES.md docs/evidence/screen-translation-installed-smoke.md
git commit -m "build(release): package admitted screen translation"
```

### Task 10: Final Branch Verification and Handoff

**Files:**
- Review: every file changed by Tasks 1-9
- Modify: `docs/evidence/screen-translation-installed-smoke.md` only with final command outputs and artifact hashes

**Interfaces:**
- Consumes: completed implementation and release artifacts.
- Produces: an evidence-backed release-candidate handoff; no signing or Store upload is performed by this plan.

- [ ] **Step 1: Re-read the design and map every requirement to a passing test or installed receipt**

The mapping must explicitly cover capture-before-dim, negative origins, mixed DPI, rotation, cross-monitor crop, all terminal teardown paths, no focus theft, OCR gate, reconstruction, privacy, stale output, worker recovery, model removal, exact toolbar controls, direct installer, MSIX, and visual approval.

- [ ] **Step 2: Verify repository hygiene**

Run: `git diff --check`

Run: `git status --short`

Run: `git log --oneline --decorate -12`

Expected: no unexplained generated files, no benchmark model blobs outside catalog-owned/test paths, and no unrelated user changes included.

- [ ] **Step 3: Run the complete final gate once more from a clean staged source**

Run: `cargo test --workspace --locked`

Run: `Invoke-Pester rust-app/scripts/tests -Output Detailed`

Run: `powershell -NoProfile -File rust-app/scripts/validate-release.ps1 -Installed`

Expected: all commands exit `0`; staged and installed hashes match the authoritative manifest.

- [ ] **Step 4: Stop before signing or Partner Center submission**

Report exact commits, test totals, installer/MSIX paths and hashes, OCR benchmark decision, known protected-content limitations, and whether user visual approval is recorded. Signing and Store submission require their own explicit action after this handoff.
