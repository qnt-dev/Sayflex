# SayFlex Release Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce one verified SayFlex release candidate implementing the frozen 2026-09-10 hardening scope.

**Architecture:** Slint remains a declarative projection. Rust owners hold hotkey, locale, provider-draft, cancellation, and persistence state behind typed contracts. Worker control uses bounded priority mailboxes and no shared mutex is held across network I/O.

**Tech Stack:** Rust 1.88, Slint 1.17, Windows API, serde, gettext PO catalogs, PowerShell, Inno Setup, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-10-sayflex-release-hardening-design.md`

## Global Constraints

- Work on `feature/release-hardening`, never directly on `main`.
- Add each behavior through a witnessed RED then GREEN test.
- Preserve clipboard insertion behavior and unknown provider configuration.
- Do not claim public readiness for an unsigned artifact.
- Do not add UI locales outside the approved nine plus System.

---

### Task 1: Restore native Slint scrolling

**Files:**
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Modify: `rust-app/apps/desktop/ui/settings.slint`
- Test: `rust-app/apps/desktop/src/settings_view.rs`

**Interfaces:**
- Consumes: Slint viewport properties already exposed by `SettingsWindow`.
- Produces: `clamp_scroll_position(current, content, shown) -> f32` only for rail projection; wheel input remains owned by Slint.

- [ ] Add a regression test showing that rail clamping does not synthesize wheel increments or accumulate an animation target.
- [ ] Run `cargo test -p sayflex-desktop settings_view::list_wheel_tests` and confirm the legacy interception expectation fails.
- [ ] Remove `install_precise_wheel`, its installation call, and the `animate ...scroll-y` block. Keep viewport clamping and rail drag.
- [ ] Run the focused test and the desktop test suite.
- [ ] Commit as `fix: restore native settings scrolling`.

### Task 2: Make hotkeys typed, editable, atomic, and persistent

**Files:**
- Modify: `rust-app/crates/protocol/src/lib.rs`
- Create: `rust-app/crates/app-core/src/hotkeys.rs`
- Modify: `rust-app/crates/app-core/src/lib.rs`
- Modify: `rust-app/apps/input-host/src/hotkeys.rs`
- Modify: `rust-app/apps/input-host/src/main.rs`
- Modify: `rust-app/apps/desktop/src/settings_document.rs`
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Modify: `rust-app/apps/desktop/ui/rows.slint`
- Modify: `rust-app/apps/desktop/ui/pages_activity.slint`
- Modify: `rust-app/apps/desktop/ui/settings.slint`

**Interfaces:**
- Produces: `HotkeyAction`, `HotkeyModifiers`, `HotkeySpec`, `HotkeyBindings`, `InputHostCommand::SetHotkeys`, `InputHostEvent::HotkeysApplied`, and `InputHostEvent::HotkeysRejected`.
- Consumes: Windows `RegisterHotKey`; the dictation hook reads an atomically replaced `HotkeyBindings` snapshot.

- [ ] Add protocol round-trip tests for all hotkey values and rejection payloads; run them RED before adding the types.
- [ ] Add app-core validation tests for duplicate chords, modifier-only chords, unsupported keys, defaults, and migration; run RED.
- [ ] Implement canonical types and pure validation until focused tests pass.
- [ ] Add input-host tests proving atomic rollback when the third registration fails, feature-disable unregisters the action, and pause suppresses every action except resume; run RED.
- [ ] Introduce a registration adapter around `RegisterHotKey`, implement staged registration plus rollback, and update the LL hook snapshot; run GREEN.
- [ ] Add desktop persistence tests proving old documents receive defaults and rejected bindings are never saved; run RED then implement.
- [ ] Add keyboard capture, per-row edit/reset controls, conflict feedback, and matching callbacks. Persist only after `HotkeysApplied`.
- [ ] Run protocol, app-core, input-host, and desktop suites.
- [ ] Commit as `feat: make global shortcuts configurable`.

### Task 3: Localize every user-facing surface

**Files:**
- Modify: `rust-app/apps/desktop/build.rs`
- Create: `rust-app/apps/desktop/src/localization.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/settings_document.rs`
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Modify: `rust-app/apps/desktop/src/settings_projection.rs`
- Modify: `rust-app/apps/desktop/ui/*.slint`
- Create: `rust-app/apps/desktop/translations/{ru,uk,es,pt-BR,de,fr,it,ko}/LC_MESSAGES/sayflex-desktop.po`
- Modify: `rust-app/installer/SayFlex.iss`

**Interfaces:**
- Produces: `UiLocale::{System,En,Ru,Uk,Es,PtBr,De,Fr,It,Ko}`, `UiText`, `resolve_system_locale`, and `text(UiLocale, UiText) -> &'static str`.
- Consumes: `slint::select_bundled_translation` after component creation; a locale change rebuilds Rust-projected models.

- [ ] Add locale parsing, system fallback, completeness, and persistence tests; run RED.
- [ ] Implement the typed locale catalog and settings migration; run GREEN.
- [ ] Wrap every static Slint literal in `@tr`, enable bundled translations, and add complete PO catalogs. Add a build-time test that every approved catalog has every message id.
- [ ] Replace Rust-authored visible prose with `UiText` lookups; keep diagnostic logs in English.
- [ ] Add the language selector and refresh all projections after a successful bundle switch.
- [ ] Localize installer tasks and messages using Inno language sections.
- [ ] Run `cargo test -p sayflex-desktop` once per bundled locale under the testing backend.
- [ ] Commit as `feat: localize the SayFlex interface`.

### Task 4: Make provider configuration and cancellation deterministic

**Files:**
- Create: `rust-app/crates/app-core/src/provider_drafts.rs`
- Modify: `rust-app/crates/app-core/src/lib.rs`
- Modify: `rust-app/apps/desktop/src/providers_view.rs`
- Modify: `rust-app/crates/cloud-translation/src/provider.rs`
- Modify: `rust-app/crates/cloud-translation/src/chain.rs`
- Modify: `rust-app/apps/speech-worker/src/transcription.rs`
- Create: `rust-app/apps/speech-worker/src/transcription_mailbox.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`
- Create: `rust-app/apps/translation-worker/src/provider_actor.rs`

**Interfaces:**
- Produces: `ProviderDraftStore`, `SessionCancellationSet`, `TranscriptionMailbox`, and `ProviderActorHandle`.
- `TranscriptionMailbox` has bounded work capacity, a priority control lane, and `latest pending finalize wins` overload semantics.
- `ProviderActorHandle` accepts `Translate`, `Cancel`, `SetProviders`, `SetCredential`, `GetStatus`, and `Shutdown`; results carry request generation.

- [ ] Add draft-store tests proving reorder/status refresh preserves key and extra-field text and only matching ACK clears it; run RED then implement.
- [ ] Add Azure tests proving blank region is accepted and only a non-empty region emits the header. Remove Google from known/default entries while preserving an unknown stored `google` row; run RED then implement.
- [ ] Replace hard-coded commercial allowances with neutral localized capability text.
- [ ] Add speech cancellation tests for overlapping A/B sessions, bounded overload, and control response while request work is blocked; run RED.
- [ ] Implement per-session tombstones and the bounded priority mailbox; run GREEN.
- [ ] Add translation actor tests proving a blocked request cannot delay credential/status handling, superseded results are discarded, and shutdown is bounded; run RED.
- [ ] Move provider ownership into one actor and remove per-request thread creation and network-held shared mutexes; run GREEN.
- [ ] Run cloud speech/translation and both worker suites.
- [ ] Commit as `fix: isolate provider control from network work`.

### Task 5: Make direction, placement, settings, and credentials recoverable

**Files:**
- Modify: `rust-app/crates/protocol/src/lib.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/ui/translation.slint`
- Modify: `rust-app/apps/desktop/src/settings_document.rs`
- Modify: `rust-app/crates/app-core/src/credentials.rs`
- Modify: `rust-app/crates/platform-windows/src/*`

**Interfaces:**
- Produces: explicit `RetryTranslation { source_language, target_language }`, `TranslationAnchor`, and `SettingsStorage` with `load_recovering`/`replace_atomically`.

- [ ] Add direction tests showing one-word auto results are visible and an explicit swap/retry carries exact languages; run RED then implement popup controls and command flow.
- [ ] Add anchor tests for selection, caret, pointer fallback, and monitor clamping; run RED then implement the fallback chain.
- [ ] Add storage fault tests for missing primary, corrupt primary, valid previous, valid temporary, and interrupted replace; run RED.
- [ ] Implement Windows atomic replacement and newest-valid startup recovery; retain corrupt candidates with diagnostic names.
- [ ] Add credential tests distinguishing `NotFound` from access/I/O failure and correct misleading comments/copy.
- [ ] Run app-core, platform, protocol, and desktop suites.
- [ ] Commit as `fix: recover translation and settings state`.

### Task 6: Finish previews, accessibility, and license attribution

**Files:**
- Modify: `rust-app/apps/desktop/src/visualizer.rs`
- Modify: `rust-app/apps/desktop/ui/pages_instruments.slint`
- Modify: `rust-app/apps/desktop/ui/settings.slint`
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `THIRD_PARTY_NOTICES.md`
- Create: `rust-app/scripts/generate-third-party-notices.ps1`

**Interfaces:**
- Produces: a top-level About page, live transparent previews for every enabled visual, and keyboard/accessibility metadata for every custom control.

- [ ] Add visual mapping tests that fail if an enabled selectable style has no live preview source; run RED and replace the Companion Orb placeholder with a live renderer or disable it explicitly.
- [ ] Add UI testing-backend checks for transparent preview surfaces and Broadcast Mic palette persistence.
- [ ] Add About navigation and render `AboutSlint`; correct the notice claim.
- [ ] Add accessible roles, labels, enabled state, focus order, and default actions to custom controls; verify keyboard traversal in the testing backend.
- [ ] Generate a dependency license inventory from Cargo metadata and licenses; make the script fail on an unknown license.
- [ ] Run desktop tests and the notice generator.
- [ ] Commit as `feat: complete previews attribution and accessibility`.

### Task 7: Build reproducible release gates

**Files:**
- Modify: `.github/workflows/rust.yml`
- Modify: `rust-app/scripts/publish.ps1`
- Modify: `rust-app/scripts/build-installer.ps1`
- Modify: `rust-app/scripts/validate-release.ps1`
- Create: `rust-app/scripts/smoke-installed.ps1`
- Modify: `rust-app/installer/SayFlex.iss`
- Modify: `rust-app/Cargo.toml`
- Modify: workspace package manifests and `Cargo.lock`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Produces: version `0.7.0`, exact installer validation, optional signing in developer builds, mandatory signing in public tag jobs, and a machine-readable smoke receipt.

- [ ] Add Pester fixtures proving validator rejects stale, mismatched, unsigned-public, and altered installers; run RED then implement exact-path validation.
- [ ] Add signing parameters that use certificate secrets without logging them and verify every shipped executable after signing.
- [ ] Add CI package and supply-chain jobs using `--locked`; upload installer, checksum, manifest, notices, and smoke schema.
- [ ] Add the installed smoke script covering editor/browser/terminal insertion and translation, ORIG/DUAL/pin/resize/multi-pin/click-away, microphone switch, provider fallback, fresh install, and upgrade. Unsupported interactive checks must be explicit `NOT_RUN`, never implicit success.
- [ ] Bump all workspace packages and installer metadata to `0.7.0`; update changelog and stable user-agent version derivation.
- [ ] Run fmt, clippy, workspace tests, cargo-deny, notice generation, release build, installer build, installed validation, and available live smoke checks.
- [ ] Commit as `release: prepare SayFlex 0.7.0`.

### Task 8: Final audit and publication boundary

**Files:**
- Review all files changed by Tasks 1-7.

**Interfaces:**
- Consumes: every acceptance requirement in the frozen spec.
- Produces: one evidence table distinguishing PASS, FAIL, BLOCKED, and NOT_RUN.

- [ ] Re-read the spec and map every requirement to code and evidence.
- [ ] Inspect `git diff` for secret material, generated build output, accidental user data, and unrelated changes.
- [ ] Verify the staged and installed manifests and hashes refer to the same commit and version.
- [ ] Push `feature/release-hardening` only after local gates pass; record remote ref identity.
- [ ] Do not create or advertise a public release unless Authenticode and every mandatory live gate are PASS.

