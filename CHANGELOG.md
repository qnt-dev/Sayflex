# Changelog

One file for the whole repository. Release notes that used to live in
`docs/RELEASE_*.md` and `rust-app/RELEASE_*.md` stay where they are as history;
new entries belong here.

Format: newest first, grouped by product. Dates are the day the work landed,
not the day it shipped.

## 0.7.1 - 2026-09-11

### Localization and translated layout

- Reworked the eight bundled non-English catalogs around UI meaning instead of
  literal word substitution. Brand and technical labels remain stable, action
  names now describe the operation they perform, and the History page no
  longer exposes the internal name `Insert-last`.
- Moved the interface-language selector from the global header to the About
  page, corrected every page index to the real nine-page total, and refreshes
  the idle status and configured dictation shortcut immediately after a locale
  change.
- Made translated backend cards and provider rows size from their actual text
  while keeping every action inside the page. List delegates and row identity
  columns are now explicitly bounded by the visible viewport.
- Catalog generation now rejects duplicate message identifiers and any change
  to the exact set of named placeholders instead of silently rewriting the
  translation. Regression tests construct every locale and verify semantic
  copy, page numbering, interactive bounds, engine-card separation, and
  provider-action geometry.

## 0.7.0 - 2026-09-11

### Release hardening

- Settings pages use Slint's native wheel/touchpad physics with one vertical
  scroll owner; the graphite rail remains a draggable projection.
- All five global shortcuts are typed, editable, atomically registered by the
  input host, conflict-checked, resettable, and persisted only after Windows
  accepts the complete set. Pause/resume now suppresses every other action.
- The interface and installer ship System, English, Russian, Ukrainian,
  Spanish, Brazilian Portuguese, German, French, Italian, and Korean locales.
- Provider drafts survive refresh and reorder. Speech and translation request
  queues are bounded; control commands remain responsive while network work is
  in flight, and superseded results cannot publish.
- Settings use flushed same-directory temporary files and Windows atomic
  replacement with recovery from primary, previous, or temporary candidates.
  Credential read failures are no longer mistaken for an empty key store.
- Translation shows its resolved direction, supports exact swap/retry, and
  anchors to selection, caret, pointer, then the monitor work area.
- Every selectable visual has a live transparent preview; custom controls have
  keyboard and accessibility contracts; About and generated dependency notices
  are included.
- Release tooling validates exact artifacts, supports Authenticode without
  accepting certificate passwords, records incomplete smoke checks explicitly,
  and requires signatures for public tag builds.

### SayFlex (rust-app)

Work from the architecture review of 31 August 2026. Every change below is
covered by `cargo fmt`, `cargo clippy -D warnings` and `cargo test --workspace`.

**Latency and responsiveness**

- The insertion command is now dispatched before the history list model is
  touched. Rebuilding up to 250 rows sat inside the release-to-insertion path
  that `docs/LATENCY_ARCHITECTURE.md` reserves for delivery. The list is also
  mutated in place, so it no longer loses its scroll position on every
  dictation.
- Microphone frames are coalesced into a single slot instead of queueing about
  thirty-one events a second on the UI thread. A stalled UI drops the backlog
  rather than replaying stale frames, and a completed transcript no longer
  waits behind them.
- Commands to workers are serialized by the caller and written by a thread that
  owns the pipe. A worker that stops reading can no longer block the UI thread,
  which previously had no timeout at all.

**Corrections after review**

The first version of the transcription work had defects that would have shipped
wrong behaviour; they are fixed here.

- Routing was two decisions that disagreed. A fast-path flag said "use the
  local engine" both when the user had put it first and when nothing at all was
  available, so switching the local engine off still produced local
  transcription. The same flag folded in the failure ledger, so one rate-limited
  service made every later recording bypass the router, and nothing recomputed
  it when the cooldown expired — a service that hit a limit once was gone until
  the settings changed or the application restarted. The flag now answers a
  question about the configuration only; everything else is the router's.
- Providers are no longer treated as interchangeable. A capability model knows
  that Azure's short-audio endpoint stops at sixty seconds, needs to be told the
  language and cannot translate; that only the OpenAI-compatible route has a
  translation endpoint, and that it needs the full model rather than turbo; that
  Groq caps an upload at 25 MB. A provider that cannot serve a recording is
  stepped over untried and without penalty.
- The language reaches the services. Azure was being told en-US for every
  dictation and Deepgram was never asked to detect one, so Russian speech came
  back as English nonsense.
- Translate-to-English is honoured on the network path instead of silently
  transcribing.
