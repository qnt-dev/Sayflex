# SayFlex Contextual Screen Translation and Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver one integrated SayFlex update that closes the agreed reliability defects and adds context-aware, block-preserving screen translation, temporary screenshot history, on-image rendering, correction, and a personal glossary.

**Architecture:** Existing worker boundaries remain authoritative. Reliability fixes establish one owner for every operation first; shared protocol types then carry stable block identities and typed completion. The desktop owns the canonical screen document and pixels, the translation worker owns translation/glossary persistence, and Slint renders text and image views from the same revisioned document.

**Tech Stack:** Rust 1.88, Windows Win32/DXGI/GDI/PDH/UI Automation, Slint 1.17, JSON worker protocols, existing PP-OCR/ONNX CPU worker, Qwen through llama.cpp, Firefox/Bergamot through the pinned `fxtranslate` engine, Inno Setup, PowerShell release tests.

**Spec:** `docs/superpowers/specs/2026-09-27-sayflex-contextual-screen-translation-and-reliability-design.md`

## Global Constraints

- Produce one public update; intermediate tasks and builds are verification slices, not public releases.
- Preserve `ORIG`, `DUAL`, pinning, copy, translation history, `Ctrl+Alt+V`, standalone Translator, provider ordering, CUDA to Vulkan to CPU speech selection, and worker isolation.
- OCR remains CPU-only and enabled by default; no VLM, inpainting, image generation, or continuously moving game overlay.
- Persist only the selected crop, never full-monitor frames; default expiry is 24 hours and the temporary screenshot budget is 500 MB with oldest-first eviction.
- Full-monitor capture buffers never enter disk, clipboard, diagnostics, crash metadata, or cloud requests.
- Only user correction plus Save or explicit Confirm writes the glossary; ordinary translation, Copy, Pin, Close, and Insert do not.
- Context and OCR are typed untrusted data, separate from system instructions; output IDs and protected values are validated.
- Qwen3.5 4B Q4_K_M is the first contextual backend. Compact translation remains available but must state its limited context/glossary capability.
- Target aggregate SayFlex VRAM is at most 8 GB and must not exceed 10 GB on the 16 GB VRAM / 64 GB RAM validation machine while a game runs; these values are measured gates, not assumptions.
- Every production behavior change follows RED to GREEN. Source inspection is never reported as a runtime measurement.
- New UI uses the current `Theme`/`Sheet` system and must receive user visual approval in five required states before production wiring is finalized.

## Review Focus

- A clipboard broker materializes SayFlex delayed text and a user presses Copy immediately afterward: SayFlex must recognize its own materialization but preserve the later user clipboard.
- A screen selection covers a small game window over a large browser: only actually visible, non-occluded pixels may choose the context source; an ambiguous split has no context.
- A compact decoder reaches its output-token cap without EOS: the fragment must be `limit_reached`, never successful or eligible for automatic replacement.
- A pinned image result passes its 24-hour expiry while open: the crop must disappear from disk and memory while corrected text and glossary records remain usable.
- A late model response races a saved user edit and application restart: the saved revision must remain authoritative in the card, persisted result, copy, and reinsertion.

---

### Task 1: Make clipboard replacement and restoration one atomic owned transaction

**Files:**
- Modify: `rust-app/apps/input-host/src/clipboard.rs`
- Modify: `rust-app/scripts/tests/delayed-clipboard-image.Integration.Tests.ps1`
- Modify: `rust-app/tests/fixtures/delayed-clipboard-image-owner.ps1`
- Modify: `rust-app/tests/fixtures/delayed-clipboard-image-consumer.ps1`

**Interfaces:**
- Produces: private `ClipboardPublication { generation: u64, replacement_sequence: u32 }`.
- Produces: `ClipboardPasteHost::publish_if_unchanged(&mut self, expected_sequence: u32, payload: &str, target_process_id: u32, target_window_handle: u64) -> Result<ClipboardPublication, ClipboardError>`.
- Produces: `ClipboardPasteHost::owns_publication(&self, publication: ClipboardPublication) -> bool` and an atomic restore helper that revalidates owner/generation while the clipboard is open.
- Consumes: existing immutable `ClipboardSnapshot::prepare_restore` and continuously pumped STA render window.

- [x] **Step 1: Add failing unit tests for the capture-to-publish race and self-materialization**

  Add tests named `a_user_copy_between_capture_and_publish_prevents_replacement`, `own_delayed_materialization_keeps_publication_owned`, and `a_user_copy_after_materialization_prevents_restore`. Assert that destructive callbacks are never entered after a foreign sequence/owner change and that a same-generation `WM_RENDERFORMAT` sequence change remains owned.

- [x] **Step 2: Run the focused tests and verify RED**

  Run: `cargo test -p sayflex-input-host clipboard::tests -- --nocapture`
  Expected: the three new ownership tests fail because `dispatch_paste` publishes after an unlocked capture and restoration trusts only the original replacement sequence.

- [x] **Step 3: Implement publication identity and under-lock freshness checks**

  Move the pre-publish sequence check into the same `OpenClipboard` transaction as `EmptyClipboard`; increment a host generation for every publication; accept sequence movement caused by the same render HWND/generation; reject any different owner before replacement or restore. Keep `ClipboardSnapshot` immutable and duplicate formats per restore.

- [x] **Step 4: Extend the cross-process delayed-image fixture**

  Exercise readable text plus image formats, foreign Copy between snapshot/publication, foreign Copy after materialization, and restored-image reads after 0, 1, 10, and 60 seconds. Assert clipboard fidelity, bounded completion, and no host/consumer hang.

