# Third-party translation models

Translation models are not distributed inside the installer. The application
downloads a user-selected pack directly from the pinned upstream revision and
verifies every artifact before installation.

| Catalog entry | Upstream model | Pinned revision | License |
| --- | --- | --- | --- |
| 13 compact English pairs | Firefox Translations v3 base-memory/tiny models | Mozilla Remote Settings records pinned by artifact URL, compressed and expanded byte length, and SHA-256 | MPL-2.0 |
| Fast multilingual | `Qwen/Qwen3.5-4B`, GGUF by `bartowski` | `4168f45a16a1290d65a4ec0fa312ae917a4c15d6` | Apache-2.0 |
| Quality multilingual | `Qwen/Qwen3.5-9B`, GGUF by `bartowski` | `182be2fd6c7bc44887d88a91cb03ff009cc9f549` | Apache-2.0 |

The compact catalog provides independent bidirectional packages between
English and Russian, Ukrainian, Spanish, Portuguese (Brazil), German, French,
Italian, Simplified Chinese, Traditional Chinese, Japanese, Korean, Hindi, and
Arabic. Each package uses the MPL-2.0 `fxtranslate` Rust runtime and Firefox
Translations model artifacts. A package is installed only when the user
chooses it or first requests its language pair, expanded from the upstream
Zstandard payloads, and verified twice: first against the compressed upstream
checksum and then against the expanded-file checksum. Installing one package
does not remove any other package. Neither the models nor an extra Python,
WebAssembly, or MKL runtime is bundled in the SayFlex installer.

- Firefox Translations models: <https://github.com/mozilla/translations>
- fxtranslate: <https://github.com/gregtatum/translations>

Inference uses the official llama.cpp Windows Vulkan archive from release
`b10164`, downloaded from `ggml-org/llama.cpp` and verified before atomic
extraction. llama.cpp is MIT-licensed.

The exact URLs, compressed and installed sizes, and SHA-256 values used by the installer live in
`crates/translation-engine/models/catalog.json`.

## Screen OCR bundle

Unlike optional translation packs, the ordinary SayFlex payload includes the
CPU-only `screen-ocr-ppocrv5-mobile-v3.9.2` bundle: one PP-OCRv5 detector,
seven script-family recognizers, and their dictionaries distributed by
RapidOCR under Apache-2.0. Every artifact has an exact byte length and SHA-256
in `crates/model-manager/src/ocr_catalog.rs`; publishing creates and validates
`bundle.verified.json`. OCR does not enable DirectML or CUDA.

The release benchmark drives the packaged `SayFlex.Ocr.exe` through the same
versioned JSON protocol as the desktop. Current measurements and known small
text, Hindi, and raw multi-column limitations are recorded in
`../../docs/evidence/screen-translation-ocr-benchmark.md`.

## Evaluated but not admitted

HY-MT2-1.8B Q4_K_M remains an experiment, not a catalog entry. It used less
memory and was faster than Qwen3.5 4B in the recorded probe, but lost a blank
paragraph boundary and produced a Russian agreement error. The exact revision,
hash, measurements, and rejection are recorded in
`evidence/contextual-translation-resources.md`. No HY-MT2 files are bundled or
retained.

Before publishing a catalog update:

1. Review the model card and license at the exact pinned revision.
2. Verify that commercial redistribution or user-directed download matches the
   intended release channel.
3. Recompute byte lengths and SHA-256 independently.
4. Run an interrupted-download test and both translation directions.
5. Update this notice and any attribution shown by the public download page.
