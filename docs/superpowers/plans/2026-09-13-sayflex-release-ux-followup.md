# SayFlex Release UX Follow-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver typed translation feedback, a coherent quick translator, a lean settings shell, consensual first-run model download, and a configurable support action without packaging unapproved visual work.

**Architecture:** The router's typed provider attempts cross the versioned protocol instead of being flattened into English. Rust owns translator state and first-run persistence; Slint renders explicit states using the established sheet controls. Direct and Store installs share the same in-app first-run authority.

**Tech Stack:** Rust 2024, Slint 1.17.1, serde JSON-lines protocol, gettext PO catalogs, Win32 ShellExecute, PowerShell release checks.

**Spec:** `docs/superpowers/specs/2026-09-13-sayflex-release-ux-followup-design.md`

## Global Constraints

- Windows 10/11 x64 only.
- Interface locales are System, English, Russian, Ukrainian, Spanish, Brazilian Portuguese, German, French, Italian, and Korean.
- Secrets and raw user text never enter protocol diagnostics or release receipts.
- First-run download requires an explicit Continue action; a default checkmark alone is not consent.
- Do not package translator, first-run, or support-action visual changes before user approval.
- Keep process exit in the tray menu; closing Settings hides the window.

---

### Task 1: Typed translation attempts across the worker boundary

**Files:**
- Modify: `rust-app/crates/protocol/src/lib.rs`
- Modify: `rust-app/apps/translation-worker/src/providers.rs`
- Modify: `rust-app/apps/translation-worker/src/main.rs`
- Test: `rust-app/crates/protocol/src/lib.rs`
- Test: `rust-app/apps/translation-worker/src/main.rs`

**Interfaces:**
- Produces: `TranslationFailureKind` with credential, misconfigured, quota-exhausted, rate-limited, unavailable, rejected, and empty-response variants.
- Produces: `TranslationAttemptReport { provider_id, kind, retry_after_ms, detail }`.
- Adds: `TranslationResult.attempts: Vec<TranslationAttemptReport>` with serde default.
- Adds: `TranslationWorkerEvent::Failed.translation: Option<TranslationFailureReport>` with serde default.

- [ ] **Step 1: Write failing protocol round-trip and redaction tests**

  Construct a completed fallback result containing a DeepL quota attempt and a
  failed request containing an unsupported-pair report. Assert exact JSON
  round-trip, leaf-level fingerprint inclusion, and absence of a fixture API
  key from every serialized field.

- [ ] **Step 2: Run the focused tests and verify RED**

  Run: `cargo test -p rust-protocol translation_attempt -- --nocapture`

  Expected: compilation fails because the typed report structures do not exist.

- [ ] **Step 3: Add protocol types and bump the wire version**

  Add the typed structures, populate every fixture with all optional fields,
  bump `PROTOCOL_VERSION` from 23 to 24, and update the asserted schema
  fingerprint only after reviewing its diff.

- [ ] **Step 4: Preserve attempts for success, local fallback, and exhaustion**

  Change `providers::Routed` so `Translated`, `UseLocalEngine`, and `Exhausted`
  retain `Vec<TranslationAttemptReport>`. Map `ProviderFailure` variants without
  parsing display strings. Attach attempts to the final result and to a typed
  request failure; keep human-readable detail redacted and secondary.

- [ ] **Step 5: Run worker and protocol suites**

  Run: `cargo test -p rust-protocol -p rust-translation-worker --no-fail-fast`

  Expected: all protocol fingerprints, route fallback tests, local fallback,
  cancellation, and failure tests pass.

- [ ] **Step 6: Commit the typed boundary**

  Run: `git add rust-app/crates/protocol rust-app/apps/translation-worker && git commit -m "feat: preserve typed translation fallback outcomes"`

---

### Task 2: Rust-owned translator presentation state

