# SayFlex 0.8.5

## Screen translation correctness

- OCR block rectangles now remain crop-local throughout image layout. Captures
  made on a monitor with a non-zero desktop origin no longer collapse every
  translated block into the upper-left corner.
- Correction saving releases the canonical document borrow before refreshing
  the UI, eliminating the re-entrant `RefCell` panic in the real Save path.
- Overlay text has a 12-pixel minimum and measured wrapping. Ordinary callouts
  avoid source regions and each other; dense results that cannot be arranged
  safely use one measured, scrollable side panel instead of overlapping text.
- The compact header has a 440-pixel minimum, and the expired-image state keeps
  its explanatory text and Copy action reachable.

## Persistence, privacy, and glossary behavior

- Screenshot pixels are written once and metadata revisions are serialized.
  A stale initial write cannot overwrite a completed translation or a user
  correction.
- Settings can reopen the latest unexpired completed screen translation after
  restart. The restored result remains an ordinary window until PIN is chosen.
- Expiry, Delete image, and Delete all release every matching in-memory Slint
  image, including pinned windows, while retaining text and glossary entries.
- Corrections can store an editable source phrase with the complete OCR block
  as its example. Later sentences match approved terms at word boundaries and
  rank game, site, document, application, and general context separately.
- Concurrent glossary actions keep independent operation identities, so an
  earlier acknowledgement cannot be discarded by a later click.
- Glossary phrases match later sentences at real word boundaries, including
  text in CJK scripts that does not use spaces between terms.

## Translation safety and responsiveness

- Structured screen translation is divided into token-bounded groups under one
  operation deadline. Capacity probing is logarithmic, cancellation covers
  request and response-body delivery, and malformed-group recovery is bounded.
- Numbers, signed values, URLs, placeholders, and protected symbols are checked
  as exact multisets. Substring changes such as `25` to `125` and dropped
  repeated values are rejected.
- A saved block keeps its real completion outcome across restart. A limited,
  cancelled, or failed block cannot be restored as a copy-safe completed result.
- Browser and PDF titles contribute bounded document context when available;
  selected OCR text remains explicitly untrusted contextual data. Translation
  continues without unavailable URL or document metadata.

## Verification status

This file is completed with exact source commit, installer hash, installed-file
identity, and witnessed Windows scenarios only after the candidate is built and
installed. Authenticode signing remains a separate public-release gate.
