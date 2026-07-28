# Translation architecture

## Product contract

- Translation is optional and disabled until the user enables it.
- No translation model is bundled, preinstalled, or downloaded automatically.
- The system locale only marks a relevant model as recommended. English
  systems do not receive a default recommendation.
- `Ctrl+Alt+T` captures selected text once, translates immediately, and opens a
  compact preview without an intermediate command palette.
- The preview can copy the result or replace the original selected text.
- `Ctrl+Alt+K` remains the dedicated keyboard-layout correction shortcut.
- The full translator never needs access to another application's selection.
- History and warm-engine retention are independent user settings.

## Isolation

```text
Windows input host
  capture selection + immutable target
             |
             v
Slint desktop coordinator
  request identity + compact preview
             |
             v
Translation worker
  model manager -> authenticated llama.cpp -> Qwen3.5 -> history
             |
             v
Input host
  explicit clipboard copy or target-checked replacement
```

Translation inference never runs on the Slint event loop, the low-level
keyboard hook thread, the audio callback, or the dictation worker. A crash or
large model allocation in translation therefore cannot delay recording or
keyboard event processing.

Each translation receives a `RequestId`. The desktop stores the immutable
origin and target against that ID, and a result can only be routed to the
matching request. There is no mutable global "latest translation" used for
replacement.

## Model lifecycle

The catalog contains multilingual GGUF packs exposed initially for the
bidirectional English/Russian product contract. Every file has a pinned
upstream revision, expected byte length, and SHA-256. A pack is downloaded into
an isolated `.download` path with range-resume support, verified with a
streaming hash, and moved into place atomically. A failed or interrupted
download cannot replace a verified pack.

The current release exposes:

- Fast: Qwen3.5 4B Q4_K_M, about 2.8 GiB.
- Quality: Qwen3.5 9B Q4_K_M, about 5.7 GiB.

The pinned llama.cpp Vulkan runtime is a separate 32 MiB verified download and
is extracted atomically. It is shared by all translation models. The worker
launches it on a random loopback port with a random per-process API key,
disables the web UI and network model lookup, and inherits the desktop worker
Job Object. A worker or desktop failure therefore cannot leave an orphaned
translation server.

The catalog, protocol, model installer, runtime installer, and inference client
are separate components. Additional languages, model sizes, or native
backends can be added without changing input capture or UI delivery semantics.

## Latency policy

Keeping the engine warm is the default after the user has selected a model.
Warm-up primes both translation directions and the GPU execution graphs.
Turning it off unloads the model after every result and trades memory for
startup latency. Inference is deterministic (`temperature = 0`, fixed seed,
thinking disabled), entirely local after installation, and never runs on a UI
or input-hook thread.

Before public release, benchmark gates should record separately:

- model load and warm-up;
- prompt processing and generation per direction and text length;
- UI request-to-result;
- copy/replace dispatch;
- peak private memory for Fast and Quality.

Translation performance must not be mixed into the dictation release-to-insert
latency metric.