**Files:**
- Create: `rust-app/apps/desktop/src/translator_status.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Modify: `rust-app/apps/desktop/src/translation_view.rs`
- Test: `rust-app/apps/desktop/src/translator_status.rs`

**Interfaces:**
- Consumes: typed attempt and failure reports from Task 1.
- Produces: `TranslatorNotice { severity, headline, detail }`.
- Produces: `present_translation_outcome(locale, result_or_failure) -> TranslatorPresentation`.
- Produces: popup error state for every terminal selection-translation failure.

- [ ] **Step 1: Write failing projection tests**

  Add table-driven tests for quota, credential, timeout/unavailable,
  unsupported request, empty response, missing local model, no source text,
  all providers disabled, and successful DeepL-to-local fallback. Assert that
  provider names remain names while all sentences come from `UiText`.

- [ ] **Step 2: Run the projection tests and verify RED**

  Run: `cargo test -p sayflex-desktop translator_status -- --nocapture`

  Expected: failure because `TranslatorPresentation` is absent.

- [ ] **Step 3: Implement localized state projection**

  Implement one match over the typed enums. Never parse `message` text to infer
  a category. Keep redacted technical detail behind the concise localized
  headline and preserve a fallback notice when the result itself succeeds.

- [ ] **Step 4: Route failures to the correct surface**

  Standalone-origin requests update the translator state. Selection-origin
  failures create and present an error popup at the same target anchor used for
  success. Superseded/cancelled requests remain silent only when the user has
  already replaced or dismissed them.

- [ ] **Step 5: Run desktop logic tests**

  Run: `cargo test -p sayflex-desktop translator -- --nocapture`

  Expected: every typed state and origin route passes.

- [ ] **Step 6: Commit translator state ownership**

  Run: `git add rust-app/apps/desktop/src && git commit -m "fix: present actionable translation outcomes"`

---

### Task 3: Translator interaction and approved-state preview

**Files:**
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `rust-app/apps/desktop/ui/controls.slint`
- Create: `rust-app/apps/desktop/assets/action-copy.svg`
- Create: `rust-app/apps/desktop/assets/action-swap.svg`
- Modify: `rust-app/apps/desktop/src/preview_modes.rs`
- Test: `rust-app/apps/desktop/src/localization.rs`

**Interfaces:**
- Consumes: `TranslatorPresentation` from Task 2.
- Produces: explicit Slint properties `translator-state`, `notice-headline`, and `notice-detail`.
- Produces: `--translator-preview=<idle|focus|processing|fallback|result|error>`.

- [ ] **Step 1: Write failing source/layout tests**

  Assert one invariant field background, signal border only for source focus or
  result emphasis, accessible icon labels, one keyboard path for Ctrl+Enter,
  and content-driven action widths in every locale.

- [ ] **Step 2: Run the desktop UI tests and verify RED**

  Run: `cargo test -p sayflex-desktop translator -- --nocapture`

  Expected: the old text actions and focus-dependent TextEdit fill violate the assertions.

- [ ] **Step 3: Implement the Slint states and icon actions**

  Keep both field fills at `Sheet.raised`; expose focus from the source
  `TextEdit`; render signal borders from explicit state; bind Ctrl+Enter to the
  same `translate()` callback; replace swap/copy labels with SVG icon buttons
  carrying localized tooltips and accessibility labels.

- [ ] **Step 4: Render all translator preview states**

  Run each preview mode at 780x610 and at the documented minimum size in all
  interface locales. Save screenshots and a JSON layout receipt containing
  visible bounds, text overflow flags, and enabled action states.

- [ ] **Step 5: Stop for user visual approval**

  Present idle, focused-source, fallback-success, result, and terminal-error
  screenshots. Do not continue to packaging until the user approves or requests
  visual changes.

- [ ] **Step 6: Commit only the approved translator visual**

  Run: `git add rust-app/apps/desktop/ui rust-app/apps/desktop/assets rust-app/apps/desktop/src/preview_modes.rs rust-app/apps/desktop/src/localization.rs && git commit -m "feat: finish quick translator interaction"`

---

### Task 4: Lean settings footer and close-to-tray contract

**Files:**
- Modify: `rust-app/apps/desktop/ui/settings.slint`
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Test: `rust-app/apps/desktop/src/localization.rs`

**Interfaces:**
- Reuses: existing `SettingsWindow.open-translator()` callback.
- Removes: `SettingsWindow.quit-requested()` and `page-numbers`.

- [ ] **Step 1: Write failing shell ownership tests**

  Assert the footer invokes `open-translator`, contains no process-exit action,
  contains no bottom page-number model, and the tray retains the only Quit menu
  callback.

- [ ] **Step 2: Run the focused test and verify RED**

  Run: `cargo test -p sayflex-desktop settings_shell -- --nocapture`

  Expected: failure on `Quit SayFlex` and `page-numbers`.

- [ ] **Step 3: Replace the footer action and delete duplicate chrome**

  Bind the persistent top-right action to `open-translator()` with localized
  `Quick translation` copy. Remove the quit handler from `wire_settings`;
  retain tray shutdown and the existing window-close hide behavior.

- [ ] **Step 4: Run localization and layout tests**

  Run: `cargo test -p sayflex-desktop settings_shell localization -- --nocapture`

  Expected: every locale fits at minimum window size and no duplicate page number exists.

- [ ] **Step 5: Commit the shell cleanup**

  Run: `git add rust-app/apps/desktop && git commit -m "fix: simplify settings window actions"`

---

### Task 5: First-run model-download consent

**Files:**
- Modify: `rust-app/apps/desktop/src/settings_document.rs`
- Modify: `rust-app/apps/desktop/src/main.rs`
- Modify: `rust-app/apps/desktop/ui/app.slint`
- Modify: `rust-app/apps/desktop/src/preview_modes.rs`
- Test: `rust-app/apps/desktop/src/settings_document.rs`
- Test: `rust-app/apps/desktop/src/main.rs`

**Interfaces:**
- Produces: `FirstRunChoice { download_recommended_speech_model: bool }` persisted after confirmation.
- Produces: `load_settings() -> LoadedSettings { settings, needs_first_run_choice }`.
- Produces: a modal first-run Slint component that returns one confirmed choice.

- [ ] **Step 1: Write failing fresh/existing-profile tests**

  Assert an absent settings file requires a choice, a valid existing document
  does not, an invalid quarantined document does require one, unchecked
  confirmation initializes speech with `prepare_local_model=false`, and no
  `EnsureModel` is sent before confirmation.

- [ ] **Step 2: Run the focused tests and verify RED**

  Run: `cargo test -p sayflex-desktop first_run -- --nocapture`

  Expected: current default settings immediately prepare the local model.

- [ ] **Step 3: Implement durable first-run ownership**

  Return origin metadata from settings loading, show the choice before worker
  initialization, and persist the confirmed local-provider state atomically.
  Existing profiles preserve their provider list unchanged.

- [ ] **Step 4: Add checked and unchecked preview receipts**

  Render the real model display name and catalog byte size. Assert the Continue
  action is keyboard reachable and no network/download begins in preview mode.

- [ ] **Step 5: Stop for user visual approval**

  Show checked and unchecked first-run screenshots before integrating the view
  into a release candidate.

- [ ] **Step 6: Run desktop and speech-worker tests**

  Run: `cargo test -p sayflex-desktop -p sayflex-speech-worker --no-fail-fast`

  Expected: consent, persistence, and no-download paths pass.

- [ ] **Step 7: Commit the approved first-run flow**

  Run: `git add rust-app/apps/desktop && git commit -m "feat: require first-run model download consent"`

---

### Task 6: Configurable support action

**Files:**
- Modify: `rust-app/apps/desktop/build.rs`
- Modify: `rust-app/apps/desktop/ui/pages_instruments.slint`
- Modify: `rust-app/apps/desktop/ui/settings.slint`
- Create: `rust-app/apps/desktop/assets/action-coffee.svg`
- Modify: `rust-app/apps/desktop/src/settings_view.rs`
- Modify: `rust-app/apps/desktop/src/localization.rs`

**Interfaces:**
- Consumes: optional build variable `SAYFLEX_SUPPORT_URL`.
- Produces: `open-support()` callback shown only for a valid HTTPS URL.

- [ ] **Step 1: Write failing URL-policy and visibility tests**

  Assert absent, non-HTTPS, and malformed values hide the action; a valid HTTPS
  value shows one secondary About action; ShellExecute failure reports through
  the settings status channel.

- [ ] **Step 2: Run the tests and verify RED**

  Run: `cargo test -p sayflex-desktop support_action -- --nocapture`

  Expected: failure because no support action exists.

- [ ] **Step 3: Implement the configured action**

  Validate the URL at build/startup, expose it to Slint only when valid, and use
  the existing Windows shell-opening helper. Do not guess or embed a developer
  account URL.

- [ ] **Step 4: Render Settings with and without the action**

  Save both preview screenshots at minimum size in every interface locale and
  assert no clipping or secondary-brand dominance.

- [ ] **Step 5: Stop for user visual approval and destination input**

  Obtain approval for the persistent footer placement and the actual support
  URL before enabling it in release packaging. Placement was approved on
  2026-09-13; the release URL remains intentionally unset.

- [ ] **Step 6: Commit support infrastructure**

  Run: `git add rust-app/apps/desktop && git commit -m "feat: add configurable developer support action"`

---

### Task 7: Full localization and installed translation matrix

**Files:**
- Modify: `rust-app/apps/desktop/translations/{ru,uk,es,pt-BR,de,fr,it,ko}/LC_MESSAGES/sayflex-desktop.po`
- Modify: `rust-app/scripts/check-ui-translations.ps1`
- Modify: `rust-app/scripts/installed-smoke.ps1`
- Modify: `docs/release-checklist.md`

**Interfaces:**
- Consumes: all approved UI copy and typed states from Tasks 1-6.
- Produces: complete semantic catalogs and installed smoke receipts.

- [ ] **Step 1: Make the catalog completeness check fail on new English ids**

  Run: `pwsh -File scripts/check-ui-translations.ps1`

  Expected: FAIL listing every untranslated new message id.

- [ ] **Step 2: Write semantic translations for all eight catalogs**

  Translate actions by user intent, preserve SayFlex/provider/model names and
  interpolation placeholders, remove stale source ids, and avoid literal
  translations such as the rejected Russian `Только этот`.

- [ ] **Step 3: Run catalog and Rust gates**

  Run: `pwsh -File scripts/check-ui-translations.ps1`

  Run: `cargo test --workspace --no-fail-fast`

  Run: `cargo clippy --workspace --all-targets -- -D warnings`

  Run: `cargo fmt --all -- --check`

  Expected: all commands exit 0.

- [ ] **Step 4: Build one candidate after all visual approvals**

  Run: `pwsh -File scripts/build-installer.ps1`

  Expected: one installer path, checksum sidecar, manifest, and protocol 24
  validation tied to the current commit.

- [ ] **Step 5: Install and run the translation failure/fallback matrix**

  Exercise success, DeepL quota to fallback, bad key, no providers, timeout,
  unsupported pair, empty source, selection popup placement, Ctrl+Enter,
  icon-copy, and icon-swap. Record provider identity and typed reason without
  recording source or translated text.

- [ ] **Step 6: Commit release evidence**

  Run: `git add rust-app/apps/desktop/translations rust-app/scripts docs/release-checklist.md && git commit -m "test: gate localized translation outcomes"`