- [x] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-input-host clipboard::tests -- --nocapture`
  Expected: PASS.
  Run: `pwsh -NoProfile -File scripts/tests/delayed-clipboard-image.Integration.Tests.ps1`
  Expected: PASS with every delayed-read interval and competitive Copy scenario reported.

- [x] **Step 6: Commit**

  `git commit -m "fix(input): make clipboard publication ownership atomic"`

### Task 2: Close speech capture, cancellation, model lifecycle, and local fallback races

**Files:**
- Modify: `rust-app/apps/speech-worker/src/audio.rs`
- Modify: `rust-app/apps/speech-worker/src/inference.rs`
- Modify: `rust-app/apps/speech-worker/src/transcription.rs`
- Modify: `rust-app/apps/speech-worker/src/transcription_mailbox.rs`
- Modify: `rust-app/apps/speech-worker/src/model_actor.rs`
- Modify: `rust-app/apps/speech-worker/src/engine.rs`
- Modify: `rust-app/crates/cloud-speech/src/router.rs`
- Modify: `rust-app/apps/speech-worker/tests/protocol_contract.rs`

**Interfaces:**
- Produces: `CaptureDrainOutcome::{Active, LimitReached { session_id, recording }}` returned to the capture owner instead of publishing the limit from `drain_audio`.
- Produces: `SessionPublicationGuard` backed by the existing per-session cancellation tombstones; local and remote finalizers call `may_publish(session_id)` immediately before UI/history output and call `complete` only after the terminal path.
- Produces: configuration-owned `selected_language: DictationLanguage` reapplied to every prepared engine.
- Consumes: existing `SessionCancellationSet`, `InferenceCommand`, `ModelActor` cancellation generation, and cloud `RouteOutcome`.

- [x] **Step 1: Add failing tests for each owner transition**

  Add tests for five-minute auto-stop followed by a successful Begin; cancel during local final inference; cancel after a cloud response but before publication; cancel after model-file verification but before activation; language preservation through Unload/Prepare; old-context destruction before new-context construction; and an exhausted network budget with Local later in the provider order.

- [x] **Step 2: Run focused tests and verify RED**

  Run: `cargo test -p sayflex-speech-worker -- --nocapture`
  Expected: the capture, publication, activation, language, and replacement tests fail on current ownership.
  Run: `cargo test -p rust-cloud-speech router::tests -- --nocapture`
  Expected: the budget/local-fallback test returns `Exhausted` instead of `UseLocalEngine`.

- [x] **Step 3: Move the recording-limit terminal transition to the capture loop**

  Have `drain_audio` report a typed limit outcome once. The capture loop takes and finalizes the active buffer, stops further level/snapshot publication, resets dropped-sample state, notifies the inference/speculation owner to cancel that session, and only then sends the typed failure.

- [x] **Step 4: Preserve cancellation through final publication**

  Pass a session publication watcher into local `InferenceCommand::Finalize` and remote `NetworkJob`. Recheck it after native/network work and immediately before `Completed`, `TranscribedRemotely`, and `HistoryCommand::Append`. A cancelled local fallback retains its tombstone until inference acknowledges the terminal result.

- [x] **Step 5: Correct model activation, language ownership, replacement order, and budget fallback**

  Recheck the model-actor cancellation generation after `ensure_model`; store language outside `WhisperEngine`; drop context/state before constructing a replacement and report the resulting unloaded/error state if preparation fails; when network budget is exhausted, skip remaining network candidates but continue scanning for an enabled Local candidate.

- [x] **Step 6: Verify GREEN and record the memory probe command**

  Run: `cargo test -p sayflex-speech-worker -p rust-cloud-speech -- --nocapture`
  Expected: PASS.
  Record in the task ledger the release-build command used later to sample private bytes during repeated speech-model switches; do not record an unmeasured peak here.

  Recorded release probe for Task 16 (run while repeatedly switching installed speech models):
  `Get-Process -Name 'SayFlex.Speech.*' | Select-Object Id, ProcessName, PrivateMemorySize64, WorkingSet64`

- [x] **Step 7: Commit**

  `git commit -m "fix(speech): preserve operation ownership through completion"`

### Task 3: Make compact translation completion explicit and keep control commands responsive

**Files:**
- Modify: `rust-app/Cargo.toml`
- Modify: `rust-app/Cargo.lock`
- Create: `rust-app/vendor/fxtranslate/` from the exact admitted `fxtranslate 0.4.2` source, retaining its license and checksums
- Modify: `rust-app/crates/translation-engine/src/firefox.rs`
- Modify: `rust-app/crates/translation-engine/src/lib.rs`
- Create: `rust-app/apps/translation-worker/src/history_actor.rs`
- Modify: `rust-app/apps/translation-worker/src/history.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`
- Modify: `rust-app/THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Produces in the pinned engine fork: `DecodeTermination::{Eos, LengthLimit}` and checked direct/segmented/pivot APIs returning text plus per-segment termination.
- Produces in SayFlex: `FragmentCompletion::{Completed, LimitReached, Cancelled, Failed}` and `FirefoxTranslation.completion`.
- Produces: nonblocking `TranslationHistoryActor::{recent, set_enabled, append, clear, shutdown}` whose command lane never holds `WorkerState.engine`.
- Consumes: existing `Partial`, `Completed`, typed request failure, and automatic replacement gates.

- [x] **Step 1: Add failing engine tests for whitespace, EOS, cap, cancellation, and pivot propagation**

  Add tests for `Hello\n\n \t\n\nWorld`, Unicode whitespace, a decoder sequence that emits EOS, a decoder sequence that exhausts `max_len`, cancellation between fragments, and a pivot whose first or second leg reaches the cap. Name the production decision that would make each test fail.

