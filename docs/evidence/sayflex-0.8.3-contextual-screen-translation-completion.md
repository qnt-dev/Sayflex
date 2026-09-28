# SayFlex 0.8.3 contextual screen translation completion report

Date: 2026-09-28

Candidate source commit: `d4eb84ab5c632a2f48c1b65eb9eb026f56fdf402`

Status: installed technical candidate; user acceptance and the environment-specific
checks listed below remain separate gates.

## Implemented

| Area | Original scenario | Change | Verification | Result |
|---|---|---|---|---|
| Clipboard ownership | A delayed clipboard materialization could look like an external Copy, while a real competing Copy could arrive between snapshot and replacement/restoration. | Replacement and restoration now revalidate one immutable publication identity while the clipboard is locked and distinguish SayFlex materialization from a user's newer clipboard sequence. | Unit and cross-process delayed-image tests cover text, image, retry, and competitive Copy. | Implemented; automated tests pass. |
| Speech lifecycle | The five-minute limit could leave capture active; cancelled final/cloud results could still publish; model activation, selected language, replacement order, and exhausted cloud budget had split owners. | Capture, speculation, final publication, installation, language configuration, model residency, and local fallback now keep operation/session ownership through their terminal transitions. | Speech CPU, CUDA, and Vulkan suites each passed 44 unit and 4 protocol tests; feature-specific Clippy passed. | Implemented; live five-minute/cloud scenarios are still manual gates. |
| Compact translation | Whitespace-only paragraphs could panic; fragment completion and cancellation were ambiguous; history could block control commands. | Whitespace is handled before ranges, completion is typed as completed/limit/cancelled/error, incomplete text is not auto-inserted, and history has independent ownership. | Translation engine and worker suites passed, including completion, pivot propagation, cancellation, and history responsiveness. | Implemented; real Firefox/long-paragraph interaction remains a manual gate. |
| Windows integration | Normalized installer paths, WezTerm stdin pressure, small work areas, and localized/late GPU counters were not handled robustly. | Path equality protects the active install; terminal I/O uses one deadline; overlay bounds are normalized; PDH refreshes late instances and distinguishes unavailable from zero. | Focused platform tests and release validation passed; standard-path in-place upgrade passed. | Implemented; custom-path uninstall and Russian/German Windows remain unverified. |
| Capture and OCR ownership | Monitor changes could require another hotkey, SayFlex surfaces could enter the next image, cancelled OCR could lose its watchdog, and cached/full GDI frames had unsafe lifetime/synchronization edges. | Capture waits for rebuilt surfaces, hides owned surfaces before snapshot, bounds DXGI reuse, flushes GDI, and separates user cancellation from confirmed OCR termination/restart. | Desktop/platform tests and the shipped-worker restart benchmark passed. | Implemented; live monitor/DPI and forced-hang scenarios remain manual gates. |
| Protocol contracts | OCR geometry was flattened before translation, and context, completion, revision, and glossary data lacked one additive contract. | Protocol 31 adds bounded, round-trippable screen blocks, context, per-block completion, glossary, revision, and image-lifetime fields. | Protocol suite passed 24 tests and staged/installed protocol consistency validation passed. | Implemented and package-consistent. |
| Canonical screen document | ORIG, DUAL, ON IMAGE, correction, and late worker events could diverge. | One revisioned document owns immutable block IDs, OCR confidence/geometry, translations, corrections, context, pixels, and expiry projections. | State-transition tests cover stale events, duplicate/missing IDs, correction precedence, and expired pixels. | Implemented; expired pixels now normalize ON IMAGE back to text mode. |
| Source context | Largest rectangle intersection could select a large covered window instead of the visible source. | The window inventory is frozen with capture; visible regions are computed with occlusion in z-order, and ambiguous sources deliberately yield no context. Context is bounded and treated as data. | Geometry and collector tests cover overlap, ambiguity, no-URL/no-document fallback, and SayFlex exclusion. | Implemented; real game/browser/PDF context is not yet manually verified. |
| Screenshot retention | Selected images had no independent bounded lifetime. | Only the selected crop is stored atomically, for 24 hours and up to 500 MB by default; oldest crops are evicted, orphan cleanup runs, and explicit Save file is independent. Text and glossary survive pixel deletion. | Screenshot-store suite passed 11 tests, including expiry, size eviction, orphans, and save/delete behavior. | Implemented; wall-clock/disk-pressure behavior in an installed session remains unverified. |
| Personal glossary | Translation history did not represent user-approved contextual terminology. | A separate atomic store records only saved corrections or explicit confirmations, keyed/ranked by languages, phrase, and domain with examples and origin metadata. | Worker tests cover create/update/rank/edit/delete, domain conflicts, and failure preservation. | Implemented; installed restart persistence and live domain workflows remain manual gates. |
| Structured block translation | Plain text translation lost block identity and could not safely apply contextual terms. | Qwen receives bounded context/glossary data and returns unique `block_id -> translation` results with protected-value checks. Compact/cloud adapters expose limited capability instead of pretending to support instructions. | Structured translation tests cover IDs, missing/duplicate blocks, numbers, links, symbols, context injection, and fallback behavior. | Implemented. |
| Coordinator | Request state was distributed through desktop `main.rs`, making cancellation and late results difficult to reason about. | A coordinator owns request/revision transitions and wires capture, OCR, translation, document publication, storage, and termination. | End-to-end coordinator tests with fake boundaries and full desktop tests passed. | Implemented. |
| Visual states | The new interaction needed approval before production wiring. | Five Task 13 states were implemented in the existing SayFlex theme; ON IMAGE uses readable backplates/callouts, and correction is opened by double-click rather than a persistent button. | User approved the interaction during preview review; installed-candidate captures were inspected after frame readiness. | Implemented and visually approved at the agreed preview stage. |
| Production UI and correction | Preview states originally had static/demo-only interactions. | Real screen blocks now drive ORIG, DUAL, ON IMAGE, double-click correction, corrected copy, glossary management, expiry, Save file, and deletion. The editor owns request/block/revision drafts and retains failed saves for retry. | Desktop 292-test suite, worker/protocol/storage/localization suites, and installed UI captures passed their automated/visual checks. | Implemented; full installed interaction acceptance remains separate. |
| Aggregate resources | Per-worker rows could omit descendants and conflate unavailable GPU data with zero; users needed safe unload/reload behavior. | The Resources page adds aggregate and per-feature CPU/RAM/GPU/VRAM rows, refreshes descendants, protects active work, supports manual/idle release, and reloads selected engines on next use. OCR stays lazy and CPU-only. | Focused resource/platform tests passed. HY-MT2 was measured against Qwen through the production engine. | Implemented; HY-MT2 was rejected for this release and the game-active budget is unverified. |
| Release candidate | The integrated change needed coherent versioning, packaged assets, shipped-path OCR evidence, and installed visual evidence. | Version 0.8.3 was published to the staged bundle, Inno installer, development MSIX, release notes, manifests, and validation scripts. | See the exact automated, artifact, upgrade, and visual evidence below. | Installed candidate produced; it is not yet a signed public release. |

