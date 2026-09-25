# SayFlex Lightweight Translation Packs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an install-on-demand catalog of verified Firefox translation packages for every SayFlex language paired with English, while preserving Qwen 4B and 9B as contextual choices.

**Architecture:** Extend the verified bundle installer to atomically expand pinned zstd artifacts, add a `compact` local translation tier backed by `fxtranslate`, and keep all engine ownership in the existing translation worker. Package lifecycle is keyed by stable catalog id rather than tier, so many compact pairs can coexist; request-time routing loads the installed package matching the resolved languages. The desktop continues to consume one worker protocol while presenting compact pair packages separately from contextual Qwen models.

**Tech Stack:** Rust 1.88 / edition 2024, `fxtranslate` 0.4.2 with native fast and ICU segmentation features, `ruzstd` 0.7, Slint 1.17.1, line-delimited JSON worker protocol, Firefox Translations v3 model artifacts.

**Spec:** `docs/superpowers/specs/2026-09-21-sayflex-lightweight-translation-packs-design.md`

## Global Constraints

- Qwen 4B and Qwen 9B remain available and their existing installed files are not removed.
- New compact packages are CPU-only and must report no GPU/VRAM allocation of their own.
- No package download, cloud-provider enablement, or mode switch occurs without an explicit user action; the existing first-run checkbox remains the only approved automatic-install opt-in.
- Existing `tiny` and `teacher` settings remain readable and keep selecting Qwen 4B and Qwen 9B respectively.
- Model URLs, compressed byte counts, compressed SHA-256 values, expanded byte counts, and expanded SHA-256 values are pinned in the checked-in catalog.
- SayFlex's model manager remains the sole owner of download, cancellation, verification, decompression, installation, and removal.
- At most one local translation engine is resident; the old engine is released before the new one loads.
- Source text and translated text never enter diagnostics.
- The existing dirty worktree is preserved. No intermediate commit may capture pre-existing overlapping edits; verification evidence is recorded in the execution ledger instead.

## Review Focus

- A corrupt but valid zstd stream must fail expanded-hash verification and must never replace an installed package.
- Selecting Compact with only one direction present must report the package as not installed, not load a half-working translator.
- Existing settings containing `tiny` or `teacher` must retain their Qwen selection after the protocol gains `compact`.
- A compact translation cancelled between paragraphs must not emit a successful completion or replace selected text.
- Removing the last Qwen package while a compact pair remains must still remove the unused llama runtime; removing Compact must not remove a runtime used by Qwen.

---

### Task 1: Atomic zstd bundle artifacts

**Files:**
- Modify: `crates/model-manager/Cargo.toml`
- Modify: `crates/model-manager/src/bundle.rs`

**Interfaces:**
- Produces: `BundleCompression::Zstd`, optional expanded metadata on `BundleArtifact`, `BundleDescriptor::installed_bytes()`.
- Consumes: existing `ArtifactFetcher`, cancellation behavior, atomic staging directory, receipts, and SHA-256 verifier.

- [ ] **Step 1: Write failing bundle tests**

Add tests that use the literal 44-byte zstd fixture
`KLUv/SQf+QAAZnh0cmFuc2xhdGUgdGlueSBtb2RlbCBmaXh0dXJlCsuODh0=` and assert:

```rust
#[test]
fn installs_zstd_artifact_as_verified_expanded_file() {
    // Download hash covers the 44-byte source; expanded hash covers
    // b"fxtranslate tiny model fixture\n".
    // The final bundle contains only model.bin, never the .zst source.
}

#[test]
fn rejects_zstd_artifact_when_expanded_hash_is_wrong() {
    // ensure_installed returns HashMismatch and the final bundle is absent.
}

#[test]
fn reports_download_and_installed_sizes_separately() {
    // total_bytes() == 44; installed_bytes() == 31.
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p rust-model-manager bundle::tests -- --nocapture`

Expected: compilation fails because compression/expanded metadata and `installed_bytes` do not exist.

- [ ] **Step 3: Implement streaming expansion**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleCompression { Zstd }
```

Extend `BundleArtifact` with optional `compression`, `expanded_bytes`, and
`expanded_sha256`. Preserve the existing `bytes`/`sha256` meaning as the bytes
downloaded from the network. Download compressed input to a dot-prefixed staging
file, verify its compressed metadata, stream it through
`ruzstd::StreamingDecoder` into the final relative path, verify the expanded
metadata, then remove the compressed staging file. Receipts describe the final
expanded file. Existing uncompressed artifacts retain byte-for-byte behavior.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p rust-model-manager bundle::tests -- --nocapture`

