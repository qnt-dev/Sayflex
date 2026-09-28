# Contextual translation resource evidence

Date: 2026-09-28

Status: aggregate accounting and idle-residency policy implemented and tested.
The HY-MT2 candidate is **not admitted**. The game-active release matrix remains
a required installed-candidate check; no game was running during the model
comparison below, so game frame impact is not claimed.

## Resource ownership and idle policy

The Resources page samples the desktop process and all descendants as one
`SayFlex total` group. Speech, translation, and OCR remain separate rows. This
captures late child processes such as `llama-server.exe` without adding their
usage twice to the total. CPU, RAM, GPU, and VRAM are reported independently;
an unavailable Windows counter is rendered as `—`, never as a measured zero.

Automatic release is conservative:

- OCR: five minutes idle;
- speech: fifteen minutes idle;
- translation: fifteen minutes idle only when `Keep warm` is disabled;
- an active request, queued request, model installation, or release operation
  is never selected for eviction;
- activity resets the idle deadline;
- the installed selection is retained and the existing lazy-reload path loads
  the selected engine again on its next use.

Focused evidence:

```text
cargo test -p sayflex-desktop resource_view -- --nocapture
6 passed; 0 failed

cargo test -p rust-platform-windows process_resources -- --nocapture
3 passed; 0 failed; 1 ignored (interactive live-PDH probe)
```

The platform tests cover descendant aggregation, refreshed late GPU instances,
localized PDH wildcard setup, and unavailable-versus-zero semantics. The
desktop tests cover active-operation protection, idle deadlines, activity
reset, keep-warm protection, aggregate-row non-unloadability, and unavailable
metrics.

## HY-MT2 1.8B candidate

Official artifact checked:

- repository: <https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF>;
- pinned revision: `a0c709d9fac510f2c807aa3af52872340dc37a4a`;
- file: `Hy-MT2-1.8B-Q4_K_M.gguf`;
- bytes: `1,133,080,448`;
- expected and measured SHA-256:
  `dc5f44fcf1fa496ee7ad725982c0c8c553a4de00259b53af84c4b89fb0c06699`;
- license: Apache-2.0, as declared by the official repository;
- runtime: the installed SayFlex `llama-b10164-vulkan` runtime, through the
  production `TranslationEngine`, not a separate inference harness.

The official model card documents the llama.cpp route and its translation
instruction format:
<https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/blob/a0c709d9fac510f2c807aa3af52872340dc37a4a/README.md>.

### Same-machine comparison

These are process-cold runs after the binaries and model files had already been
read on this boot. They are not cold-boot disk benchmarks. No game was active.
Windows exposed VRAM bytes but not a numeric instantaneous GPU percentage.

| Measurement | HY-MT2 1.8B Q4 | Qwen3.5 4B Q4 |
|---|---:|---:|
| Runtime start + two-direction warm-up | 1,546 ms | 4,120 ms |
| Eight warm RU/EN cases | 609 ms | 10,349 ms |
| First partial result | 9-11 ms | 183-205 ms |
| Aggregate working-set increase | 1,350,238,208 B | 4,475,514,880 B |
| Aggregate VRAM | 1,503,055,872 B | 2,483,023,872 B |
| Structured context/glossary probe | 2,545 ms | 14,032 ms |
| In-flight cancellation | pass | pass |
| Approved glossary phrase | pass | pass |
| Numbers, URL, and time preserved | pass | pass |
| Two-paragraph structure | **fail** | pass |

HY-MT2 changed each blank paragraph separator into spaces plus a single line
break, so the result failed the same completeness rule used by SayFlex. It also
produced the grammatically incorrect Russian phrase `на этой компьютере` in
the ordinary sentence case. Its speed and footprint are attractive, and its
structured glossary response used `время перезарядки` correctly, but the
candidate does not meet the current release quality gate.

Decision: keep HY-MT2 outside the catalog and third-party notices. Revisit only
after a pinned prompt/template adapter and a broader quality corpus eliminate
the observed regressions. The 1.13 GB benchmark copy was removed after hashing
and measurement.

## Game-active release matrix

Not verified in this session because no game workload was active. Before the
installed candidate is accepted, measure Qwen 4B + speech + OCR in cold, warm,
idle, unload, and reload states while a real game is rendering. Record private
bytes, aggregate RAM, GPU, VRAM, request latency, and frame-time impact. The
gate is at most 10 GB SayFlex VRAM, with an 8 GB target. Estimates or an idle
desktop run must not be substituted for that measurement.
