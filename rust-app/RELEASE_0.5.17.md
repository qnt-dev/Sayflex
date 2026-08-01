# SayFlex 0.5.17

This release removes the long-text insertion bottleneck in terminal targets
and bounds speculative speech work without changing the installed Whisper or
translation models.

## Faster, application-aware insertion

- SayFlex now selects the native paste shortcut for the foreground
  application;
- WezTerm and modern terminal hosts use `Ctrl+Shift+V`;
- classic terminal hosts use `Shift+Insert`;
- standard editors continue to use `Ctrl+V`;
- Unicode character injection remains an automatic fallback instead of the
  mandatory WezTerm path.

This changes long WezTerm insertion from per-character input to one guarded
clipboard transaction.

## Predictable long-dictation finalization

- speculative work is bounded by the uncommitted audio tail rather than the
  total dictation duration;
- committed stable speech segments remain reusable;
- an unbounded preview can no longer compete with final transcription after
  key release;
- model choice and recognition quality settings are unchanged.

## Diagnostics

- insertion diagnostics now separate physical-key release wait, paste method
  time and total dispatch time;
- speech latency records expose the final inference queue wait;
- the desktop writes complete release-to-insert stage timing into the rotating
  application log;
- existing history files remain backward compatible.

## Validation

The release gate includes workspace tests, Clippy with warnings denied, release
publishing, release file-set and SHA-256 validation, installer creation, and
live insertion checks in Notepad and WezTerm.