- [x] **Step 2: Add a failing worker concurrency test**

  Start a controllably blocked translation operation, issue `GetHistory` and then `Cancel`, and assert Cancel is observed without waiting for the engine lock. Assert an incomplete compact result never emits `Completed` and cannot reach replacement.

- [x] **Step 3: Run focused tests and verify RED**

  Run: `cargo test -p rust-translation-engine -p sayflex-translation-worker -- --nocapture`
  Expected: whitespace range panic/cap ambiguity and history-lock responsiveness tests fail.

- [x] **Step 4: Pin the minimal completion-aware fxtranslate patch**

  Vendor the exact locked crate; change only decode result reporting so every loop records whether EOS ended decoding or the length cap did. Preserve existing public APIs for compatibility and add checked APIs used by SayFlex. Record the original crate checksum, patch diff, license, and update procedure.

- [x] **Step 5: Implement typed fragment completion and the history actor**

  Guard whitespace ranges before slicing; propagate checked termination through direct, segmented, and pivot paths; publish success only when every fragment is Completed. Move history I/O and reads behind its own actor so the stdin command loop never waits for the engine mutex.

- [x] **Step 6: Verify GREEN**

  Run: `cargo test -p rust-translation-engine -p sayflex-translation-worker -- --nocapture`
  Expected: PASS, including explicit length-limit and Cancel-after-GetHistory cases.

- [x] **Step 7: Commit**

  `git commit -m "fix(translation): report compact completion and isolate history"`

### Task 4: Correct Windows integration, installer safety, and resource telemetry

**Files:**
- Modify: `rust-app/installer/SayFlex.iss`
- Modify: `rust-app/scripts/tests/release-validation.Tests.ps1`
- Modify: `rust-app/apps/input-host/src/wezterm.rs`
- Modify: `rust-app/tests/fixtures/wezterm-selection-fixture.ps1`
- Modify: `rust-app/tests/fixtures/wezterm-insertion-fixture.ps1`
- Modify: `rust-app/crates/platform-windows/src/overlay.rs`
- Modify: `rust-app/crates/platform-windows/src/process_resources.rs`

**Interfaces:**
- Produces: installer helper `CanonicalInstallPath` comparison before legacy deletion.
- Produces: `run_cli` with one `Instant` deadline covering stdin writer, child lifetime, and concurrent bounded stdout/stderr drain.
- Produces: pure `fit_window_to_work_area(window_size, work_area, desired_origin) -> FittedWindow` used by both anchored and pointer placement.
- Produces: PDH wildcard setup following English-add, localized-info, localized-expand, localized-add; refreshable counter generation and explicit unavailable status.

- [x] **Step 1: Add failing tests for path equality, pipe pressure, small work areas, and localized counters**

  Extend release-validation tests with standard/custom paths including case, separators, and final-dot normalization. Add a fixture that blocks stdin while filling both output pipes. Add geometry tests where the card is larger than the work area. Extract PDH sequencing behind a test seam and assert the documented call order plus late-instance refresh and unavailable-not-zero.

- [x] **Step 2: Run focused tests and verify RED**

  Run: `cargo test -p sayflex-input-host wezterm::tests -- --nocapture`
  Expected: pipe/deadline tests fail.
  Run: `cargo test -p rust-platform-windows -- --nocapture`
  Expected: invalid clamp and PDH-sequence tests fail.
  Run: `pwsh -NoProfile -File scripts/tests/release-validation.Tests.ps1`
  Expected: same-directory StyleFlex cleanup scenario fails.

- [x] **Step 3: Implement owner-level fixes**

  Skip legacy deletion when normalized `{app}` equals a legacy path; service stdin/stdout/stderr concurrently under one deadline and kill/reap once on timeout; fit size before clamping coordinates; implement Microsoft's localized PDH wildcard sequence, periodic/empty refresh, and `None` for unavailable data.

- [ ] **Step 4: Verify on English plus Russian or German Windows**

  Run unit/integration tests on the development installation, then execute the GPU sampling fixture on one English and one Russian/German Windows environment. Record locale, counter availability, late process appearance, zero-load, and unavailable cases as separate evidence.

  Evidence 2026-09-27: the live fixture discovered GPU Engine instances on the available en-US system locale (en-NL user culture). Deterministic tests cover late-instance refresh, valid zero, and unavailable counters. A separate Russian or German Windows environment is still required before release closure.

- [x] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-input-host -p rust-platform-windows -- --nocapture`
  Expected: PASS.
  Run: `pwsh -NoProfile -File scripts/tests/release-validation.Tests.ps1`
  Expected: PASS.

- [x] **Step 6: Commit**

  `git commit -m "fix(windows): harden installer terminal and telemetry paths"`

### Task 5: Finish screen-capture, surface-readiness, and OCR termination ownership

**Files:**
- Modify: `rust-app/crates/platform-windows/src/screen_capture/gdi.rs`
- Modify: `rust-app/crates/platform-windows/src/screen_capture/dxgi.rs`
- Modify: `rust-app/crates/platform-windows/src/screen_capture.rs`
- Modify: `rust-app/apps/desktop/src/screen_translation.rs`
- Modify: `rust-app/apps/desktop/src/overlay_presenter.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Modify: `rust-app/apps/desktop/src/worker_supervisor.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`