- Silence is a failed dictation, not an empty success, matching the local
  engine.
- The deadline belongs to the recording rather than to each provider, and a
  cancelled dictation stops the walk instead of arriving late.
- The interface no longer claims audio stays on the device while a service is
  enabled, and no longer reports a key as stored before the worker has written
  it. The key store writes before it adopts, so a failed write cannot leave a
  key that works until the next restart.
- A custom endpoint must be HTTPS; response bodies are read with a limit; a key
  echoed back inside an error body is removed from diagnostics; a 403 is only a
  key problem when the service says so; Retry-After is capped; the provider list
  is deduplicated and bounded.
- A cancellation arriving while an install request was still queued no longer
  vanishes.

### Repository

- The desktop monolith is being split. `apps/desktop/src/main.rs` went from
  9,300 lines to 3,620 by moving — not rewriting — the visualizer, the worker
  supervisor, the preview modes, the translation preview, the provider list and
  all window wiring into modules of their own. Behaviour and tests are
  unchanged.
- `platform-api` is removed. It defined capability traits nothing implemented
  and nothing called, promising a portability the code did not have.
- The speech worker gains a hardware-free diagnostic mode so the protocol can
  be tested against the executable the product actually ships.

**Transcription through API services**

- Dictation can go to a network service instead of the local engine: Groq,
  Cloudflare Workers AI, Azure Speech, Deepgram, or any OpenAI-compatible
  endpoint. Each has a free allowance; the point of the list is to use the next
  one when the one before it runs out.
- The order is the user's, arranged by dragging rows in Settings. The local
  engine is a row like the others and can be moved or switched off. A row can
  be switched off without losing its place or its settings.
- Failing over is decided by what the service answered, not by a local counter:
  a refused key takes a row out until it is replaced, a spent allowance holds it
  until the allowance resets, throttling honours Retry-After, and a request the
  service rejected as malformed stops the walk instead of spending every other
  quota on audio nobody will accept.
- Keys are encrypted with DPAPI in user scope and kept in their own file, never
  in settings.json. Settings shows a masked tail and never the value.
- The local path is unchanged. While no service is ahead of the local engine —
  a fresh installation, or one with no keys — a finished recording goes
  straight to inference as before: no audio encoded, no thread crossed.
- The worker reports which service produced a transcript. Audio leaving the
  machine is not something to infer from a settings screen.
- Protocol version 15 carries the list, the keys and each row's availability.

**Model installation**

- Installation runs on each worker's own installer thread. Both workers used to
  download gigabytes inside their command loop, which meant they read nothing
  else for minutes.
- Protocol version 14 adds `CancelModelInstall`. An installation can be stopped
  within about a megabyte, choosing a different model or tier cancels the one
  in flight, and shutdown cancels before asking a worker to exit. Partial
  downloads are kept and resumed.
- A visible cancel control in Settings is not part of this work.

**Translation runtime**

- A lost race for the local server port is retried on a freshly drawn port, up
  to three times, instead of surfacing as a ninety-second timeout.
- The server's API key travels in its environment rather than on its command
  line.

**Settings**

- One typed document with a `schema_version`, replacing a hand-written parser
  and a separate `json!` literal that had to be kept in step by hand. Reading
  is field by field on purpose: one unusable value no longer resets every other
  preference. Unknown fields are ignored, so a document from a newer build stays
  readable.
- Migration tests the format never had: a document from the previous build,
  absent fields, unknown fields, an unparseable document, out-of-range values,
  and a save/load round trip.

**Tests and build**

- First cross-process test in the workspace: a stand-in worker binary proves
  that commands arrive whole and in order, and that a worker which stops reading
  produces an error rather than a hang. No model, microphone or GPU needed.
- The quality workflow also triggers on `desktop-pet/**`, because the companion
  renderer builds against crates in that workspace.
- The single HTTP artifact fetcher lives in `model-manager`; the two byte-identical
  copies in the workers are gone.
- Path dependencies no longer declare version 0.5.12 while the workspace is at
  0.5.27.

### Repository

- The working tree is under version control again: 119 uncommitted paths were
  inventoried and committed, including two source files that were untracked and
  without which a fresh clone did not compile.
- `design/concept-form-system` and `design/concept-minimal` are tracked. The two
  web prototypes keep their own git history and are excluded rather than
  embedded as gitlinks.
- `__pycache__`, prototype dependencies and prototype build output are ignored.

## History

Earlier notes: `docs/RELEASE_0.1.0.md` … `docs/RELEASE_0.5.2.md` and
`rust-app/RELEASE_*.md`.
