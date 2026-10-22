# SayFlex fxtranslate patch

This directory is the exact `fxtranslate 0.4.2` crate source admitted from
crates.io, plus the scoped SayFlex changes documented below.

- Original crates.io checksum:
  `e085babeb3347fe8296e1c4a30b0c5226a284428850746d8e281139909502eb2`
- Upstream license: MPL-2.0; retained verbatim in `LICENSE`.
- SayFlex change: greedy decoding records `DecodeTermination::Eos` or
  `DecodeTermination::LengthLimit`; checked direct, segmented, long-input and
  pivot APIs return the text together with every segment termination. Existing
  upstream APIs remain source-compatible and return only the text.

- SayFlex change (2026-10-02): the gemmology SIMD shim builds with MSVC.
  `build.rs` passes `/arch:AVX2` to MSVC instead of the GCC-style `-mavx2`
  (which MSVC ignores), and `src/gemmology_shim.cpp` allocates its 64-byte
  aligned buffers with `_aligned_malloc` / `_aligned_free` under MSVC, whose
  C runtime has no `std::aligned_alloc`. Before this, every Windows build
  silently fell back to the scalar kernel.
- `src/gemm.rs`: `PreparedB` is `Send`. It owns plain heap memory freed with
  the C runtime's (aligned) free, and the shim keeps no per-thread state; the
  SayFlex translation worker holds its engine in a `Mutex` shared between
  threads, which needs `Send` once the SIMD kernel is live.

- SayFlex change (2026-10-10): `vendor/gemmology/gemmology.h` specializes the
  AVX2 shifted unsigned-byte/signed-byte dot product. It splits each unsigned
  activation into low seven bits and its high bit, widens each adjacent-product
  sum separately, then adds in int32. This prevents `maddubs` int16 saturation
  on the admitted models' full-range weights. The existing random full-range
  parity gate now includes AVX2, and a new adversarial multi-row/padded-column
  regression requires exact scalar equality. No second matmul or buffer is used.

- SayFlex change (2026-10-10): target decoder position zero starts with a zero
  token embedding before positional encoding, matching Marian's empty history
  in `DecoderBase::embeddingsFromPrediction` and its shifted training input.
  The common embedding path covers scalar, timed and active-batch decoding;
  source position zero and subsequent target positions still use their learned
  embeddings. A tiny synthetic nonzero embedding fixture exercises these
  boundaries without depending on a downloaded model.
  Reference: <https://github.com/mozilla/translations/blob/main/inference/marian-fork/src/models/decoder.h>.

- SayFlex change (2026-10-10): consecutive SentencePiece byte pieces decode as
  UTF-8, rather than literal `<0xNN>` text. Non-byte pieces end a byte run, raw
  byte whitespace/`▁` is preserved, and every invalid byte produces U+FFFD as
  in the reference. Tests cover valid multibyte text, malformed/truncated bytes,
  control boundaries and the distinction from normal whitespace marker pieces.
  Checked against official SentencePiece 0.2.2 with the admitted EN-RU vocab;
  encoding is unchanged.
  Reference: <https://github.com/google/sentencepiece/blob/master/src/sentencepiece_processor.cc>.

To update, download the newly selected crate version, verify its crates.io
checksum, replace this directory with that exact source, then reapply and review
the documented completion, Windows build, exact-AVX2, decoder bootstrap and
SentencePiece byte-decoding changes before updating
the workspace patch and lock file. Run the vendored library tests and the SayFlex
translation-engine tests.