**Interfaces:**
- Produces: `ScreenSelectionSurfaces::await_ready(deadline: Instant) -> Result<(), String>` and one retry-after-rebuild path for the current request.
- Produces: `OcrRequestState::{Running, CancelRequested, Terminated}`; only terminal worker events clear the active watchdog identity.
- Produces: bounded DXGI reuse entries with `captured_at` and `take_or_expire(now, ttl)`; request completion drops non-reused captures.
- Consumes: existing request ID, capture generation, overlay presenter frame ticket, OCR watchdog, and previous preview identity.

- [x] **Step 1: Add failing lifecycle tests**

  Cover a monitor/DPI change immediately before the first request; overlay hide confirmation before capture; pinned/current SayFlex surfaces excluded from the next frame; user Cancel followed by a stuck OCR worker; terminal OCR acknowledgement; DXGI cache expiry; and GDI read refusal until flush succeeds.

- [x] **Step 2: Run focused tests and verify RED**

  Run: `cargo test -p sayflex-desktop --features screen-translation screen_translation -- --nocapture`
  Expected: readiness and cancellation/watchdog tests fail on current behavior.
  Run: `cargo test -p rust-platform-windows screen_capture -- --nocapture`
  Expected: cache-lifetime or flush tests fail on current behavior.

- [x] **Step 3: Implement readiness and self-surface exclusion**

  Rebuild and asynchronously await surface HWND attachment within a bounded deadline; continue the same request. Retire/cloak current and pinned surfaces by their old identities, wait on presenter/DWM confirmation, then capture.

- [x] **Step 4: Separate user cancellation from compute termination**

  Send OCR Cancel and mark `CancelRequested` without clearing the watchdog. Clear only on `Cancelled`, `Recognized`, `Failed`, confirmed process exit, or forced watchdog termination. Reject stale instance/request events.

- [x] **Step 5: Bound capture resources and synchronize GDI**

  Call `GdiFlush` after successful `BitBlt` and before reading DIB bytes. Expire/drop cached CPU frames by the documented short reuse TTL and topology generation while preserving cached D3D devices/duplication objects; ensure protected content on one monitor does not poison unrelated output selection.

- [x] **Step 6: Verify GREEN**

  Run: `cargo test -p sayflex-desktop -p rust-platform-windows --features screen-translation -- --nocapture`
  Expected: PASS with no retained full-frame bytes after terminal/expiry tests.

- [x] **Step 7: Commit**

  `git commit -m "fix(screen): retain ownership through capture and OCR termination"`

### Task 6: Add block, context, completion, and glossary protocol contracts

**Files:**
- Modify: `rust-app/crates/protocol/src/lib.rs`
- Modify: `rust-app/apps/speech-worker/tests/protocol_contract.rs`
- Modify: `rust-app/crates/app-core/tests/worker_process_contract.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/crates/platform-windows/src/process_exit.rs`

**Interfaces:**
- Produces: `ScreenBlockId(String)` with non-empty bounded validation at consumers.
- Produces: `ScreenTextBlock { block_id, source_text, bounds, confidence_milli }`.
- Produces: `SourceContext { application_id, executable_name, window_title, site_url, document_name, nearby_text, domain, partial }` with optional bounded fields.
- Produces: `BlockCompletion::{Completed, LimitReached, Cancelled, Failed}` and `TranslatedScreenBlock`.
- Produces: `TranslationWorkerCommand::TranslateBlocks`, glossary CRUD/confirm commands, and corresponding partial/completed/list/saved/failed events; existing enum variants and JSON names remain compatible.

- [x] **Step 1: Add failing JSON round-trip and compatibility tests**

  Assert exact JSON names, omitted optional context, negative physical origins, duplicate/empty ID rejection at the validation layer, four completion states, glossary origin, and successful decoding of every previously persisted command/event fixture.

- [x] **Step 2: Run protocol tests and verify RED**

  Run: `cargo test -p rust-protocol -p rust-app-core worker_process_contract -- --nocapture`
  Expected: new contract tests fail because the types and variants do not exist.

- [x] **Step 3: Implement additive protocol types and size limits**

  Add the exact types above without image bytes. Bound block count, ID/text/context lengths at the worker command boundary; keep ordinary `Translate` and `TranslationResult` unchanged.

- [x] **Step 4: Verify GREEN**

  Run: `cargo test -p rust-protocol -p rust-app-core -- --nocapture`
  Expected: PASS, including old protocol fixture compatibility.

- [x] **Step 5: Commit**

  `git commit -m "feat(protocol): add contextual screen block contracts"`

### Task 7: Create the canonical revisioned screen document

**Files:**
- Create: `rust-app/apps/desktop/src/screen_result.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/crates/ocr-engine/src/lib.rs`
- Modify: `rust-app/crates/ocr-engine/src/reconstruct.rs`
- Modify: `rust-app/crates/ocr-engine/tests/reconstruct.rs`

**Interfaces:**
- Produces: `ScreenTranslationDocument { request_id, revision, crop, blocks, context, created_unix_ms, expires_unix_ms }`.
- Produces: `ScreenBlockState { source, bounds, confidence_milli, machine, user_edit }` keyed by `ScreenBlockId`.
- Produces: `apply_machine_blocks(request_id, base_revision, blocks)`, `save_user_edit(block_id, expected_revision, edit)`, `effective_translation(block_id)`, and `expire_pixels(now)`.
- Consumes: Task 6 protocol blocks and the selected `RgbaFrame`; pixels stay desktop-local.

- [x] **Step 1: Add failing document-state tests**

  Test unique/complete ID mapping, unknown/missing/duplicate output rejection, number/URL/placeholder preservation warnings, low-confidence flags, stale request/revision rejection, user edit precedence over a late machine result, corrected Copy projection, and pixel expiry without text loss.

