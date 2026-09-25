# Lightweight Russian-English translation evidence

Date: 2026-09-21

Status: admitted as an optional lightweight language package. It is not a
replacement for the larger contextual Qwen models: the measured package is
fast and resource-efficient, but its wording quality is visibly lower on some
sentences.

## Artifact under test

- Catalog id: `firefox-ru-en-v3.1`
- Runtime: `fxtranslate 0.4.2`, pure Rust CPU inference
- Model source: Firefox Translations v3.1 base-memory records from Mozilla
  Remote Settings
- License: MPL-2.0
- Download: 46,545,826 bytes
- Installed model data: 72,152,848 bytes
- Installation: on demand; not bundled in the SayFlex installer

Every source file is verified before expansion and every installed file is
verified again after Zstandard expansion. The exact six URLs, compressed
hashes, installed sizes, and installed hashes are pinned in
`crates/translation-engine/models/catalog.json`.

## Machine and build

- Windows 11 Pro 10.0.26200, build 26200
- Intel Core Ultra 7 270K Plus, 24 cores / 24 logical processors
- 68,139,642,880 bytes physical RAM
- NVIDIA GeForce RTX 5070 Ti and Intel Graphics present
- Rust 1.95.0, `x86_64-pc-windows-msvc`
- Command: `cargo run --release -p sayflex-translation-benchmark -- --install --include-text`

The benchmark uses the production `BundleInstaller`, the built-in production
catalog, and the same `TranslationEngine` adapter as the worker.

## Measurements

| Measurement | Result |
|---|---:|
| Cold load and bidirectional warm-up | 63 ms |
| Eight warm corpus cases | 1,101 ms total |
| Process CPU time, warm-up plus corpus | 1,046 ms |
| Working set before load | 17,563,648 bytes |
| Working set after corpus | 110,788,608 bytes |
| Working-set increase | 93,224,960 bytes |
| Fastest case | 28 ms |
| Slowest case | 355 ms |

The compact path starts no child inference process and does not initialize a
GPU backend. Windows PDH did not expose a numeric GPU/VRAM counter for this
short benchmark run, so GPU use is recorded as not available rather than
claimed as a measured zero.

## Completeness and representative output

All eight cases returned non-empty output in both directions. The two
multi-paragraph cases preserved both paragraphs and their blank-line separator;
no case was reported as truncated.

Representative results:

```text
EN: Alex sent 24 files to https://example.com/docs at 18:30 on 21 September 2026.
RU: Алекс отправил 24 файла на https://example.com/docs в 18:30 21 сентября 2026 года.

RU: SayFlex хранит ваш текст на этом компьютере и не отправляет его в сервис.
EN: SayFlex stores your text on this computer and does not send it to the service.
```

Quality caveat observed in the same run:

```text
EN: SayFlex keeps your text on this computer and does not send it to a service.
RU: несуть текст SayFlex хранит ваш текст на этом компьютере и не отправляет его в сервис.
```

This is why the interface labels the package as the small RU↔EN option and
keeps both contextual Qwen choices available. The compact pack passes the
latency, memory, bidirectional, URL/number, and completeness checks; it does not
pass a claim of contextual-model-equivalent translation quality.
