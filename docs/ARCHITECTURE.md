# Architecture

```text
Keyboop.InputHost.exe
  dedicated WH_KEYBOARD_LL thread
    -> Ctrl+Win push-to-talk / repeat-last / pause hotkeys
    -> optional experimental auto-correction pipeline
    -> default mode passes ordinary typing through without text buffering
          |
          | authenticated current-user named pipe
          v
Keyboop.UI.exe (.NET 10 WPF)
  tray / settings / selection hotkey / preview
          |
          | versioned named pipe
          v
Keyboop.SpeechWorker.exe
  WASAPI / quality gate / Whisper CUDA
          |
          | accepted transcript + detected source language
          v
  optional local NMT translation backend
```

No per-keystroke IPC is permitted. The InputHost owns keyboard state and has enough
local configuration to continue safely when every other process is unavailable.

## Manual selection transaction

1. Windows delivers `Ctrl+Alt+K` to the UI through `RegisterHotKey`.
2. Capture the foreground target immediately, then wait until Ctrl, Alt, and K are
   physically released so the copy chord cannot become `Ctrl+Alt+C`.
3. Snapshot the clipboard sequence and data object.
4. Send `Ctrl+C`, require an observed sequence change, and read Unicode text.
5. Build deterministic RU/EN layout alternatives locally and show a topmost preview.
6. Escape restores the clipboard and performs no target edit.
7. An explicit candidate restores the original target, pastes the replacement, and
   restores the old clipboard only if the operation still owns the sequence.

The transaction never monitors ordinary typing and never auto-applies a candidate.

## Runtime choices

- Core and InputHost: C#/.NET 10, with NativeAOT publish evaluated after compatibility
  tests. The hook callback is allocation-free and queues compact value events.
- UI: WPF on .NET 10.
- Speech: Whisper.net over whisper.cpp. CUDA is preferred; CPU remains a fallback.
- Audio: Windows Core Audio (WASAPI).
- IPC: named pipes restricted to the current user, length-prefixed JSON during early
  development; a source-generated serializer and strict message limits are required
  before release.
- Settings: versioned JSON under `%LOCALAPPDATA%`; secrets use DPAPI.
- Voice history: opt-in atomic JSON store, capped at 200 successful entries.

## Experimental automatic replacement transaction

1. Record the physical boundary event and snapshot target window/thread/layout.
2. Decide synchronously from in-memory detector data.
3. If correction is required, enter a short bounded transaction.
4. Queue later physical events up to a hard event/deadline cap.
5. Revalidate target identity, focus, layout, and supported-field policy.
6. Submit deletion, Unicode replacement, and boundary in one `SendInput` array.
7. Replay queued physical events in original order.
8. On any failed precondition, replay the untouched original sequence.

Transactions never wait on the UI, speech worker, disk, network, or database.

## Speech strategy

The target machine has an Intel Core i9-10850K, 64 GB RAM, and an NVIDIA RTX 3080 Ti
with 12 GB VRAM. The primary profile therefore keeps `large-v3-turbo` resident on
CUDA. Holding left Ctrl+left Windows starts WASAPI capture into a temporary local
WAV file. Releasing either key closes the capture and runs final local recognition.
The current release does not claim streaming partial transcription or Silero VAD.

The capture file is bounded to 20 minutes, with a warning at 19 minutes, and is
deleted after conversion for recognition. Admission gates reject very short, quiet,
or punctuation-only results. Final text is inserted through a guarded clipboard
transaction: materialize the existing formats, paste into the still-focused target,
and restore only while Keyboop still owns the clipboard sequence. Unicode
`SendInput` is the fallback. WezTerm uses the previously verified direct Unicode
path for ordinary live dictation.

When history is enabled, SpeechWorker atomically stores the successful transcript
before returning it to InputHost, so target loss cannot destroy recognized text.
The store is `%LOCALAPPDATA%\Keyboop\dictation-history.json`, capped at 200
entries, and contains text plus language and timing metadata but no audio.
`Shift+Alt+X` is held as a pending recovery chord until all three physical keys
are released.

InputHost assigns an immutable `sessionId` when capture begins. The hook and the
physical-key watchdog submit intents through the same synchronized session owner
and single-reader action channel. `Begin`, idempotent `End`, the final transcript,
history, insertion, and recovery all carry this ID. SpeechWorker retains one
completion task per session; concurrent or repeated `End` requests share it, and a
recovery request waits for that exact task even if it reaches the named-pipe server
before `End`. In that case recovery may own the same idempotent finalization itself,
so a delayed or lost standalone `End` cannot leave it waiting forever. Completion
order therefore cannot redefine which text “last” means.
The global last-transcript query remains only as a restart fallback when InputHost
has no in-memory session identity.

Recovery puts the resolved text in the clipboard without restoring the previous
clipboard. The explicit second step is `Shift+Insert` in WezTerm or `Ctrl+V` in
ordinary applications, avoiding focus races and application-specific paste
shortcuts inside the global hook path.

NVIDIA Parakeet TDT 0.6B v3 is retained as an experimental backend candidate. It is
not the initial production dependency because its Windows NeMo deployment surface is
larger and less self-contained than whisper.cpp.

## Dictate-and-translate direction

Translation must run after ordinary recognition rather than enabling Whisper's
translation task on the current `large-v3-turbo` model. Turbo is not trained for
speech translation and can return the source language even when translation is
requested.

The planned pipeline is:

1. Whisper Turbo recognizes speech and reports the detected source language.
2. If the selected output is `original`, the transcript follows the existing
   insertion path unchanged.
3. If a target language is selected, a separate local NMT backend translates
   the accepted transcript.
4. The translated result becomes the last accepted transcript and follows the
   same guarded insertion transaction.
5. A missing model, unsupported pair, timeout, or low-confidence result fails
   open by retaining the original transcript instead of losing the dictation.

The first candidate pair is RU to EN with an attributed OPUS-MT model through a
native CTranslate2 worker. The protocol and settings should store a target
language code rather than a RU-to-EN boolean so additional downloadable language
pairs can be added without changing the dictation contract.
