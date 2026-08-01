# SayFlex 0.2.0

This release replaces the synchronous post-recording pipeline with measured,
isolated actors. The C# reference application is unchanged.

## Latency contract

- `Ready` means that the selected model is installed, loaded, and warmed.
- Every transcript records audio-finalize, preprocessing, inference, and
  release-to-completion timings.
- Bundled workers negotiate protocol version 2 before accepting sessions.
- Insertion reports the dispatch method and dispatch duration independently.
- The overlay closes after text dispatch, not after clipboard restoration.

## Audio and inference

- The microphone callback performs bounded downmixing into a preallocated,
  lock-free SPSC ring buffer.
- A dedicated audio consumer owns all growing buffers and UI level events.
- A dedicated inference actor keeps the selected Whisper context resident.
- During a natural trailing pause, a cancellable speculative pass may prepare
  the final transcript. It is reused only when a conservative speech-tail guard
  proves that no spoken audio followed it.
- Resumed speech invalidates and aborts stale speculative work.

## Persistence and insertion

- Transcript completion is emitted before history persistence.
- The completed entry is projected into UI history immediately and then
  persisted with the same timestamp, so async durability cannot show the
  previous transcript as the newest one.
- History writes are serialized by a dedicated worker.
- Clipboard paste dispatch and clipboard restoration are separate lifecycle
  events.
- Clipboard transactions remain serialized so rapid consecutive dictations
  cannot restore another transcript as the user's original clipboard.

## UI delivery

- Worker messages wake the Slint event loop directly.
- Runtime state no longer waits for a 12 ms polling pump.
- The indicator animates at 60 Hz and enters Processing immediately on release.