Expected: all bundle tests pass.

- [ ] **Step 5: Record checkpoint**

Record the test command and result in the execution ledger. Do not commit over
the pre-existing dirty worktree.

### Task 2: Compact pack protocol and pinned RU ↔ EN catalog

**Files:**
- Modify: `crates/protocol/src/lib.rs`
- Modify: `crates/translation-engine/src/catalog.rs`
- Modify: `crates/translation-engine/models/catalog.json`
- Modify: protocol-version assertions that require the new value

**Interfaces:**
- Consumes: Task 1 expanded bundle metadata.
- Produces: `TranslationTier::Compact`, `FirefoxDirectionDescriptor`, and
  `TranslationBackendDescriptor::Firefox { directions }`.

- [ ] **Step 1: Write failing protocol/catalog tests**

Add literal behavior tests asserting that:

```rust
assert_eq!(serde_json::to_string(&TranslationTier::Compact).unwrap(), "\"compact\"");
let compact = catalog.by_tier(TranslationTier::Compact).unwrap();
assert!(compact.supports(TranslationLanguage::English, TranslationLanguage::Russian));
assert!(compact.supports(TranslationLanguage::Russian, TranslationLanguage::English));
assert!(!compact.supports(TranslationLanguage::German, TranslationLanguage::Russian));
assert!(compact.as_bundle().total_bytes() > 46_000_000);
assert!(compact.as_bundle().total_bytes() < 48_000_000);
assert!(compact.as_bundle().installed_bytes() > 72_000_000);
```

Also assert that each direction names exactly one model, shared vocab, and
shortlist file and that every named expanded path belongs to the bundle.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p rust-protocol -p rust-translation-engine catalog -- --nocapture`

Expected: compilation fails because `Compact` and the Firefox descriptor do not exist.

- [ ] **Step 3: Add the backend and catalog entry**

Bump `PROTOCOL_VERSION` once. Add `compact` without renaming `tiny` or `teacher`.
Add a `firefox-ru-en-v3.1` catalog entry whose six zstd URLs and both compressed
and expanded hashes are copied from the 2026-09-21 Mozilla Remote Settings
snapshot. Its display name is `Russian ↔ English`, license is `MPL-2.0`, and its
two directions are `en→ru` and `ru→en`. Keep the Qwen descriptors unchanged.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p rust-protocol -p rust-translation-engine catalog -- --nocapture`

Expected: protocol and catalog tests pass.

- [ ] **Step 5: Record checkpoint**

Record tests in the ledger without committing overlapping files.

### Task 3: Native Rust compact translation adapter

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/translation-engine/Cargo.toml`
- Create: `crates/translation-engine/src/firefox.rs`
- Modify: `crates/translation-engine/src/lib.rs`
- Modify: `crates/translation-engine/src/engine.rs`

**Interfaces:**
- Consumes: Task 2 Firefox direction descriptors and installed bundle root.
- Produces: `FirefoxRuntime::load(pack, root)`, `FirefoxRuntime::translate_streaming(...)`, and backend-neutral `TranslationEngine` runtime ownership.

- [ ] **Step 1: Write failing adapter tests**

Use an injected `FnMut(TranslationLanguage, TranslationLanguage, &str) -> Result<String>`
in the paragraph coordinator and assert:

```rust
#[test]
fn compact_translation_preserves_blank_lines_and_emits_completed_paragraphs() {
    // "One.\n\nTwo." becomes "Один.\n\nДва." and partial 1 never contains "Два".
}

#[test]
fn compact_translation_stops_before_the_next_paragraph_when_cancelled() {
    // Cancellation after paragraph 1 returns TranslationCancelled and makes one call.
}

