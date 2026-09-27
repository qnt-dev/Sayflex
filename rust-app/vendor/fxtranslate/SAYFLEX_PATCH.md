# SayFlex fxtranslate patch

This directory is the exact `fxtranslate 0.4.2` crate source admitted from
crates.io, plus the narrow completion-reporting patch used by SayFlex.

- Original crates.io checksum:
  `e085babeb3347fe8296e1c4a30b0c5226a284428850746d8e281139909502eb2`
- Upstream license: MPL-2.0; retained verbatim in `LICENSE`.
- SayFlex change: greedy decoding records `DecodeTermination::Eos` or
  `DecodeTermination::LengthLimit`; checked direct, segmented, long-input and
  pivot APIs return the text together with every segment termination. Existing
  upstream APIs remain source-compatible and return only the text.

To update, download the newly selected crate version, verify its crates.io
checksum, replace this directory with that exact source, then reapply and review
only the completion-reporting diff before updating the workspace patch and lock
file. Run the vendored library tests and the SayFlex translation-engine tests.
