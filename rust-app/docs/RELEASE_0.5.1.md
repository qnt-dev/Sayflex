# SayFlex 0.5.1

This release hardens the Windows input, privacy, recovery, model, and speech
pipelines without adding latency to the visible dictation path.

## Reliability

- Input services are ready before the global hook is installed.
- Every worker belongs to a kill-on-close Windows Job Object.
- Failed workers restart with a bounded policy and receive the current settings.
- Physical key state reconciles after desktop or hook event gaps.
- AltGr and right-side modifiers cannot activate the default command chord.

## Privacy and insertion

- Temporary paste data opts out of clipboard monitoring, history, and cloud sync.
- Rich clipboard state is retained as its OLE data object instead of copying every
  format into application memory.
- Clipboard restore no longer blocks the insertion worker for 750 ms.
- Native password edit controls reject dictation and selected-text actions.

## Speech and models

- Preview and final Whisper passes use identical segmentation.
- Release optimization is explicit: fastest preserves pause-time preparation;
  efficient caps speculative work at two passes per session.
- Audio resampling is band-limited and tested for alias rejection.
- Idle audio and speculation threads park instead of polling.
- Recordings have a five-minute memory bound.
- Interrupted model downloads resume with HTTP Range and bounded retry.

## Distribution

- Rust 1.95.0 is pinned.
- Windows manifests declare per-monitor DPI, long-path, UTF-8, and supported OS
  behavior.
- CI enforces formatting, tests, Clippy warnings, advisories, licenses, bans,
  and registry sources.
- The Slint royalty-free attribution is present in the top-level About page.
