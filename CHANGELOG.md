# Changelog

One file for the whole repository. Release notes that used to live in
`docs/RELEASE_*.md` and `rust-app/RELEASE_*.md` stay where they are as history;
new entries belong here.

Format: newest first, grouped by product. Dates are the day the work landed,
not the day it shipped.

## Unreleased

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