#[test]
fn compact_runtime_rejects_a_direction_not_declared_by_the_pack() {
    // German→Russian is rejected before touching model files.
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p rust-translation-engine firefox -- --nocapture`

Expected: compilation fails because the adapter does not exist.

- [ ] **Step 3: Implement the compact runtime**

Pin `fxtranslate = 0.4.2` with `default-features = false` and features
`["fast", "icu-segmenter"]`. For each direction load the model with
`Engine::load_mmapped`, attach the shortlist with `Shortlist::load`, and keep both
engines resident in `FirefoxRuntime`. Split input on paragraph separators,
translate each non-empty paragraph with `translate_long`, preserve separators,
check cancellation between paragraphs, and emit only completed accumulated
paragraphs. Measure first completed paragraph as first-result latency.

Change `TranslationEngine` to hold a runtime enum. The current llama.cpp path and
its token streaming remain unchanged; the Firefox path emits paragraph progress.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p rust-translation-engine -- --nocapture`

Expected: all translation-engine tests pass.

- [ ] **Step 5: Record checkpoint**

Record tests in the ledger without committing overlapping files.

### Task 4: Translation-worker lifecycle and settings compatibility

**Files:**
- Modify: `apps/translation-worker/src/main.rs`
- Modify: `apps/desktop/src/settings_document.rs`
- Modify: `apps/desktop/src/main.rs`
- Modify: related worker/protocol tests

**Interfaces:**
- Consumes: Task 3 backend-neutral `TranslationEngine` and `TranslationTier::Compact`.
- Produces: selection/install/remove/unload behavior for Compact with no llama runtime download.

- [ ] **Step 1: Write failing lifecycle tests**

Add tests asserting:

```rust
assert_eq!(choose_initial_tier(None, true, false, false), Some(TranslationTier::Compact));
assert_eq!(choose_initial_tier(Some(TranslationTier::Tiny), true, true, true), Some(TranslationTier::Tiny));
assert_eq!(choose_initial_tier(Some(TranslationTier::Teacher), true, true, true), Some(TranslationTier::Teacher));
```

Add a settings fixture containing `"selected_translation_tier":"tiny"` and
assert it remains `Tiny`. Add pure lifecycle coverage showing Compact install and
selection do not call the llama runtime installer, while Qwen still does.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p sayflex-translation-worker -p sayflex-desktop settings_document -- --nocapture`

Expected: new compact-aware arity/lifecycle assertions fail.

- [ ] **Step 3: Implement lifecycle routing**

Make runtime installation conditional on `backend.requires_llama_runtime()`.
Prefer Compact only when no persisted installed choice is valid. Preserve Qwen
choices on upgrade. On removal, remove llama.cpp only when no installed Qwen
pack remains, not when the entire translation catalog becomes empty. Unload and
switch operations continue dropping the old engine before loading the new one.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p sayflex-translation-worker -p sayflex-desktop settings_document -- --nocapture`

Expected: lifecycle and migration tests pass.

- [ ] **Step 5: Record checkpoint**

Record tests in the ledger without committing overlapping files.

### Task 5: Clear compact/contextual model UX

**Files:**
- Modify: `apps/desktop/ui/rows.slint`
- Modify: `apps/desktop/ui/pages_lists.slint`
- Modify: `apps/desktop/ui/pages_forms.slint`
- Modify: `apps/desktop/ui/settings.slint`
- Modify: `apps/desktop/src/settings_projection.rs`
- Modify: `apps/desktop/src/settings_view.rs`
- Modify: `apps/desktop/src/settings_preview.rs`
- Modify: `apps/desktop/ui_texts.txt`
- Modify: all eight `apps/desktop/translations/*/LC_MESSAGES/sayflex-desktop.po` catalogs

**Interfaces:**
- Consumes: compact model summaries and existing choose/install/remove callbacks.
- Produces: visually separated `Fast translation packages` and `Contextual translation models` groups, explanatory mode text, and a deterministic preview state.

- [ ] **Step 1: Write failing projection/localization tests**

Add a Compact summary and assert its `ModelRow` has the fast-package section,
localized `Russian ↔ English` title, `Recommended` detail, and a CPU/no-GPU
description. Assert Qwen rows use the contextual section. Extend typed-catalog
tests so every new message exists in all eight locales.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p sayflex-desktop settings_projection localization -- --nocapture`

Expected: tests fail because compact projection and localized messages do not exist.

- [ ] **Step 3: Implement the UI and translations**

Keep the existing list layout and action buttons, but split translation rows into
two headings. The Translation page's model row shows a two-line explanation:
compact means small CPU package/no GPU memory; contextual means larger RAM and
possible GPU use. Add a visible Models-page action rather than technical engine
controls. Populate preview rows for an installed selected compact pair plus both
Qwen choices. Supply native translations for Russian, Ukrainian, Spanish,
Portuguese-Brazil, German, French, Italian, and Korean.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p sayflex-desktop settings_projection localization -- --nocapture`

Expected: projection and catalog tests pass.

- [ ] **Step 5: Build and open the visual checkpoint**

Run: `cargo build --release -p sayflex-desktop`

Then run the settings preview on the Models and Translation pages in Russian.
Inspect the actual window for clipping, unclear nesting, inconsistent action
heights, and scroll behavior. Leave the preview open for user review.

### Task 6: Real package install and RU ↔ EN admission evidence

**Files:**
- Create: `tools/translation-benchmark/Cargo.toml`
- Create: `tools/translation-benchmark/src/main.rs`
- Modify: root `Cargo.toml`
- Create: `docs/evidence/lightweight-translation-ru-en.md`

**Interfaces:**
- Consumes: Tasks 1–4 installed pack and runtime.
- Produces: release-build latency, memory, completeness, and sample-output evidence without making the product default prematurely.

- [ ] **Step 1: Write the failing benchmark contract test**

The tool accepts `--pack-root`, translates a checked-in literal corpus in both
directions, writes only timings/resource counts to stdout by default, and writes
source/output samples only with explicit `--include-text`. Unit tests assert the
default report contains no corpus phrases and that an empty or partial direction
is rejected.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p sayflex-translation-benchmark -- --nocapture`

Expected: package does not exist.

- [ ] **Step 3: Implement and run the real release benchmark**

Install the pinned pair through `BundleInstaller`, load both directions through
the production adapter, and translate UI strings, paragraphs, punctuation,
names, numbers, a URL, and multiline text. Capture cold load, warm translation,
process working-set delta, CPU duration, and zero child GPU-process use. Record
exact artifact hashes and machine facts in the evidence document.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test -p sayflex-translation-benchmark -- --nocapture`

Run: `cargo run --release -p sayflex-translation-benchmark -- --install --include-text`

Expected: both directions complete without empty or truncated output; evidence
records measurements and representative samples for user acceptance.

### Task 7: Supply-chain, release, and whole-workspace gates

**Files:**
- Modify: `THIRD_PARTY_NOTICES.md`
- Modify: `scripts/generate-third-party-notices.ps1`
- Modify: `scripts/validate-release.ps1`
- Modify: `scripts/tests/release-validation.Tests.ps1`
- Modify: release manifest/package inputs if the crate build adds native objects

**Interfaces:**
- Consumes: all prior tasks.
- Produces: reproducible licensing and installed-artifact validation.

- [ ] **Step 1: Write failing release assertions**

Require notices for `fxtranslate` and Firefox Translation models (MPL-2.0), the
pinned compact catalog entry, successful expanded-file verification, and absence
of Python/WASM/MKL runtime artifacts.

- [ ] **Step 2: Verify RED**

Run: `pwsh -NoProfile -File scripts/tests/run-tests.ps1`

Expected: new lightweight-translation release assertions fail.

- [ ] **Step 3: Implement release metadata**

Generate notices from the resolved Cargo graph, add the model attribution and
source URL, and extend release validation to inspect the compact catalog and
expanded hashes. Do not bundle the 46–48 MB language package in the application
installer; it remains install-on-demand.

- [ ] **Step 4: Run final verification**

Run: `cargo fmt --all -- --check`

Run: `cargo test --workspace --all-targets`

Run: `cargo build --release --workspace`

Run: `pwsh -NoProfile -File scripts/tests/run-tests.ps1`

Run: `pwsh -NoProfile -File scripts/validate-release.ps1`

Expected: every command exits 0; the validator reports all release checks passed.

- [ ] **Step 5: Final independent review**

Package the diff, spec, plan, test evidence, and ledger for one fresh reviewer.
Fix Critical or Important findings once using RED→GREEN tests; report Minor
findings without silently expanding scope.

### Task 8: Catalog-id lifecycle and settings migration

**Files:**
- Modify: `crates/protocol/src/lib.rs`
- Modify: `crates/translation-engine/src/catalog.rs`
- Modify: `crates/translation-engine/src/engine.rs`
- Modify: `apps/translation-worker/src/main.rs`
- Modify: `apps/desktop/src/settings_document.rs`
- Modify: `apps/desktop/src/main.rs`
- Modify: `apps/desktop/src/settings_view.rs`

**Interfaces:**
- Produces: install/remove/select commands keyed by `catalog_id`, an optional persisted `selected_translation_model_id`, and catalog lookup by id/pair.
- Preserves: `TranslationTier` as the compact/contextual engine class and the old persisted tier as a migration fallback.

- [ ] **Step 1: Write failing identity and migration tests**

Assert that two Compact descriptors validate together, exact-id lookup returns
the requested pack, pair lookup returns the matching compact pack, removing one
compact id cannot address another, and a settings document containing only
`selected_translation_tier` still selects the corresponding legacy Qwen model.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p rust-protocol -p rust-translation-engine -p sayflex-translation-worker -p sayflex-desktop -- --nocapture`

Expected: the multi-Compact catalog is rejected and lifecycle commands cannot
name an exact compact package.

- [ ] **Step 3: Implement catalog-id ownership**

Make `EnsureModel`, `RemoveModel`, and model selection carry `catalog_id`.
Persist `selected_translation_model_id`; use the old tier only when the id is
absent. Compare loaded/selected rows by id. Add `TranslationEngine::catalog_id`
so two Compact engines are never mistaken for each other.

- [ ] **Step 4: Verify GREEN**

Run the Task 8 command and require every target to pass.

### Task 9: Full verified Firefox language catalog

**Files:**
- Create: `scripts/update-firefox-translation-catalog.ps1`
- Modify: `crates/translation-engine/models/catalog.json`
- Modify: `crates/translation-engine/src/catalog.rs`
- Modify: `docs/THIRD_PARTY_MODELS.md`

**Interfaces:**
- Consumes: Mozilla `translations-models-v2` records.
- Produces: thirteen bidirectional English-pair packages with pinned compressed and expanded metadata.

- [ ] **Step 1: Write failing coverage tests**

For every SayFlex translation language other than English, assert exactly one
Compact package supports both directions with English, has a unique catalog id,
contains the model/lexicon/vocabulary files referenced by both descriptors, and
passes bundle validation.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p rust-translation-engine catalog -- --nocapture`

Expected: only Russian ↔ English exists.

- [ ] **Step 3: Add the reviewed importer and snapshot**

The script selects the newest complete Mozilla model generation per direction,
maps `vocab` or `srcvocab`/`trgvocab` correctly, preserves the admitted Russian
package id, and rewrites only Compact entries while retaining Qwen entries.
Run it once and inspect the generated diff before accepting it.

- [ ] **Step 4: Verify GREEN**

Run the Task 9 command and require all catalog tests to pass.

### Task 10: Pair-aware loading and clear multi-package UX

**Files:**
- Modify: `apps/translation-worker/src/main.rs`
- Modify: `crates/translation-engine/src/firefox.rs`
- Modify: `apps/desktop/src/settings_projection.rs`
- Modify: `apps/desktop/src/settings_view.rs`
- Modify: `apps/desktop/src/settings_preview.rs`
- Modify: localized UI catalogs only when new copy is introduced

**Interfaces:**
- Consumes: Task 8 id lifecycle and Task 9 pair catalog.
- Produces: request-time compact routing, independent installed states, localized pair names, and recommended-first ordering.

- [ ] **Step 1: Write failing routing and projection tests**

Assert that Compact mode chooses the installed package matching the resolved
source/target pair, reports the exact missing package when absent, switches
resident engines when the pair changes, never removes another installed pair,
and renders localized language names without one hard-coded Russian id.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p sayflex-translation-worker -p sayflex-desktop settings_projection -- --nocapture`

Expected: tier lookup selects the first Compact package and non-Russian titles
fall back to English catalog strings.

- [ ] **Step 3: Implement routing and UX**

Resolve the source/target languages before loading the engine. In Compact mode,
select the installed exact pair; otherwise return a typed missing-model failure
that names its catalog id. Keep every installed package visible, sort the pack
matching the current primary/secondary languages first, and retain Install,
Use, Remove, Cancel, sizes, directions, and contextual Qwen rows. Warm compact
models with language-neutral input instead of Russian/English literals.

- [ ] **Step 4: Verify GREEN and inspect the release UI**

Run the Task 10 command, build `sayflex-desktop` in release mode, and inspect the
Models and Translation pages at their normal and minimum supported sizes.

### Task 11: Final catalog and release gates

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo test --workspace --all-targets`.
- [ ] Run `cargo build --release --workspace`.
- [ ] Run the Pester release tests and `cargo deny check licenses`.
- [ ] Validate every Compact URL/hash/size record without downloading every
      model, then install and execute at least one second language pair in the
      production adapter as a cross-pair smoke test.
- [ ] Package the amended diff for final review. Do not claim a remotely
      updateable catalog until a SayFlex signing key and hosted endpoint exist.
