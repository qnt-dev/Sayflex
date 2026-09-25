# SayFlex Lightweight Translation Packs — Design

**Date:** 2026-09-21  
**Status:** Approved; amended for the full language catalog  
**Scope:** Add compact language-pair translation without removing the existing Qwen models.

## 1. Product decision

SayFlex will offer two clearly different kinds of local translation:

1. **Fast local translation** — the recommended default for new installations.
   It uses small CPU language-pair packages based on Mozilla's Bergamot/Marian
   models. It does not reserve GPU memory.
2. **Contextual translation** — optional Qwen models for text where broader
   context matters. The existing Qwen 4B and Qwen 9B packages remain available
   and are not silently removed or replaced.

The UI describes the user-visible trade-off, not implementation jargon. Model
names, licenses, hashes, and engine details remain available under details.

## 2. Why Bergamot is the first compact backend

The preferred implementation is the native Rust `fxtranslate` engine, pinned to
an admitted release and called inside the existing translation worker.

- The engine and the Firefox model family were designed for local translation
  on consumer devices.
- The model inventory contains small directional models and supports pivoting
  through English.
- It reads the Firefox/Bergamot model format directly, supports memory-mapped
  weights and long-input segmentation, and has direct and pivoted translation
  types.
- It avoids bundling a browser, JavaScript runtime, Python, or a general-purpose
  language model.
- The worker remains the crash and resource boundary; an engine fault must not
  terminate the desktop process.

Alternatives rejected for the first implementation:

- **Bergamot C++ through a custom ABI:** uses the reference implementation but
  adds a second build system, native ABI, exception boundary, and linear-algebra
  dependency. It remains the correctness oracle, not the first shipping path.
- **Bergamot through WebAssembly:** keeps model compatibility but adds an
  unnecessary WASM runtime, startup cost, memory cost, and packaging surface to
  a native Windows application.
- **CTranslate2 plus OPUS-MT:** technically viable and retained as a fallback
  investigation, but requires a separate tokenizer/detokenizer contract and a
  new converted-model supply chain. Its currently identified RU/EN source
  models are larger than the Firefox/Bergamot pair.
- **NLLB/M2M100:** multilingual but materially larger; NLLB's published license
  and model card are unsuitable for this product route.

## 3. User experience

### Translation page

The local translator section presents two cards:

### Fast local — Recommended

Canonical description:

> Small language packages for everyday translation. Runs on the CPU and does
> not use GPU memory.

The currently configured primary/secondary language pair is shown directly
below it. For example:

> Russian ↔ English  ·  53 MB download

The pair has one of four unambiguous actions: **Download**, **Cancel**,
**Use**, or **Remove**. Forward and reverse artifacts are presented as one pair;
users do not have to understand directional model files.

### Contextual

Canonical description:

> Better for long or context-sensitive text. Uses considerably more RAM and may
> use GPU memory.

It contains the existing choices:

- **Balanced context** — Qwen 4B;
- **Maximum context quality** — Qwen 9B.

The technical Qwen name is secondary text. The resource page shows the actual
live RAM, CPU, GPU, and VRAM use instead of promising a fixed memory figure.

Changing the mode is explicit. SayFlex never silently substitutes Qwen for a
missing compact pair and never silently sends text to a cloud service. The
existing provider order may continue to a cloud provider only when that
provider was explicitly enabled by the user.

### Models page

The existing combined models list remains, with three readable groups:

1. Speech models
2. Fast translation packages
3. Contextual translation models

Compact packages use human pair names such as `Russian ↔ English`; contextual
rows use the product names above. Download size, installation state, selected
state, removal confirmation, SHA-256 verification progress, and license remain
visible through the existing row contract.

### First run and upgrades

- New installations select **Fast local** as the recommended mode.
- No model is downloaded without a user action or an explicitly retained
  installer/onboarding checkbox.