## Verified

### Automated source gates

- `cargo fmt --all -- --check`: passed.
- Workspace Clippy excluding `sayflex-speech-worker`, all targets/all features,
  with `-D warnings`: passed.
- Speech-worker Clippy passed separately for CPU, CUDA, and Vulkan. The literal
  workspace `--all-features` command is invalid because CUDA and Vulkan are
  intentionally mutually exclusive; the feature matrix is the equivalent
  valid release gate and preserves that compile-time guard.
- Workspace tests excluding speech-worker, all features: passed.
- Speech-worker CPU, CUDA, and Vulkan: each passed 44 unit and 4 protocol tests.
- `scripts/tests/ui-translations.Tests.ps1`: passed for all shipped locales.
- `scripts/tests/release-validation.Tests.ps1`: passed.
- Focused desktop Clippy with `-D warnings`: passed.

### Shipped OCR path

`scripts/benchmark-ocr.ps1 -Iterations 30` exercised the staged
`SayFlex.Ocr.exe` through protocol 31 after a real worker restart:

- 17 immutable fixtures, 30 iterations each;
- aggregate CER `0.046`, exact rate `0.824`;
- p50 `29 ms`, p95 `91 ms`, ordinary light/dark p95 `33 ms`;
- maximum sampled working set `387,854,336` bytes and observed peak
  `399,384,576` bytes;
- worker restart generation `2`.

The benchmark completed every row. Known limitations are retained in the
benchmark rather than normalized away: Hindi CER `0.147`, small mixed
Latin/Cyrillic CER `0.240`, and raw two-column order CER `0.395`. The canonical
geometry stage, not raw detector order, owns final column ordering.

Detailed evidence: [screen-translation-ocr-benchmark.md](screen-translation-ocr-benchmark.md).

### Packages and installed upgrade

- Final staged source identity:
  `d4eb84ab5c632a2f48c1b65eb9eb026f56fdf402`, version `0.8.3`, protocol `31`.
- Inno installer:
  `rust-app/dist/SayFlex-0.8.3-win-x64-setup.exe`,
  `568,215,938` bytes, SHA-256
  `052FF4E2F72385A5F84C49BFB2E7520B45A67CF96C0E5939EF4C83DC3DDDEB32`.