- [x] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-desktop --features screen-translation screen_result -- --nocapture`
  Expected: FAIL because the document owner does not exist.

- [x] **Step 3: Implement the pure state owner**

  Keep all mutation methods deterministic and UI-free. Increment revision on accepted machine/user/persistence changes. Store the crop in an optional reference-counted buffer/path handle that can be dropped independently from block text.

- [x] **Step 4: Route OCR geometry into the document without flattening it**

  Generate stable request-local block IDs from OCR reading order, preserve geometry/confidence, and derive the existing plain source string/DUAL projection from document blocks rather than a second copy.

- [x] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-desktop --features screen-translation screen_result -- --nocapture`
  Expected: PASS.

- [x] **Step 6: Commit**

  `git commit -m "feat(screen): add canonical revisioned translation document"`

### Task 8: Snapshot visible source windows and collect bounded context

**Files:**
- Create: `rust-app/crates/platform-windows/src/window_snapshot.rs`
- Modify: `rust-app/crates/platform-windows/src/lib.rs`
- Create: `rust-app/apps/desktop/src/source_context.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`

**Interfaces:**
- Produces: `WindowInventory::capture(generation: u64, deadline: Instant) -> Result<Self, WindowSnapshotError>`.
- Produces: `WindowInventory::source_for(selection: PhysicalRect) -> Option<WindowSnapshot>` using saved regions/Z order only.
- Produces: `ContextCollector::from_snapshot(source: &WindowSnapshot) -> SourceContext` with bounded executable/title/UIA fields captured within the pre-overlay deadline.
- Consumes: capture generation from Task 7 and Task 6 `SourceContext`.

- [x] **Step 1: Add failing pure geometry/occlusion tests**

  Cover the review-focus game-over-browser case, peer overlap, shaped region, negative monitor origin, cloaked/tool/SayFlex exclusion, majority winner, center plus 1.5 lead, and ambiguous split returning `None`.

- [x] **Step 2: Add failing context tests**

  Cover browser without URL, PDF without book metadata, stale/reused HWND identity, command-like OCR/title text treated as data, and a context timeout that still permits translation.

- [x] **Step 3: Run and verify RED**

  Run: `cargo test -p rust-platform-windows window_snapshot -- --nocapture`
  Expected: FAIL because the snapshot module does not exist.
  Run: `cargo test -p sayflex-desktop --features screen-translation source_context -- --nocapture`
  Expected: FAIL because the context module does not exist.

- [x] **Step 4: Implement the frozen inventory and context collector**

  Enumerate eligible top-level windows before overlays, save process identity, extended bounds/window region, cloaking, Z order, and available bounded metadata. Reconstruct visible regions by subtracting saved higher windows. Never query current foreground state to choose the source after selection; omit optional URL/document fields on unavailable/timeout.

- [x] **Step 5: Integrate inventory capture with the same background capture generation**

  Return pixels plus inventory as one capture preparation result. Selection later consumes only that immutable pair. Diagnostics record counts/timing and typed omissions, never titles, URLs, or nearby text.

- [x] **Step 6: Verify GREEN**

  Run: `cargo test -p rust-platform-windows -p sayflex-desktop --features screen-translation -- --nocapture`
  Expected: PASS.

- [x] **Step 7: Commit**

  `git commit -m "feat(screen): capture visible source-window context"`

### Task 9: Add temporary screenshot persistence and cleanup

**Files:**
- Create: `rust-app/apps/desktop/src/screenshot_store.rs`
- Modify: `rust-app/apps/desktop/src/settings_document.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/Cargo.toml`
- Create: `rust-app/crates/platform-windows/src/private_directory.rs`
- Modify: `rust-app/crates/platform-windows/src/lib.rs`
- Modify: `rust-app/Cargo.toml`

**Interfaces:**
- Produces: `ScreenshotRetention { enabled: bool, ttl: Duration, max_bytes: u64 }` defaulting to enabled, 24 hours, and 500 MiB.
- Produces: `ScreenshotStore::{open, persist_crop, load_crop, delete_now, save_permanent, cleanup}`.
- Produces: metadata records with request ID, dimensions, block geometry/translations, created/expiry time, and relative crop filename; no composited result bitmap.
- Consumes: Task 7 canonical documents; stores tightly bounded PNG only for the selected crop.

- [x] **Step 1: Add failing storage tests**

  Test atomic crop/metadata write, no full-frame API, expiry at 24 hours, oldest-first 500 MiB eviction, startup orphan cleanup, disabled retention, delete-now, pinned/open expiry releasing pixels, permanent Save file, restrictive directory ACLs, and cleanup failure not blocking the caller.

- [x] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-desktop --features screen-translation screenshot_store -- --nocapture`
  Run: `cargo test -p sayflex-desktop --features screen-translation settings_document -- --nocapture`
  Expected: FAIL because storage/settings fields do not exist.

- [x] **Step 3: Implement per-user atomic storage**

  Use `%LOCALAPPDATA%\SayFlex\screen-translations`, create a protected DACL for the current user plus Windows system/administrators, use temporary file plus atomic replace, checked dimensions/byte limits, relative filenames, and content-free diagnostics. Never accept a full `DesktopCapture`; accept only the selected `RgbaFrame`.

- [x] **Step 4: Wire startup/runtime cleanup and settings migration**

  Add missing-field defaults without changing settings schema meaning. Run cleanup on startup and a bounded timer; update active documents when their files expire. Disabling future history stops new writes and offers explicit deletion separately.

- [x] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-desktop --features screen-translation screenshot_store -- --nocapture`
  Run: `cargo test -p sayflex-desktop --features screen-translation settings_document -- --nocapture`
  Expected: PASS.

