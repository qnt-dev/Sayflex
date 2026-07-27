# Translation architecture

## Product contract

- Translation is optional and disabled until the user enables it.
- No translation model is bundled, preinstalled, or downloaded automatically.
- The system locale only marks a relevant model as recommended. English
  systems do not receive a default recommendation.
- `Ctrl+Alt+T` captures selected text once. In adaptive mode one enabled action
  runs directly; two or more enabled actions open the command palette.
- Translation can preview, copy, or replace the selected text.
- The full translator never needs access to another application's selection.
- History and warm-engine retention are independent user settings.

## Isolation

```text
Windows input host
  capture selection + immutable target
             |
             v
Slint desktop coordinator
  request identity + delivery policy
             |
             v
Translation worker
  model manager -> Candle/Marian -> history
             |
             v
Input host
  clipboard copy or target-checked replacement
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

The catalog contains bidirectional packs. Every file has a pinned upstream
revision, expected byte length, and SHA-256. A pack is downloaded into an
isolated `.download` directory, verified with streaming hashes, and moved into
place atomically. A failed or interrupted download cannot replace a verified
pack.

The current release exposes:

- Fast: two distilled OPUS-MT models, about 108 MB total.
- Quality: the two original OPUS-MT teacher models, about 623 MB total.

The catalog, protocol, and bundle installer are separated from the Marian
runtime so more language-pair engines or a multilingual backend can be added
without changing input capture or UI delivery semantics.

## Latency policy

Keeping the engine warm is the default after the user has selected a model.
Turning it off unloads the model after every result and trades memory for
startup latency. Translation uses deterministic beam search; no network call,
generative LLM, or UI-thread work is part of the request path.

Before public release, benchmark gates should record separately:

- model load and warm-up;
- tokenization;
- inference per direction and text length;
- UI request-to-result;
- copy/replace dispatch;
- peak private memory for Fast and Quality.

Translation performance must not be mixed into the dictation release-to-insert
latency metric.
