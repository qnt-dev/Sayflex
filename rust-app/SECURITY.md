# Security and privacy

Rust Voice is designed as a local desktop utility. Audio capture, speech
recognition, layout conversion, and installed translation-model inference run
on the user's machine.

## Data handling

- Live keyboard state is held only in the input-host process and is never
  persisted.
- Debug input traces are restricted to the configured command keys and gesture
  transitions. They do not record arbitrary keyboard scan codes or typed text.
- Temporary clipboard payloads used for paste opt out of Windows clipboard
  monitoring, clipboard history, and cloud clipboard. The previous `IDataObject`
  is restored only while the temporary clipboard sequence is still current, so
  a newer clipboard change made by the user is never overwritten.
- Explicit **Copy** commands intentionally create a normal clipboard item.
- Dictation and translation history can be disabled and cleared in the
  application. Clear removes the primary, recovery, temporary, and quarantined
  history files.
- Native Win32 password edit controls are rejected before recording or selected
  text capture. Applications with custom web or GPU-rendered password controls
  remain responsible for exposing the field as protected; users should disable
  dictation in unknown sensitive fields.

## Process and input isolation

- The desktop owns all workers through a Windows Job Object with
  `KILL_ON_JOB_CLOSE`. A desktop crash or forced termination cannot leave the
  global keyboard hook running.
- Worker processes communicate through versioned JSON-lines protocols. Their
  standard output is reserved for protocol messages; diagnostics use standard
  error.
- The low-level keyboard callback owns its physical-key state and performs no
  file, clipboard, UI, or blocking IPC work.
- Synthetic input is tagged with a private marker and cannot trigger a command
  gesture. The application releases only keys whose corresponding key-down it
  injected itself.
- Text insertion is rejected if the foreground target changed or physical
  command keys are still held. Long Unicode insertion revalidates the target
  between bounded chunks.
- Input into elevated applications is not supported from the normal
  non-elevated process because of Windows UIPI. Rust Voice does not attempt to
  bypass UAC.

## Model and update integrity

- Downloads use HTTPS, resume partial files, verify the exact byte length and
  full SHA-256 before atomic installation, and retain partial network downloads
  for retry.
- A verification receipt avoids re-hashing multi-gigabyte models on every
  launch. It is a corruption/incomplete-download check, not a boundary against
  malicious code already running as the same Windows user.
- The current unsigned per-user build likewise cannot defend its executables or
  models from an attacker that already has write access as that user. Release
  checksums and reproducible CI protect distribution integrity; Authenticode
  signing can be added later without changing the runtime architecture.
- CUDA discovery accepts only an installed NVIDIA toolkit directory and does
  not search arbitrary writable entries inherited through `PATH`.

## Reporting

Do not include dictated text, model files, history files, or clipboard contents
in a report. Include the application version, Windows version, worker name,
error code, and reproduction steps.
