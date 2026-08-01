# SayFlex 0.3.0

This release adds a failure-isolated local translation subsystem and selected
text command palette. The C# reference application remains unchanged.

## Text actions

- `Ctrl+Alt+T` captures selected text once and routes it through an adaptive
  action policy.
- With one enabled action the command runs directly; with multiple actions a
  compact palette opens.
- `Ctrl+Alt+K` remains the direct, explicit RU/EN layout-correction shortcut.
- Translation supports preview, clipboard copy, and target-checked selection
  replacement.
- `Ctrl+Alt+V` recovery and the fast dictation path are unchanged.

## Local translation

- A separate translation worker runs Candle/Marian inference outside the
  input, UI, and speech processes.
- The full local translator and selected-text translation share the same
  request-identified engine.
- Fast and Quality bidirectional RU/EN packs are available by explicit user
  download.
- No translation model is bundled, preinstalled, or downloaded automatically.
- Locale detection marks a recommendation only and never selects or downloads
  a model.

## Model integrity

- Multi-file packs use pinned upstream revisions, exact byte sizes, and
  SHA-256 for every file.
- Hashing is streaming, including the 300+ MB Teacher weights.
- Installation uses isolated temporary directories and an atomic directory
  replacement.
- A verification receipt avoids rehashing unchanged installed models on every
  launch.

## Privacy and resource controls

- Translation and layout correction can be disabled independently.
- The translation engine can remain warm for low latency or unload after each
  result to reduce memory.
- Voice and translation history can be disabled and cleared.
- Translation text is not sent to the speech process or written to diagnostic
  logs.

## Protocol and packaging

- Protocol version 4 carries explicit text-action intent and translation
  request identity.
- The Windows installer now ships desktop, input host, CPU/CUDA speech
  workers, and the translation worker.
