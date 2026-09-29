# SayFlex Contextual Screen Translation and Reliability Design

**Date:** 2026-09-27  
**Status:** Approved for implementation
**Delivery:** One public application update after the complete scope passes its gates

## 1. Outcome

The next SayFlex update strengthens existing Windows workflows and adds a
context-aware screen-translation result that can be rendered over the captured
image, corrected by the user, and reused through a personal glossary.

The implementation is developed and verified in dependency-ordered slices, but
those slices are not separate public releases. Reliability fixes are completed
before dependent feature work, and the final installer is published only after
the integrated behavior passes automated, live-Windows, installed-upgrade, and
resource checks.

SayFlex remains a local Windows assistant for work between languages. The release
does not add continuous game overlays, visual-language models, image generation,
background reconstruction, inpainting, file transcription, subtitles, or new
long-recording modes.

## 2. Existing behavior that remains authoritative

The change preserves these existing capabilities and does not reimplement them:

- dictation results are stored before insertion, retained in history, and can be
  copied or reinserted with `Ctrl+Alt+V`;
- translation results support `ORIG`, `DUAL`, pinning, copying, history, and
  editing/retranslation in the standalone Translator;
- speech compute selection follows the existing CUDA to Vulkan to CPU policy;
- speech, translation, OCR, input, and window interaction remain isolated in
  their current process boundaries;
- models use pinned metadata, integrity verification, atomic installation, and
  explicit removal; cloud credentials remain protected by Windows DPAPI;
- OCR is enabled by default and already returns geometry, confidence, column
  grouping, and reconstructed reading order;
- the lightweight Firefox/Bergamot packages remain the low-resource translation
  mode, while Qwen remains the contextual mode;
- previously completed fixes for speculative timestamps, long-audio copying,
  interruptible model downloads, Qwen completion status, and sequential
  translation-model replacement are not reopened without a new reproducer.

The current screen-translation design remains authoritative except where this
document explicitly supersedes its earlier non-goals for screenshot persistence
and translation rendered over a frozen image. The lightweight translation-pack
design remains authoritative for package selection and compact-mode behavior.

## 3. Reliability work

Every correction follows the same evidence contract: reproduce the original
failure or establish a deterministic failing test, identify the owning state
transition, make the smallest owner-level correction, then verify the original
scenario and the surrounding regression suite.

### 3.1 Clipboard transaction ownership

The input host must distinguish an application-owned delayed-data materialization
from a genuinely new user copy. Clipboard freshness is revalidated while the
clipboard is locked immediately before destructive replacement and restoration.
The capture-to-replacement gap cannot overwrite a concurrent user copy.

Coverage includes Unicode text, ordinary and delayed-rendered images, private
formats that cannot be materialized, concurrent Copy, failed delivery, repeated
insertion, and restoration at 0, 1, 10, and 60 seconds. The continuously pumped
STA/OLE owner remains responsive while restored data is requested by another
process. Existing immutable snapshot/duplicated-handle behavior is preserved.

### 3.2 Speech operation ownership

- Reaching the five-minute recording limit stops capture and speculative
  recognition before the failure is published. A new recording can start
  immediately afterward.
- Cancellation identity remains live through final local or cloud recognition,
  history publication, UI delivery, and insertion. Late output from a cancelled
  session is discarded.
- Model installation revalidates operation identity after file verification and
  before activation.
- Selected language is configuration state, not engine-instance state, and
  survives unload/reload.
- Speech-model switching releases the old context before allocating the new one.
  Failure leaves a clear unloaded/error state; peak memory is measured rather
  than described as an assumed multiple.
- Exhausted cloud budget stops further network attempts but still permits an
  allowed local fallback regardless of provider ordering.

### 3.3 Translation operation ownership

- Firefox/Bergamot handles blank and whitespace-only fragments, including
  Unicode whitespace, without invalid ranges or worker loss.
- Compact translation has a bounded cancellation path for long paragraphs.
  Every translated fragment has an explicit terminal outcome: `completed`,
  `limit_reached`, `cancelled`, or `failed`. The adapter may publish a successful
  combined result only when every non-empty fragment is `completed`. The current
  library returns text without distinguishing normal completion from truncation,
  so its output is not treated as confirmed complete until the adapter can prove
  completion through an admitted backend contract or an independently validated
  completion mechanism. Splitting input into shorter fragments alone is not such
  proof. An unconfirmed, limit-reached, cancelled, or failed fragment may be shown
  with an explicit incomplete status for recovery, but it is never published as
  a successful terminal translation and never used for automatic replacement.
- History retrieval cannot block the command receiver behind an active engine
  lock. Cancellation, shutdown, and installation control remain responsive.
- Automatic external-text replacement is disabled for any incomplete result.

### 3.4 Screen capture and OCR ownership

