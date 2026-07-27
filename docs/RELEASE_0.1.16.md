# Keyboop for Windows 0.1.16

This release replaces the timing-based repeat-last repair with session-based
dictation ownership.

## Root cause

Earlier releases treated the newest transcript as a mutable global value.
Keyboard release, watchdog release, transcription completion, history persistence,
and `Shift+Alt+X` recovery were separate asynchronous operations. Waiting for
“currently known work” or forcing `End` ahead of `RecoverLast` narrowed the race
but could not prove that recovery belonged to the recording the user meant.

## Architecture change

- InputHost creates a unique immutable session ID at `Begin`.
- The hook and watchdog use one synchronized session owner and one action channel.
- `Begin`, `End`, transcript, history, insertion, and recovery carry the same ID.
- SpeechWorker permits one recording owner and gives each session one completion
  task.
- Repeated and concurrent `End` requests are idempotent and await that same task.
- Recovery addresses the exact session. It can arrive before `End` at the pipe
  server and own the same idempotent finalization, so a delayed standalone `End`
  cannot strand it.
- History uses the session ID as its durable identity and deduplicates repeated
  persistence. Existing history without session IDs remains readable.
- The IPC protocol version is raised to 2 so mixed old/new processes fail
  explicitly instead of silently misinterpreting payloads.

## Verification

- Debug and Release solution builds: zero warnings and zero errors.
- Formatting verification passed.
- Core tests: 49 passed.
- Input/protocol/session tests: 43 passed.
- Concurrent `End` ownership, reversed completion order, active-session recovery,
  history migration, and history idempotency are covered.
- Installer: `dist\Keyboop-0.1.16-win-x64-setup.exe`.
- Installer SHA-256:
  `7127032BA2B97E5D488AFA263A5FE8BFA898973F15B3FFEAC5B622CBB26810CA`.
- Silent per-user installation succeeded.
- All 307 installed application files matched the release manifest.
- UI, InputHost, and SpeechWorker started from the 0.1.16 install directory.
- Protocol-v2 begin/end smoke test passed and the listening indicator appeared.
- Physical `Ctrl+Win` dictation followed by `Shift+Alt+X` recovery remains an
  explicit user-acceptance check because a synthetic IPC probe does not exercise
  the keyboard hook or the user's microphone.
