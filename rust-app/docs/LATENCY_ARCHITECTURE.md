# Latency architecture

Rust Voice treats release-to-insertion as a product contract, not a single
stopwatch around Whisper.

## User-visible contract

`Ctrl + Alt + Space` has two phases:

1. `Listening` begins on the accepted physical trigger edge.
2. `Processing` begins immediately on the physical release edge and remains
   visible until text injection has been dispatched.

Clipboard restoration, history persistence, model downloads, and model warm-up
are never part of the release-to-insertion critical path.

## Warm-path budget

The recommended CUDA profile is designed around this budget:

| Stage | Warm target |
| --- | ---: |
| Audio ownership/finalize | <= 15 ms |
| Resample/preprocessing | <= 10 ms |
| Prepared final tail | <= 250 ms |
| Safe target dispatch | <= 45 ms |
| Release to visible insertion | 150–300 ms typical |

The application records each stage independently in `TranscriptionLatency` and
reports final insertion dispatch from the Windows input host. Hardware, phrase
length, language detection, and target-editor behavior can move an individual
sample outside the typical range; no stage is allowed to hide that fact.

## Process and thread ownership

```text
CPAL callback
    -> wait-free SPSC ring buffer
    -> audio consumer actor
    -> speculation scheduler
    -> inference actor (resident, warmed Whisper context)
    -> desktop session registry
    -> insertion actor
         -> dispatch acknowledgement
         -> serialized clipboard restore

Completed transcript
    -> history writer actor
```

The audio callback performs no locks, allocation, filesystem access, JSON, UI,
or blocking IPC. The low-level keyboard hook likewise owns only physical-key
and gesture state.

## Incremental, correctness-guarded preparation

During a natural trailing pause the scheduler may request a cancellable
transcript segment. Every segment carries an absolute source range, session
identity, model/output-mode generation, and a conservative speech-tail guard.
Once a segment completes, its transcript and ending sample become the stable
prefix for that session.

Later preparation copies and decodes only audio after that stable sample. A
short bounded text prompt preserves vocabulary and sentence continuity across
the boundary; segment results are merged without introducing spaces before
punctuation or between CJK characters. The work therefore grows approximately
with the duration of new audio instead of repeatedly decoding the complete
1 s, 2 s, 3 s ... recording prefixes.

On release:

- an exact candidate with only a short silent tail is reused directly;
- otherwise only the unprepared tail is decoded and merged into the stable
  prefix;
- an invalid range, a sub-180 ms spoken tail, or a failed segmented decode
  triggers one authoritative full-recording pass.

The fallback is deliberate: latency targets never permit dropping a final
syllable or trusting inconsistent state. Continuous speech without a usable
pause also takes the authoritative path.

The default **fastest** profile may prepare every eligible natural pause, but
each completed source range is processed once and only one segment can be in
flight. The user-selectable **efficient** profile additionally caps preparation
at two segments per recording. Neither profile can build an inference backlog
ahead of the final release command.

## Clipboard transaction rule

Paste dispatch is acknowledged immediately after the target receives the owned
`Ctrl+V` sequence. The original OLE clipboard data object is retained without
eagerly rendering or copying its formats and restored later by the same
serialized actor. A second operation settles an unfinished restore first, so it
cannot preserve a previous transcript as the user's original clipboard.

## Release gate

Run the optimized worker against a representative WAV:

```powershell
.\scripts\benchmark-latency.ps1 `
  -WavPath C:\path\phrase.wav `
  -ModelPath "$env:LOCALAPPDATA\KeyboopRust\models\ggml-large-v3-turbo.bin"
```

The script reports median and p95 inference latency from warmed CUDA passes and
fails its short-phrase target when the median exceeds 300 ms. Longer audio is
reported without pretending inference time is independent of duration. Natural
pauses let the fastest profile keep the release path short without quadratic
reprocessing; uninterrupted long speech may still require a full final pass.