- [x] **Step 6: Commit**

  `git commit -m "feat(screen): retain selected crops with bounded cleanup"`

### Task 10: Add independent personal-glossary persistence and retrieval

**Files:**
- Create: `rust-app/apps/translation-worker/src/glossary.rs`
- Create: `rust-app/apps/translation-worker/src/glossary_actor.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`
- Modify: `rust-app/crates/app-core/tests/worker_process_contract.rs`

**Interfaces:**
- Produces: `GlossaryEntry { id, source_phrase, approved_translation, source_language, target_language, examples, context, origin, created_unix_ms, updated_unix_ms }`.
- Produces: `GlossaryActor::{confirm, save_correction, list, update, delete, lookup, shutdown}`.
- Produces: deterministic merge key `(normalized source phrase, language pair, normalized domain scope)`; reconfirmation appends a deduplicated example.
- Consumes: Task 6 glossary protocol; stores independently at `%LOCALAPPDATA%\SayFlex\translation-glossary.json`.

- [x] **Step 1: Add failing glossary-domain and persistence tests**

  Assert machine completion/Copy/Close write nothing; Save correction and Confirm write; Dota and Counter-Strike meanings coexist; reconfirmation merges examples; clear history/screenshot deletion leaves entries; edit/delete survive restart; corrupt primary recovers/quarantines; failed write retains the caller's corrected text and returns a retryable operation error.

- [x] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-translation-worker glossary -- --nocapture`
  Expected: FAIL because glossary storage/actor do not exist.

- [x] **Step 3: Implement atomic actor-owned storage and ranking**

  Serialize all glossary writes through one actor. Rank lookup by exact language pair, normalized expression, exact domain/application, then general scope; return bounded structured matches without substring mutation.

- [x] **Step 4: Wire commands/events without blocking translation control**

  Glossary list/write/read work must not acquire the active engine mutex in the stdin loop. Every save/confirm carries operation ID, request ID, block ID, and expected revision and returns saved/error explicitly.

- [x] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-translation-worker glossary -- --nocapture`
  Expected: PASS.

- [x] **Step 6: Commit**

  `git commit -m "feat(translation): add domain-aware personal glossary"`

### Task 11: Translate screen blocks with structured context and glossary guidance

**Files:**
- Create: `rust-app/apps/translation-worker/src/block_translation.rs`
- Modify: `rust-app/apps/translation-worker/src/provider_actor.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`
- Modify: `rust-app/crates/translation-engine/src/engine.rs`
- Modify: `rust-app/crates/translation-engine/src/firefox.rs`
- Modify: `rust-app/crates/cloud-translation/src/router.rs`

**Interfaces:**
- Produces: `BlockTranslationPlan { blocks, context, glossary_matches, backend_capabilities }`.
- Produces: Qwen structured request/response schema mapping exact `block_id` to text; no context text is concatenated into source strings.
- Produces: capability flags `structured_blocks`, `context_instructions`, and `glossary_guidance` surfaced in completed metadata.
- Consumes: Task 6 protocol and Task 10 glossary lookup.

- [ ] **Step 1: Add failing structured translation tests**

  Cover reordered output, missing/duplicate/unknown IDs, JSON/prose injection inside OCR/context, numbers/URLs/placeholders, two related blocks sharing context, different domain glossary matches, late cancellation, compact limited-capability mode, and cloud provider without contextual instruction support.

- [ ] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-translation-worker block_translation -- --nocapture`
  Expected: FAIL because only plain-string translation exists.
  Run: `cargo test -p rust-translation-engine -p rust-cloud-translation -- --nocapture`
  Expected: FAIL on the new completion/capability adapter contracts.

- [ ] **Step 3: Implement Qwen structured block translation**

  Place immutable translation instructions in the system message and a typed JSON envelope in the user message. Require strict JSON output, validate the exact ID set and protected values, use deterministic generation, and retry malformed/incomplete groups only within a bounded policy.

- [ ] **Step 4: Implement honest limited-mode adapters**

  Compact and ordinary cloud APIs preserve block identity with separate bounded requests or deterministic separators but advertise no instruction-based glossary/context support. Never prepend service text to the source. Propagate Task 3 fragment completion per block.

- [ ] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-translation-worker -p rust-translation-engine -p rust-cloud-translation -- --nocapture`
  Expected: PASS.

- [ ] **Step 6: Commit**

  `git commit -m "feat(translation): preserve screen blocks and contextual terms"`

### Task 12: Integrate the canonical document with the screen-translation coordinator

**Files:**
- Modify: `rust-app/apps/desktop/src/screen_translation.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Modify: `rust-app/apps/desktop/src/worker_supervisor.rs`

**Interfaces:**
- Produces: `ScreenTranslationCoordinator` as the single desktop owner for capture preparation, selection, OCR, context, document revision, block translation, persistence, cancellation, and terminal publication.
- Produces: ordinary text/DUAL projection derived from Task 7 document, preserving existing preview registry and pin behavior.
- Consumes: Tasks 5 through 11.

- [ ] **Step 1: Add failing end-to-end coordinator tests with fake boundaries**

  Cover capture to OCR to block translation to text projection; supersede/cancel at each stage; missing URL; ambiguous source; OCR correction; persistence disabled/failure; late block result after edit; worker restart; and image expiry while pinned.

- [ ] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-desktop --features screen-translation screen_translation -- --nocapture`
  Expected: new coordinator scenarios fail against the current `main.rs` orchestration.

- [ ] **Step 3: Extract orchestration from `main.rs` and wire real workers**

  Keep Slint callbacks/event dispatch in `main.rs`, but move request state/transitions to the coordinator. Do not remove old toolbar behavior. Plain translation requests continue through the existing path unchanged.