- After monitor or DPI topology changes, selection surfaces are recreated and
  awaited; the current request continues without requiring a second hotkey.
- SayFlex-owned surfaces are hidden before capture, using the previous request
  identity until hiding is confirmed. Pinned surfaces are covered by the same
  policy.
- User cancellation and compute termination are separate states. OCR remains
  under its watchdog until the worker confirms termination; a stuck isolated
  worker can still be restarted.
- DXGI cached full-monitor frames have an explicit bounded lifetime. A completed
  request cannot retain all monitor contents without an active reuse contract.
- The GDI fallback synchronizes drawing before reading DIB memory.

### 3.5 Windows integration and telemetry

- WezTerm uses one deadline for input writing, execution, and output draining.
  stdout and stderr are serviced without pipe deadlock, and timeout terminates
  the child process. Ambiguous delivery is never repeated automatically.
- Translation-card geometry first constrains the card to the working area, then
  clamps a valid position. Small work areas, high DPI, and missing source
  rectangles leave all controls reachable.
- Installer legacy cleanup compares normalized old and current installation
  paths. It never deletes the active installation's licenses, assets, or
  uninstaller, including custom-path upgrades.
- GPU telemetry refreshes wildcard instances even while process IDs remain
  unchanged and distinguishes unavailable data from measured zero. Language-
  neutral counter names remain in source, but wildcard setup follows Microsoft's
  localized-PDH sequence: add the English wildcard with
  `PdhAddEnglishCounterW`, obtain its localized full path through
  `PdhGetCounterInfoW`, expand instances with `PdhExpandWildCardPathW`, then add
  each expanded localized path with `PdhAddCounterW`. Counter refresh repeats the
  required expansion when instances appear later. This behavior is verified on
  at least one English Windows installation and one non-English installation
  using Russian or German display language. A localized-path failure reports
  telemetry as unavailable rather than as zero. The required sequence follows
  the [Microsoft `PdhAddEnglishCounterW` contract](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw).

The governing principle is one owner per operation state: that owner controls
start, cancellation, terminal publication, resource release, and rejection of
stale results. New owner/coordinator modules are extracted from desktop
`main.rs` only where the touched workflow needs the boundary; unrelated
refactoring is out of scope.

## 4. Canonical screen-translation result

The screen path becomes:

```text
frozen capture
  -> OCR blocks with geometry
  -> source-window context
  -> block-aware contextual translation
  -> canonical result revisions
  -> text card and image rendering from the same data
```

One request-scoped result owns:

- request identity and monotonically increasing revision;
- selected crop, pixel dimensions, creation/expiry times, and current image
  availability;
- two explicit coordinate spaces: the selected crop has a physical-screen
  rectangle, while OCR block rectangles are crop-local. Crop-local rectangles
  are the only input to image-overlay layout; conversion to or from physical
  screen coordinates is explicit and never inferred from raw integers;
- stable block IDs, source text, OCR rectangles, reading-order relationships,
  and confidence;
- machine translation, user correction, and correction revision for each block;
- captured source context and glossary matches;
- persistence and deletion state.

Text presentation and image presentation are views over this result. They do not
maintain independent copies that can diverge.

The translation boundary consumes related blocks with shared context and returns
an exact mapping from `block_id` to translated text. The coordinator rejects
missing, duplicate, or unknown IDs and validates preservation of numbers, URLs,
placeholders, and protected symbols. Existing column and paragraph reconstruction
continues to determine reading groups. Low-confidence OCR is visually marked and
can be corrected before glossary confirmation.

Late model output may update only the machine layer for the matching request,
block, and revision. It cannot overwrite a user-edited revision.

## 5. Source context

The window inventory is frozen in the same capture generation as the desktop
pixels and before any SayFlex overlay appears. For every eligible top-level window
it records stable identity, physical bounds/region, DWM visibility and cloaking,
Z order, process identity, and the context fields available at that instant.
Context lookup after selection uses only this snapshot; it does not inspect the
then-active SayFlex overlay or a newer foreground window.

The source is selected from what was actually visible inside the chosen region:

1. reconstruct each candidate's visible physical-pixel region by clipping its
   saved window region to the desktop and subtracting the saved regions of visible
   eligible windows above it in Z order;
2. intersect each reconstructed visible region with the user's selection and
   calculate its visible area; pixels occluded by a higher window never count for
   the covered window behind it;
3. select a source only when one candidate has a unique largest visible area and
   either owns more than half of the accounted visible selection or owns the
   selection center with at least a 1.5-to-1 visible-area lead over the runner-up;
4. otherwise treat the result as ambiguous and continue without application
   context.

