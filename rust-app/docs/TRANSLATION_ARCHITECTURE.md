# Translation architecture

## Product contract

- Translation is optional and disabled until the user enables it.
- Translation runs on this machine by default. Settings → Providers holds a
  second, separate ordered list in which DeepL and Azure Translator can be
  placed above the local engine. Google Cloud Translation is deliberately not
  implemented; an old saved Google row remains visible only as unsupported so
  it cannot silently become a routing backdoor. Every cloud row is off until
  the user switches it on and supplies a key, and the list itself is what
  decides whether text leaves the machine — the claim the window makes is read
  from it rather than from a separate setting.
- No translation model is bundled, preinstalled, or downloaded automatically.
- The system locale only marks a relevant model as recommended. English
  systems do not receive a default recommendation.
- `Ctrl+Alt+F` captures selected text once, translates immediately, and opens a
  compact preview without an intermediate command palette.
- The preview can copy the result or replace the original selected text.
- `Ctrl+Alt+K` remains the dedicated keyboard-layout correction shortcut.
- The full translator never needs access to another application's selection.
- History and warm-engine retention are independent user settings.
- `Ctrl+Alt+G` captures a user-selected screen region. OCR is local and
  CPU-only; the text then follows the same ordered translation provider policy
  as other text.

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
replacement. A newer request increments the worker generation. The async HTTP
path observes that generation on a 10 ms cancellation clock while it is
waiting for response headers as well as while it reads the stream, so prompt
processing can be interrupted before the first token. Dropping the request
connection asks llama.cpp to stop the active task; stale partials and stale
final results are rejected independently by request identity.

Selected-text capture overlaps independent work instead of serializing it.
UI Automation starts as soon as the command is accepted. A usable read-only
UIA result is sent to the translation worker immediately, without waiting for
the physical hotkey release. Clipboard copy is attempted only after release
and only when UI Automation did not return a usable selection. Replacement
and copy delivery remain behind a request-scoped release gate, so early
translation can never inject input while the command chord is held.
Diagnostics keep release wait, UIA provider time, clipboard time, tokenizer
time, model first-token time, chunk progress, model completion and end-to-end
time as separate fields.

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

The key reaches the server through its environment rather than its command
line, because command lines of same-user processes are readable with ordinary
tools while an environment block requires `PROCESS_VM_READ`. The port is drawn
by binding an ephemeral socket and releasing it, so another process can take it
in the gap before the server binds; that shows up as an immediate exit and is
retried on a freshly drawn port up to three times. A start that is merely slow
is never retried, because the ninety-second readiness deadline would multiply.

Installation of a pack or of the runtime runs on the worker's installer thread,
not in its command loop. The worker keeps reading commands throughout, so a
download of several gigabytes can be cancelled, and shutdown does not have to
wait for it. A cancelled download keeps its partial file and resumes later.

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

The engine sends an ordinary selection as one request even when it contains
line breaks. Selections up to a conservative 1,000 UTF-8 bytes skip the
tokenizer HTTP round-trip entirely. Longer input is measured with the pinned
llama.cpp tokenizer and split at natural boundaries only above the explicit
1,200 source-token budget. This avoids the legacy per-line request multiplier
while leaving room inside the 4,096-token context for the instruction and
translated output. Chunks preserve their original whitespace boundaries and
publish their index/count as progress.

The system instruction is direction-independent and always occupies the first
message. Source language, target language and text live in the user message.
This keeps the longest reusable prompt prefix stable across English/Russian
direction changes without weakening the explicit translation contract.

llama.cpp responses use server-sent events. The worker coalesces partial text
to at most one IPC update every 30 ms (while allowing sentence boundaries
through immediately), and the Slint preview opens on the first visible text.
The final event remains authoritative for history, copy and replacement.
Closing or superseding a request cancels generation without restarting the
resident model.

The speech worker is isolated from this pipeline. Its prepared whisper.cpp
context owns one reusable inference state instead of reallocating CUDA/KV
buffers for every speculative or final pass. Native routine diagnostics are
filtered and application logs are written through a bounded asynchronous
queue, so diagnostics cannot add synchronous disk I/O to the hot path.

Benchmark gates record separately:

- model load and warm-up;
- physical hotkey release wait;
- UI Automation and clipboard capture;
- model first visible token;
- prompt processing and generation per direction and text length;
- backend request count and end-to-end completion;
- copy/replace dispatch;
- peak private memory for Fast and Quality.

Translation performance must not be mixed into the dictation release-to-insert
latency metric.

## Screen translation document

The desktop owns one revisioned screen document per request. It contains the
selected crop, physical-pixel coordinate system, stable `block_id` values, OCR
text and confidence, block bounds, translated text, user corrections, source
context, and image expiry. ORIG, DUAL, and ON IMAGE are projections of this
same document; they are not separate copies that can drift.

```text
Ctrl+Alt+G
  -> freeze visible monitor frames in memory
  -> select and crop one region
  -> SayFlex.Ocr.exe (CPU, coordinates + confidence)
  -> capture optional visible-window context
  -> TranslateScreenBlocks(block_id, text, context)
  -> validate exact IDs and protected values
  -> text projection / translated overlays on the selected crop
```

Window metadata is captured with the frozen frame. After selection, context is
chosen only from actually visible, non-occluded window regions. A large window
behind a smaller game therefore cannot win because of its hidden area. An
ambiguous result continues without context. URL, document name and nearby text
are optional; SayFlex does not synthesize Ctrl+L, replace the clipboard, or
inject into another process to obtain them.

The selected PNG and metadata use a 24-hour default lifetime and a 500 MiB
oldest-first budget. Full-monitor frames never enter the store. Pinning does
not extend pixel lifetime. Explicit Save file creates the only permanent image
copy. Expiry or Delete now removes pixels while retaining translated text and
approved glossary records.

## Context, backend capabilities, and glossary

Qwen receives immutable instructions separately from a typed JSON envelope of
screen blocks, optional context, and ranked glossary matches. The reply must
map each unique `block_id` exactly once; missing, duplicated, or unknown IDs,
truncated output, and changed protected numbers/URLs/placeholders are rejected
or retried within a bounded policy.

Firefox compact packs and ordinary cloud APIs are limited adapters. They
translate bounded blocks while preserving IDs, but report
`context_instructions=false` and `glossary_guidance=false`; SayFlex never
pretends that prepending service prose to source text implements either
capability. Compact decoder completion is typed per fragment as completed,
limit reached, cancelled, or failed. Anything not proven complete is excluded
from automatic replacement.

The personal glossary is actor-owned and independent of translation history
and temporary screenshots. Save correction and explicit Confirm are the only
write paths. Copy, Pin, Close, or an untouched machine result write nothing.
Entries are keyed by normalized phrase, language pair, and domain/application
scope; confirming again merges examples instead of creating a duplicate.
Lookup ranks exact language, expression, and domain matches without blindly
replacing substrings in generated text.

## Resource residency

Resources samples the desktop process and all descendants, then exposes an
aggregate row plus speech, translation, and OCR rows. CPU, RAM, GPU, and VRAM
remain distinct and missing GPU counters remain unavailable rather than zero.
Manual or idle release removes only resident engines, never verified files or
the selected model. The next operation reloads the selected engine.

Active capture, OCR, translation, installation, cancellation, and queued work
protect their engine from eviction. The current idle policy releases OCR after
five minutes, speech after fifteen minutes, and translation after fifteen
minutes only when Keep warm is disabled.