- [ ] **Step 4: Verify GREEN**

  Run: `cargo test -p sayflex-desktop --features screen-translation -- --nocapture`
  Expected: PASS.

- [ ] **Step 5: Commit**

  `git commit -m "refactor(screen): centralize contextual translation ownership"`

### Task 13: Build and obtain approval for the five visual states

**Files:**
- Create: `rust-app/apps/desktop/ui/screen_translation.slint`
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `rust-app/apps/desktop/src/settings_preview.rs`
- Create: `rust-app/apps/desktop/src/screen_translation_preview.rs`

**Interfaces:**
- Produces preview-only `ScreenImageResult`, `TranslationCorrectionPanel`, and deterministic fixtures for image result, editing, saved, save error, and expired image.
- Consumes: current Theme/Sheet tokens and Task 7 document projection; preview mode registers no hotkeys and starts no workers.

- [ ] **Step 1: Add failing structural UI tests**

  Assert the preview component exposes `ORIG`, `DUAL`, `ON IMAGE`, `PIN`, Correct, Save, Confirm, retryable save error, expiry copy, accessible labels, readable minimum font, callout fallback, and focusable editor/non-activating result separation.

- [ ] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-desktop screen_translation_preview -- --nocapture`
  Expected: FAIL because components/fixtures do not exist.

- [ ] **Step 3: Implement preview-only Slint states**

  Render the crop plus block overlays using `Image + Rectangle + Text`; wrap rather than shrinking below the readable minimum; create callouts for overflow; highlight the selected block; keep editor inside the work area; show corrected text in ordinary color.

- [ ] **Step 4: Render screenshots at representative DPI and sizes**

  Produce the five required screenshots at 100%, 125%, and 200% DPI, including long translation and low-confidence OCR. Inspect every artifact before presenting it.

- [ ] **Step 5: HARD STOP — obtain user visual approval**

  Show the live preview/screenshots to the user. Do not wire the production callbacks in Task 14 until the user approves or requests revisions.

- [ ] **Step 6: Verify approved preview and commit**

  Run: `cargo test -p sayflex-desktop screen_translation_preview -- --nocapture`
  Expected: PASS after approved revisions.
  Commit: `git commit -m "feat(ui): prototype contextual screen translation states"`

### Task 14: Wire on-image rendering, correction, glossary management, and screenshot controls

**Files:**
- Modify: `rust-app/apps/desktop/ui/screen_translation.slint`
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `rust-app/apps/desktop/ui/pages_lists.slint`
- Modify: `rust-app/apps/desktop/ui/pages_forms.slint`
- Create: `rust-app/apps/desktop/src/screen_result_view.rs`
- Create: `rust-app/apps/desktop/src/translation_editor.rs`
- Create: `rust-app/apps/desktop/src/glossary_view.rs`
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Modify: `rust-app/apps/desktop/src/localization.rs`
- Modify: every `rust-app/apps/desktop/translations/*/LC_MESSAGES/sayflex-desktop.po`

**Interfaces:**
- Produces: production callbacks for view mode, block selection, Correct, Save, Confirm, retry, Copy, Delete now, Save file, glossary list/edit/delete, and screenshot-retention settings.
- Produces: editor draft key `(request_id, block_id, revision)` and focus-transfer/restore contract.
- Consumes: Tasks 9, 10, 12, and the visually approved Task 13 components.

- [ ] **Step 1: Add failing interaction and localization tests**

  Test `Ctrl+Enter`, Escape, outside click preservation, late result after edit, failed glossary write retry, corrected Copy, no retroactive external edit, full glossary CRUD, screenshot disclosure/delete, expiry fallback, and complete strings in all `UiLocale::ALL` locales.

- [ ] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-desktop --features screen-translation translation_editor -- --nocapture`
  Run: `cargo test -p sayflex-desktop --features screen-translation glossary_view -- --nocapture`
  Run: `cargo test -p sayflex-desktop --features screen-translation screen_result_view -- --nocapture`
  Run: `cargo test -p sayflex-desktop --features screen-translation localization -- --nocapture`
  Expected: FAIL because the corresponding production bindings do not exist.

- [ ] **Step 3: Wire approved result/editor components**

  Keep result windows non-activating. Create a focusable editor only after explicit Correct, preserve the external target snapshot, and apply saved revisions to document/text/image projections. Keep `ORIG`, `DUAL`, copy, and pin behavior intact.

- [ ] **Step 4: Add glossary and screenshot-history settings surfaces**

  Use existing Sheet list/form patterns with search, edit, delete, expiry explanation, 24-hour/500-MiB defaults, Disable future storage, Delete now, and Save file. Do not combine screenshot retention with dictation/translation history retention.

- [ ] **Step 5: Verify GREEN**

  Run: `cargo test -p sayflex-desktop --features screen-translation -- --nocapture`
  Expected: PASS.
  Run: `pwsh -NoProfile -File scripts/tests/ui-translations.Tests.ps1`
  Expected: PASS for every shipped locale.

- [ ] **Step 6: Commit**

  `git commit -m "feat(ui): edit glossary and render translation on images"`

### Task 15: Enforce aggregate resource policy and benchmark the optional model candidate

**Files:**
- Modify: `rust-app/apps/desktop/src/resource_view.rs`
- Modify: `rust-app/apps/desktop/src/worker_supervisor.rs`
- Modify: `rust-app/apps/desktop/src/localization.rs`
- Modify: every `rust-app/apps/desktop/translations/*/LC_MESSAGES/sayflex-desktop.po`
- Modify: `rust-app/tools/translation-benchmark/src/main.rs`
- Create: `rust-app/docs/evidence/contextual-translation-resources.md`
- Create only if admitted: pinned HY-MT2 catalog/notice entries in the existing translation catalog and `THIRD_PARTY_NOTICES.md`

**Interfaces:**
- Produces: aggregate resource snapshot over desktop/input/speech/translation/OCR children, with per-feature rows and unavailable-vs-zero semantics.
- Produces: idle eviction decisions that never unload an active operation and explain automatic reload.
- Consumes: corrected Task 4 telemetry and existing unload commands.

- [ ] **Step 1: Add failing aggregate-policy tests**

  Test descendant-process aggregation, late llama-server/OCR appearance, unavailable GPU data, active-operation protection, manual unload, idle unload, and automatic reload on next use.

- [ ] **Step 2: Run and verify RED**

  Run: `cargo test -p sayflex-desktop resource_view -- --nocapture`
  Expected: aggregate/policy tests fail where only current per-worker sampling exists.
  Run: `cargo test -p rust-platform-windows process_resources -- --nocapture`
  Expected: late-instance and unavailable-vs-zero telemetry tests fail.

- [ ] **Step 3: Implement aggregate residency policy and localized explanations**

  Report CPU, RAM, GPU, and VRAM separately; never label unavailable as zero. Keep OCR lazy and CPU-only. Protect active operations from unload and reload selected engines automatically on demand.

- [ ] **Step 4: Run the release resource matrix with a game active**

  Measure Qwen 4B plus speech plus OCR cold/warm/idle/peak RAM, private bytes, GPU, VRAM, latency, unload/reload, and game frame impact. Fail the gate above 10 GB VRAM; record whether the 8 GB target is met.

- [ ] **Step 5: Benchmark HY-MT2 without blocking the core release**

  Verify official GGUF hash/license/template/runtime compatibility, then compare RU to EN and EN to RU quality, glossary adherence, protected values, latency, RAM/VRAM, and cancellation with Qwen 4B. Add it to the catalog only if admitted; otherwise record it as deferred with evidence.

- [ ] **Step 6: Verify GREEN**

  Run: `cargo test -p sayflex-desktop -p rust-platform-windows -- --nocapture`
  Expected: PASS and evidence document contains measured values rather than estimates.

- [ ] **Step 7: Commit**

  `git commit -m "feat(resources): manage aggregate SayFlex model residency"`

### Task 16: Update documentation, run full verification, and build the installed candidate

**Files:**
- Modify: `rust-app/README.md`
- Modify: `rust-app/SECURITY.md`
- Modify: `rust-app/docs/TRANSLATION_ARCHITECTURE.md`
- Modify: `rust-app/docs/THIRD_PARTY_MODELS.md`
- Modify: `rust-app/THIRD_PARTY_NOTICES.md`
- Modify: `docs/evidence/screen-translation-ocr-benchmark.md`
- Create: next `rust-app/docs/RELEASE_*.md`
- Modify: `rust-app/scripts/tests/release-validation.Tests.ps1`
- Modify: installer/package manifests selected by the existing release process

**Interfaces:**
- Consumes: all previous tasks and the approved visual evidence.
- Produces: one installable candidate plus a final report separated into Implemented, Verified, Not verified, and Deferred.

- [ ] **Step 1: Update security/product/model/release documentation**

  Document screenshot retention and deletion, optional context fields, local/cloud boundaries, glossary semantics, compact-mode limitations, dependencies, model licenses, resource measurements, and recovery behavior after unload.

- [ ] **Step 2: Run the OCR benchmark through the shipped application path**

  Use the checked-in corpus and worker protocol used by the application. Record accuracy, omissions, numbers/symbols, latency, memory, languages, dark/light/small/column cases, and worker restart behavior.

- [ ] **Step 3: Run formatting, lint, and all automated tests**

  Run: `cargo fmt --all -- --check`
  Expected: PASS.
  Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  Expected: PASS.
  Run: `cargo test --workspace --all-features -- --nocapture`
  Expected: PASS.
  Run: `pwsh -NoProfile -File scripts/tests/ui-translations.Tests.ps1`
  Expected: PASS.
  Run: `pwsh -NoProfile -File scripts/tests/release-validation.Tests.ps1`
  Expected: PASS.

- [ ] **Step 4: Build release binaries and installer**

  Use the repository's pinned release script/manifest. Validate executable versions, hashes, required runtime/model manifests, absence of unapproved bundled models, installer/uninstaller integrity, and the existing Store packaging path.

- [ ] **Step 5: Execute real installed Windows scenarios**

  Upgrade over the previous installed release from standard and custom paths; verify uninstall and Store-package preservation of settings, history, downloaded assets, and licenses. Exercise clipboard text/images and competitive Copy; five-minute limit recovery; local/cloud cancellation; model switches; WezTerm timeout; monitor/DPI changes; source context; no-URL/no-book fallback; correction/confirmation glossary rules; domain conflicts; restart persistence; screenshot expiry/eviction/delete; long on-image translations; OCR worker hang/restart; Russian/German GPU counters; and resource budget with a game.

- [ ] **Step 6: Capture final approved UI evidence**

  Capture image result, editing, saved, save error, and expired image from the installed candidate at representative DPI. Compare them with the approved Task 13 states.

- [ ] **Step 7: Write the evidence-separated completion report**

  For every spec item list the original scenario, change, verification, and result. Label every unrun/manual/environment-specific gate as Not verified; list rejected model experiments and future VLM/continuous-overlay work as Deferred.

- [ ] **Step 8: Commit**

  `git commit -m "chore(release): validate contextual screen translation update"`