SayFlex-owned, invisible, cloaked, and ordinary tool windows are excluded before
occlusion is evaluated. Shaped windows use their saved window region; per-pixel
alpha that Windows does not expose reliably is not guessed. A selection spanning
several applications uses the unambiguous winner only and records that the context
is partial rather than merging unrelated application data. Tests cover a small
game window above a large browser, partially overlapping peer windows, shaped
windows, selections crossing applications, and an ambiguous split that produces
no context.

Standard Windows/application interfaces may provide:

- executable identity and window title for ordinary applications and games;
- recognized game identity and an admitted game-specific terminology set;
- browser identity, page title, accessible URL when available, and nearby
  accessible text;
- PDF viewer identity, document name, available metadata, and accessible current
  section text.

URL, book title, and surrounding text are optional. Translation continues when
they are unavailable. SayFlex does not synthesize `Ctrl+L`, replace clipboard
contents, inject into another process, or bypass protected applications to obtain
context. Browser extensions remain future work.

OCR and context are passed as typed, length-bounded data, separate from the
translation instruction. System instructions, structured user-data envelopes,
strict output schemas, ID validation, and output constraints reduce prompt
injection risk; the product does not claim that arbitrary untrusted text can be
made absolutely safe by prompt wording alone.

Application identity selects a domain but is not itself a glossary. Dota and
Counter-Strike terminology sets remain separate, reviewed assets. No terminology
is silently downloaded from arbitrary sources.

Compact Firefox/Bergamot translation cannot execute contextual instructions like
an LLM. Its UI and result metadata state that application glossary/context support
is limited. Context is not concatenated into translatable text to simulate support.

## 6. Personal glossary

A glossary record is created only when the user:

- changes a translation and successfully saves it; or
- explicitly confirms an unchanged translation.

Receiving, copying, pinning, closing, or inserting an ordinary machine result
does not change the glossary.

Each record contains source phrase, approved translation, language pair, source
example/fragment, domain/application/site/book context, origin (`corrected` or
`confirmed`), and creation/update times. The same phrase may have different
translations in different domains. Reconfirmation updates the matching entry and
adds a deduplicated example instead of creating duplicate rules.

Retrieval ranks exact language pair, expression, and context. Matches are supplied
to an instruction-capable backend as structured terminology guidance. SayFlex
never blindly replaces matching substrings in generated text.

The glossary is independent from bounded translation history and expiring images.
It supports list/search, edit, and delete. Deleting history or a screenshot does
not delete confirmed glossary entries.

Storage failure does not discard the in-memory correction. The editor shows a
retryable error, retains its draft, and does not claim the glossary write
succeeded.

## 7. Correction interaction

The existing SayFlex visual system remains authoritative: transient cards use
dark `Theme` surfaces, light text, and the warm accent; ordinary windows use
`Sheet`; current typography, spacing, accessibility, and focus behavior remain.

The correction flow is:

1. double-click a translated block to invoke correction; no separate
   **Correct** button is rendered over the image;
2. open a compact editor adjacent to the selection;
3. show an editable source expression (initially the whole OCR block), editable
   translation, example sentence, detected domain, and language direction;
4. **Save** applies a changed translation and writes the glossary entry;
5. **Confirm** stores an unchanged translation;
6. show a restrained **Saved to glossary** confirmation after persistence.

Word correspondence is never inferred from DUAL sentence ordering. The user can
select or amend the source expression in the editor. Correcting an entire block
stores a phrase pair rather than inventing word-level pairs.

The ordinary result card remains non-activating. Only an explicit double-click
or keyboard edit action opens a focusable editor while preserving the original
insertion target identity.
`Ctrl+Enter` saves; `Escape` cancels. An outside click neither silently saves nor
silently destroys the draft. Draft identity includes request, block, and revision.

Copy and subsequent insertion use the corrected revision. Text already inserted
into another application is not retroactively changed, and `Ctrl+Alt+V` retains
its existing recovery meaning. Long context scrolls, and the panel stays inside
the monitor work area.

Before production integration, the user reviews rendered mockups or live preview
states for: image result, editor, saved confirmation, persistence failure, and
expired image. Visual approval is distinct from automated correctness.

## 8. Translation rendered on the image

The result adds **On image** while preserving translation-only, `ORIG`, `DUAL`,
copy, and pin behavior.

The first version uses Slint `Image`, `Rectangle`, and `Text` over the immutable
selected crop. Each translated block uses its OCR rectangle after coordinate
conversion for image scale and monitor DPI.

- A calm opaque or nearly opaque theme surface covers original glyphs so source
  and translation do not overlap.
- Font size and wrapping use actual UI text measurement. Overlay text is never
  reduced below 12 logical pixels.
- Long text wraps. When the source rectangle cannot hold readable text, a nearby
  callout is placed inside the viewport and linked to the source rectangle.
- Layout resolves collisions between translated blocks and callouts. A compact
  result is at least 440 logical pixels wide, and its leading and trailing header
  controls never overlap at any supported DPI.