- Existing installations preserve their selected Qwen model and provider order.
  The UI may recommend Fast local, but an upgrade does not change translation
  behavior behind the user's back.

## 4. Language-pair behavior

The checked-in catalog publishes one independently installable English pair for
every non-English language already exposed by SayFlex: Russian, Ukrainian,
Spanish, Portuguese, German, French, Italian, Simplified Chinese, Traditional
Chinese, Japanese, Korean, Hindi, and Arabic. Every artifact is imported from a
reviewed Mozilla Remote Settings snapshot with immutable URL, byte count,
compressed SHA-256, expanded byte count, and expanded SHA-256. No arbitrary URL
or user-supplied model is accepted by the ordinary UI.

Russian ↔ English remains the measured admission baseline, not the only
installable package. Additional pairs are visible in the same catalog and are
installed independently, so installing German does not replace or remove
Russian.

- English ↔ another language uses one user-facing bidirectional package.
- A non-English ↔ non-English policy uses two packages and pivots through
  English only after both packages are installed.
- The UI states `Uses two local packages via English` in details and shows the
  combined download size before installation.
- Auto-detected source language still chooses the target with the existing
  primary/secondary language policy.
- A source language outside the installed pair returns a typed
  `local_pack_missing` or `unsupported_pair` result; it is never guessed.

When compact mode is selected, the worker resolves the package from the actual
source and target languages for each request. If that package is installed it
is loaded automatically, replacing only the resident engine. If it is absent,
SayFlex identifies the exact package required. A user action on that package
installs it; contextual Qwen remains the explicit universal fallback.

## 5. Runtime architecture

### Engine selection

Keep `TranslationTier` as the user-facing engine class, but never use it as a
package identity. The stable engine selection classes are:

- `compact_pair`
- `qwen_4b`
- `qwen_9b`

Install, remove, progress, loaded state, and the concrete selected model use a
stable `catalog_id`. The compact selection derives the required package ID from
the request's source/target pair. Qwen selection refers to one multilingual
catalog entry. Persist stable IDs, not localized labels; retain the old tier
field only as a migration fallback for existing settings.

During settings migration, the existing `tiny` value maps to `qwen_4b` and the
existing `teacher` value maps to `qwen_9b`. Missing new fields do not rewrite an
existing user's choice.

### Catalog

The translation catalog gains two backend descriptors:

- `bergamot_pair`: source, target, forward artifacts, reverse artifacts,
  engine/runtime version, and optional pivot metadata;
- `llama_cpp`: the current Qwen descriptor unchanged.

Published catalog entries pin immutable URLs, byte counts, and SHA-256 values.
The application does not consume Mozilla's mutable Remote Settings collection
directly at runtime. A maintenance script imports and validates an upstream
snapshot, but release artifacts use a reviewed, checked-in catalog. A future
online catalog may replace this snapshot only after SayFlex owns a signing key,
publishes a signed endpoint, and verifies signatures before parsing; an
unsigned runtime feed is deliberately out of scope.

The existing atomic bundle installer, resumable downloads, verification
receipts, cancellation, and safe removal remain the single owner of files.

### Native boundary

The `fxtranslate` crate is pinned exactly in `Cargo.lock` and built as part of
the existing Rust workspace. A narrow SayFlex adapter owns:

- engine creation and destruction;
- loading one direct pair or two pivot pairs;
- translating a list of UTF-8 paragraphs;
- cooperative cancellation between paragraphs;
- typed error and timing results.

No source text, translated text, or model contents are written to diagnostic
logs. SayFlex uses its existing model manager rather than `fxtranslate`'s
network/cache features, so there is one owner for downloads, cancellation,
verification, storage, and removal.

The existing translation worker owns both backends and exposes one protocol to
the desktop. At most one local translation backend is resident. Switching mode
or pair releases the old engine before loading the new one, preventing the
temporary two-model memory peak.

### Paragraphs and progress

