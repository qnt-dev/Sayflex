# SayFlex 0.8.3

## Contextual screen translation

- `Ctrl+Alt+G` freezes the desktop, selects a region, performs CPU-only local
  OCR, and translates stable text blocks without losing their coordinates.
- ORIG, DUAL, and ON IMAGE read from one revisioned result. Long translations
  wrap or use a nearby callout instead of becoming unreadably small.
- Double-clicking a translated block opens correction. Saving a correction or
  explicitly confirming a result writes the personal context-aware glossary;
  ordinary translation, Copy, Pin, and Close do not.
- Temporary selected crops are bounded to 24 hours and 500 MB by default.
  Full-monitor frames are never persisted, and expired pixels do not remove
  translated text or glossary entries.

## Reliability

- Clipboard replacement and restoration revalidate publication ownership so a
  competing user Copy wins without losing text or images.
- Dictation cancellation, the five-minute recording limit, model switching,
  language selection, and local fallback retain one owner through terminal
  publication.
- Compact Firefox translation distinguishes EOS, output limit, cancellation,
  and failure. Incomplete output cannot be presented as complete or inserted.
- Screen capture handles monitor/DPI rebuilds, excludes SayFlex surfaces from
  the next frame, bounds DXGI reuse, flushes GDI drawing, and keeps the OCR
  watchdog active until computation really terminates.
- Installer path normalization protects an existing installation, and terminal
  capture uses one deadline for stdin, execution, stdout, and stderr.

## Context, glossary, and resources

- Context is derived from the actually visible source window and can include
  application/game identity, document/page details, and nearby text when
  available. Ambiguous or unavailable context is omitted.
- Qwen3.5 uses structured block IDs and glossary guidance. Compact Firefox and
  ordinary cloud adapters remain available but report their limited context
  capability honestly.
- Resources shows aggregate SayFlex CPU, RAM, GPU and VRAM together with
  speech, translation, and OCR rows. Releasing memory never deletes models;
  the selected engine loads again on next use. Active work is not evicted.

## Upgrade and verification boundary

Install 0.8.3 over the existing SayFlex installation. Settings, history,
downloaded model packs, runtimes, and glossary data live outside the program
directory and are preserved by an in-place upgrade.

Automated checks and artifact validation establish technical consistency, not
user acceptance. The final report for this candidate separately lists what was
implemented, verified, not verified on the available Windows environment, and
deferred.
