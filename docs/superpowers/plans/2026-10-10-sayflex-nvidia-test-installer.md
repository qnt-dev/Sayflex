# SayFlex optional NVIDIA test installer implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development and TDD. User approved the previously discussed design and explicitly requested a usable test installer on 2026-10-10. Continue implementation without another approval round.

**Goal:** Deliver a local unsigned installer that the user can install with fresh settings and explicitly add NVIDIA acceleration.

**Architecture:** Keep CPU/Vulkan and the open-source CUDA speech worker. Download a pinned official NVIDIA runtime only after a visible user action, verify it, install atomically and activate when the speech worker is idle. A marker beside the executables selects an isolated test profile and install identity, without changing the normal release path.

**Tech stack:** Rust, Slint 1.17.1, existing model-manager HTTP/cancellation machinery, SHA-256 and ZIP, PowerShell and Inno Setup.

**Spec:** Approved in-chat design: separate runtime, explicit consent, progress/cancel/retry, real CUDA verification, isolated fresh user trial; no SignPath approval or license change is claimed.

## Global constraints
- Preserve existing uncommitted reliability work; baseline copied to `C:\Users\nksep\AppData\Local\Temp\sayflex-nvidia-test-baseline-20261010-123818`.
- Current existing feature worktree is reused. No commits, public release, support messages or installed production replacement.
- Test channel: `sayflex-test-channel` marker, data `%LOCALAPPDATA%\SayFlex-Test-Nvidia`, own installer ID/path/shortcuts, no legacy-data migration or system CUDA auto-discovery.
- Download only official NVIDIA HTTPS artifacts; pin version, size and SHA-256, retain license material. Do not add Python or a full Toolkit dependency.
- Existing CPU/Vulkan fallback remains. Installed runtime is distinct from model loaded/working CUDA.
- Keep current licenses while awaiting support; artifact is explicitly a local unsigned test candidate, not a Foundation-compliant release.

## Review focus
- Corrupt/truncated archive and path traversal cannot become an executable runtime.
- Cancellation, offline failures and retries remain usable without blocking the UI.
- Installing/activating acceleration cannot interrupt a recording or model installation.
- Test data and shortcuts cannot reuse or modify the production profile.
- Installed status must not falsely report active CUDA; the real worker must load and warm up a model.

## Task A: runtime download and integrity (input_performance)
- [x] Write failing fixture tests for corrupted payload, unsafe ZIP, cancellation/retry and incomplete installed contents.
- [x] Add `CudaRuntimeManager` in model-manager; manager root is `product::data_root().join("runtimes")`.
- [x] API: new(PathBuf), runtime_path(), is_installed(), ensure_installed(fetcher, cancellation, progress). Export CUDA_DOWNLOAD_BYTES and CUDA_LICENSE_URL.
- [x] Verify official download and actual extracted files; run model-manager tests.

## Task B: consent and status UI (ui_review)
- [x] Test explicit consent, cancellation and distinction between installed/active states.
- [x] Add NVIDIA card to Dictation settings, license link, progress, cancel/retry and activate actions; first-run explains where to enable it.
- [x] Expose `wire_cuda_setup(&SettingsWindow, Rc<RefCell<AppState>>, activate closure) -> CudaSetupController`; background I/O, retained UI timer, cancellation on drop.
- [x] Complete Russian/English strings; verify Slint accessibility/layout.

## Task C: isolated installer (release_security)
- [ ] Build a separate test installer with marker and without NVIDIA DLL payload; preserve production release gates.
- [ ] Emit local-test manifest/source identity and checksum; isolate shortcuts and uninstall identity.
- [ ] Validate installer contents and installation behaviour without changing the production installation.

## Task D: integration and delivery (root)
- [x] Test and implement isolated profile selection and disabled legacy migration.
- [x] Add managed CUDA location to supervisor; test builds ignore system Toolkit; integrate UI activation through existing safe worker switch and initialization.
- [x] Run focused checks, workspace tests, fmt and clippy, then whole-change independent review.
- [ ] Build final candidate, verify payload and a real CUDA worker with downloaded runtime; compare timings where feasible.
- [ ] Deliver installer path, checksum, short clean-install walkthrough and precise remaining user acceptance gates.

## Progress / decisions
- Existing 49-file dirty baseline is protected by a copied snapshot; no reset/stash.
- Separate test profile avoids asking the user to remove their working installation or private data.
- Runtime package and activation integration details will be recorded as verified, not assumed.

- 2026-10-10 integration gate: workspace tests, fmt, all-targets clippy, dependency advisories/licenses passed; independent read-only review found no new blocker. Packaging script suite: 47 passed, 1 intentional live-clipboard skip. Final installer/runtime acceptance is recorded outside source to preserve the final build snapshot.

- Native QA: fresh-profile first-run rendered and explicit opt-out persisted; actual pinned CUDA worker passed 3/3 isolated GPU transcription runs with equal text and verified module paths (128-132 ms inference). User reported intermittent scroll jitter in both installed and test UI, stronger in test. Cause remains unproven; existing scroll source equals installed snapshot.
- Packaging correction: workspace feature unification had linked PP-OCR/ONNX/DirectML into desktop. Desktop now receives a separate Cargo build before staging; actual PE import validation prevents recurrence. Script suite 51 passed, 1 intentional live clipboard skip. Rebuild required; objective reduced footprint is distinct from proving scroll performance.