- Selected blocks receive a thin accent outline and expose correction.
- Pan/scroll/scale changes preserve exact block alignment.
- Missing or expired pixels degrade to the normal text result; translation and
  glossary remain available.

No second permanently composited image is stored. Rendering is reconstructed from
the crop and canonical block data each time.

## 9. Screenshot lifetime and privacy

Only the user-selected crop may be persisted. Full-monitor DXGI/GDI frames are
never written to disk and are released after crop/OCR handoff or request
termination, subject only to a short explicit reuse window.

Default temporary retention is:

- 24 hours from capture;
- 500 MB total temporary-crop budget;
- oldest files removed first when the budget is exceeded;
- cleanup during runtime and startup;
- orphan cleanup after crashes or interrupted writes.

The UI displays the expiry time and explains that the size cap can cause earlier
deletion. It offers **Delete now**, an option to disable screenshot history, and
an explicit **Save file** action for a permanent copy. The default is disclosed
where screenshot history is controlled. Screenshot retention is separate from
translation/dictation history retention.

Temporary storage is per-user, uses atomic writes and restrictive user access,
and never enters diagnostics, crash metadata, clipboard, or cloud requests.
Disabling screenshot history prevents future disk writes. Open or pinned windows
do not silently extend expiry; after deletion they retain text but release image
memory. Cleanup failures are logged without paths/content and do not block the UI.

One serialized persistence owner writes a crop's immutable pixels once and applies
later metadata revisions in monotonically increasing order. A stale initial write
cannot overwrite a completed translation or user correction. Every open or pinned
view observes image invalidation caused by explicit deletion, expiry, size-budget
eviction, disabling storage, or delete-all.

## 10. Models and aggregate resources

The target validation machine has 16 GB VRAM and 64 GB RAM while a game is
running. The SayFlex target is at most 8 GB VRAM for the intended combination and
must not exceed the user's 10 GB ceiling. These are validation requirements, not
claims about unmeasured usage.

- OCR remains CPU-only and starts lazily.
- Contextual translation begins with the existing Qwen3.5 4B Q4_K_M text model.
- No vision module is loaded.
- Firefox/Bergamot remains the low-resource path.
- Resource policy covers the aggregate of all SayFlex worker processes.
- Only required large engines remain resident; unloading preserves installed
  files and the next use reloads the chosen engine automatically.
- The Resources page distinguishes CPU activity, RAM, GPU activity, VRAM, and
  unavailable telemetry. It explains localizedly that unloaded functions reload
  automatically on next use.

Release measurements include cold/warm latency, peak and idle private memory,
aggregate RAM, CPU, GPU, VRAM, worker startup, cancellation, unload/reload, and
game-frame impact.

The first optional translation experiment is HY-MT2-1.8B using its official GGUF
and the pinned llama.cpp runtime. It is admitted only if license, template,
runtime compatibility, glossary adherence, quality, memory, and latency are
verified against Qwen 4B. TranslateGemma remains secondary research because it
uses Gemma terms and an image-text-capable architecture outside the current need.
Parakeet TDT 0.6B v3 is a separate future speech experiment. None of these model
experiments blocks the core release.

## 11. Delivery and verification

Development order inside the single release is:

1. reliability reproductions and fixes;
2. canonical screen-result, context, persistence, and glossary storage;
3. block-aware contextual translation and revision protection;
4. correction workflow and image rendering after visual approval;
5. aggregate resource policy, documentation, installed-package validation, and
   optional model benchmark.

The final report separates **implemented**, **verified**, **not verified**, and
**deferred**. A source inspection is not described as a runtime measurement.

Required checks include:

- every reliability issue has a failing pre-fix scenario and passing post-fix
  evidence, or an explicit proof that current code already closes it;
- source context follows the deterministic source-window rule and missing
  optional metadata never blocks translation;
- only correction/confirmation writes the glossary;
- domain-specific meanings do not conflict;
- edits survive late model output and application restart;
- deleting images/history leaves glossary records intact;
- overlays remain readable and aligned across DPI, scale, long text, and small
  work areas;
- cancellation/retry leaves no hung capture, OCR, speech, or translation owner;
- aggregate resources remain within the measured budget with a game running;
- history, recovery insertion, `ORIG`, `DUAL`, pinning, compact translation, Qwen
  translation, and the standalone Translator retain their existing behavior;
- direct-installer upgrade from the previous release and Store packaging preserve
  settings, history, downloaded assets, licenses, and uninstallability.

Release gates include focused Rust tests, workspace tests, formatting, linting,
release builds, installer validation, installed Windows scenarios, and screenshots
of the five approved UI states. README, SECURITY, third-party/model notices,
release notes, and the OCR benchmark are updated from the same application path
that ships.