- Development-identity unsigned MSIX:
  `rust-app/dist/SayFlex-0.8.3-win-x64-development-unsigned.msix`,
  `631,341,102` bytes, SHA-256
  `07A0193ADD1D380535723A85F29B97DE8CA98FA1EB6D5AC2FC3594316A6BA534`.
- CPU/CUDA/Vulkan, OCR, translation, input, and companion binaries passed
  version, protocol, manifest, and hash validation in staging and after install.
- A standard-path in-place upgrade from installed 0.8.2 to 0.8.3 completed
  successfully. The installed tree matched the staged payload byte-for-byte,
  auto-started after reboot, and its desktop/workers were responsive.
- `scripts/smoke-installed.ps1` recorded artifact identity as `PASS=1`,
  `FAIL=0`; all 39 interaction/environment checks were honestly `NOT_RUN`.

### Installed UI evidence

These captures came from the installed 0.8.3 candidate, not a source-only
mockup. The separate preview is an ordinary non-topmost window.

| State | Evidence | SHA-256 |
|---|---|---|
| Image result | [installed-0.8.3-image-result.png](screen-translation-preview/installed-0.8.3-image-result.png) | `E94713EF65DAB7A45BCF729C86EA1C3815486EDA01B7FB0515EBD0011EC09864` |
| Editing | [installed-0.8.3-editing.png](screen-translation-preview/installed-0.8.3-editing.png) | `09E9A0176E247ABBBC790C8A338EF4D53106659299967AAF0B957175D504F0A9` |
| Saved | [installed-0.8.3-saved.png](screen-translation-preview/installed-0.8.3-saved.png) | `20911C0660DF59384EDB803653DE7B14875C418BAF49BF3A1A459267EF6205D1` |
| Save error | [installed-0.8.3-save-error.png](screen-translation-preview/installed-0.8.3-save-error.png) | `908F06CEE9DA068765483FA6054D04E3358DB2F14A9D82DDB2FC706730149A7D` |
| Expired image | [installed-0.8.3-expired-image.png](screen-translation-preview/installed-0.8.3-expired-image.png) | `9A19C6850E424C4A5C3D36E2C885D6D8C0346F7279427B03BCEC51FFA1049065` |

## Not verified

The following are not failures; they require interaction, time, hardware,
another Windows locale, external services, or Store/signing credentials that
were not available to this automated run:

- the 39 real installed interaction scenarios in the smoke receipt, including
  dictation and editor/browser/terminal insertion, delayed clipboard images in
  Firefox/Telegram, a live five-minute recording, cloud cancellation/fallback,
  real WezTerm timeout behavior, unload/reload interaction, and repeated hotkeys;
- live monitor attach/detach, DPI change, access-lost recovery, protected
  content, and a deliberately hung OCR process;
- real source-context extraction from a game, browser/site, and PDF, including
  no-URL/no-book fallbacks;
- end-to-end installed glossary domain separation, correction/confirmation
  rules, late-answer protection, and persistence after restart;
- screenshot expiry after 24 wall-clock hours, 500 MB disk eviction, and
  cleanup under actual disk pressure;
- long real-world ON IMAGE translations across representative displays;
- custom installation-path upgrade/uninstall preservation, fresh install, and
  complete uninstall behavior;
- GPU counter behavior on Russian or German Windows. The validation machine
  used culture `en-NL` and UI language `en-US`;
- Qwen + speech + OCR RAM/VRAM/latency and frame-time impact while a real game
  is rendering. The 10 GB ceiling and 8 GB target therefore remain open;
- Authenticode/SignPath signing and public Store submission. The MSIX uses a
  development identity and is unsigned because Partner Center identity and
  publisher credentials were not supplied.

## Deferred

- Vision-language OCR, generated-image translation, inpainting, and a
  continuously updating overlay over a moving game.
- HY-MT2 admission. Its footprint and speed were favorable, but it collapsed a
  blank paragraph and produced a Russian agreement error; it needs a pinned
  adapter and broader quality corpus before reconsideration.
- TranslateGemma and Parakeet model experiments; they are not required for the
  approved core scenario.
- A browser extension for guaranteed URL/page-context extraction.
- The Atom visual effect previously moved to a future update.

## Release boundary

The installed 0.8.3 build is a coherent technical candidate and replaces the
old installed 0.8.2. It is not represented here as a publicly signed release or
as having passed the unrun manual/environment-specific scenarios. The binary
manifest intentionally remains tied to `d4eb84a`; later evidence-only commits
do not require rebuilding an identical application payload.
