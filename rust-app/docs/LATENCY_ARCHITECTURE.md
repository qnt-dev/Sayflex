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
| Final Whisper pass | <= 250 ms |
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

## Correctness-guarded speculation

During a natural trailing pause the scheduler may request a cancellable
transcript candidate. The candidate carries:

- session identity;
- model/output-mode generation;
- source sample count;
- conservative speech-tail guard.

Any resumed speech invalidates the generation and aborts stale native inference.
On release, the candidate is reusable only when all subsequent audio is a short
silent tail. Otherwise a complete final pass runs over the authoritative
recording. Latency targets never permit dropping or guessing spoken audio.

The default **fastest** profile preserves low release latency across long
dictations by preparing a new candidate after later natural pauses. The
user-selectable **efficient** profile caps this work at two candidates per
recording. This resource/latency policy is explicit rather than silently
changing recognition or delaying every user. Both profiles allow at most one
preview in flight, so a pause cannot create an inference backlog ahead of the
final release command.

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
reported without pretending inference time is independent of duration; in the
interactive fastest profile, safe preview reuse is what keeps its release path
short.
