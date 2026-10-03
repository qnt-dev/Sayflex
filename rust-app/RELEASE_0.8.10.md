# SayFlex 0.8.10

Fixes from the review of the installed 0.8.9 (two independent reviews, every
mechanism re-checked against code, logs and git history). Plan and status:
[docs/superpowers/plans/2026-10-02-sayflex-0.8.10-review-fixes.md](docs/superpowers/plans/2026-10-02-sayflex-0.8.10-review-fixes.md).

## Shortcuts

- Settings keep the chords you chose, not what Windows managed to register.
  When another application holds a chord (Ctrl+Alt+P on the review machine),
  the row says so and the chord is retried on the next start. Previously the
  refused pause was saved as disabled, which reset every custom chord on each
  start and made the shortcut editor reject every new chord.
- Pause can be moved to a free chord while the old one is held elsewhere.
- A refused translate chord no longer switches the translation feature off.
- A row with a problem shows its full explanation instead of cutting it off.

## Dictation and translation

- A translation-worker exit no longer cancels a dictation in progress.
- Translations the exited worker can no longer answer end with "worker
  unavailable" instead of leaving the translator busy.
- Ctrl+Enter in the translator restarts a running translation without the
  replaced request resetting the window to "Translation cancelled".
- A paste the target already read is never reported as not delivered.
- "History cleared" appears only after both history files are gone; otherwise
  the remaining history stays visible and the reason is shown.
- With keep-warm off, Resources no longer shows an unloaded model as loaded.
- The compact (Firefox) translator's int8 SIMD kernel now builds on Windows:
  8 benchmark sentences 975 ms -> 114 ms, cold load 224 ms -> 64 ms.

## Screen translation

- Translations on the image sit on the picture's own colour, with dark text on
  light pages; the fixed dark-brown plates and a stray edge pixel are gone.
- Text mode (click the active view button again) scrolls a long result.
- "COPY TEXT" on an expired image confirms with "COPIED".
- The source-window context is collected again: the snapshot no longer waits
  on SayFlex's own UI thread (it missed its deadline on every capture).

## Privacy and diagnostics

- Cloud-speech failures and truncated provider replies no longer write any
  part of an API key to the log.
- Status messages no longer show internal session ids.
- Panic logs record module offsets that resolve against the PDBs kept from the
  build (`dist\symbols`, never installed).
- `cargo deny check` passes (yanked `chacha20 0.10.1` updated to `0.10.2`).
- Worker protocol version 33.

## Not changed on purpose

- The compact translation card keeps exactly the approved controls (ORIG,
  DUAL, PIN); it closes when you click elsewhere.

## Verification boundary

Automated: workspace `cargo fmt`, Clippy `-D warnings`, `cargo test`, `cargo
deny check`, the 8-locale UI translation check, offscreen renders of the
changed screens, an opt-in timing of the window snapshot on the build machine,
and the compact-translator benchmark. This local candidate is not
Authenticode-signed and is not a public release.