Input is split only at safe paragraph boundaries. Empty lines and paragraph
order are preserved. Short input is translated in one request. Long input emits
completed paragraphs through the existing `Partial` event so the preview can
grow smoothly without token-by-token flicker.

The compact engine may return a short translation in one completed update. It
must not manufacture token streaming. The existing animated `Translating`
indicator remains visible only while more paragraphs are outstanding.

An incomplete, cancelled, or failed paragraph never produces a successful
terminal result and never triggers automatic replacement of the source text.

## 6. Resource behavior

- Compact translation is CPU-only in the first release and reports `0 MB` VRAM.
- It starts lazily on first use unless the user explicitly keeps local
  translation warm.
- `Unload from memory` drops the engine but keeps verified package files.
- The next translation reloads the selected engine automatically; the resource
  page keeps the already approved localized explanation of this behavior.
- Removing a package is separate from unloading it.

## 7. Failure and fallback behavior

User-facing failures are typed and localized:

- package not installed;
- download cancelled or interrupted;
- integrity verification failed;
- language pair unsupported;
- compact engine failed to load;
- translation incomplete;
- translation cancelled.

If the required pair is missing, the preview offers the exact package and size
to download. It may also point to an already installed contextual model. It does
not start a download, switch engines, or enable a cloud provider automatically.

## 8. Supply chain and licensing

- Pin the exact `fxtranslate` crate version and checksum in `Cargo.lock`; do not
  enable its independent network/cache feature.
- Record the crate source revision used for admission and preserve the Marian
  reference comparison evidence supplied by the upstream project.
- Use only model artifacts with confirmed redistributable terms.
- Record `fxtranslate`, Bergamot/Marian, and model licenses and attribution in
  `THIRD_PARTY_NOTICES.md` and in package details.
- Verify that the selected native SIMD dependency is open-source and compatible
  with the project's release license; do not silently depend on MKL.
- Direct installer, portable build, and Store package use the same verified
  runtime and model contract.

## 9. Admission gate before changing the new-install default

The RU ↔ EN compact prototype remains development-only until it passes all of
these checks in a release build on the target Windows machine:

1. Both directions work for short UI text, ordinary paragraphs, punctuation,
   names, numbers, URLs, and multiline input.
2. A fixed bilingual corpus is compared with Qwen 4B and Qwen 9B. Known meaning
   omissions, duplicated fragments, broken placeholders, and silent truncation
   are release blockers.
3. The user reviews representative translations before the mode becomes the
   default.
4. Cold and warm latency, peak working set, idle working set, CPU utilization,
   and VRAM are measured and recorded. The compact route must use no GPU memory
   and must materially reduce disk and resident-memory cost versus Qwen 4B.
5. Cancellation, unload/reload, model removal, interrupted download resume,
   corrupt-file rejection, and mode switching pass automated tests.
6. Installer and portable manifests contain only pinned, licensed artifacts and
   pass the existing release validation.

If quality is not acceptable, Qwen remains the default and the compact mode is
not presented as recommended. Small size alone is not an admission criterion.

## 10. Delivery sequence

1. Build a pinned `fxtranslate` prototype and RU ↔ EN benchmark pack, checking
   output against the Bergamot/Marian reference where practical.
2. Add backend-neutral protocol/catalog identities and settings migration.
3. Add compact runtime ownership to the translation worker with tests.
4. Integrate package download, verification, cancellation, removal, and resource
   reporting.
5. Add the two-mode translation UI and localized copy.
6. Run the admission benchmark and user review.
7. Publish all verified English-centric packages already covered by SayFlex,
   with independent install/remove actions and request-time package routing.
8. Only after quality acceptance, make Fast local the default for new
   installations.

## 11. Explicit non-goals for this slice

- Removing Qwen or its existing downloaded files.
- Cloud translation without explicit provider configuration.
- GPU acceleration for compact packages.
- Downloading the full language catalog during installation.
- Claiming identical quality between a compact pair model and a contextual LLM.
