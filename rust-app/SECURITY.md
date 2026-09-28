# Security and privacy

SayFlex is designed as a local desktop utility. Audio capture, speech
recognition, layout conversion, and installed translation-model inference run
on the user's machine, and that is what a fresh installation does.

## Two kinds of content can be sent to a network service, and neither is by default

Settings holds two separate ordered lists of providers, and each is its own
decision:

- **Speech.** A recording can be transcribed by Groq, Cloudflare Workers AI,
  Azure Speech, Deepgram or a custom OpenAI-compatible endpoint instead of by
  the model on this machine.
- **Text.** A selection or locally recognized screen-text block can be
  translated by DeepL or Azure Translator instead of by the engine on this
  machine. Screen pixels are not sent to these providers.

They are deliberately not one list and not one switch. A key for Azure
Translator is not a key for Azure Speech, the free allowances are counted
separately, and somebody willing to send a few seconds of audio to a service is
not thereby willing to send the contents of their documents.

Every cloud row in both lists starts switched off and stays off until the user
supplies a key. The local engine starts enabled as a row in each list, and the
walk stops the moment it reaches an enabled one — so anything placed below it
is never asked.
What the window claims about audio or text leaving the machine is computed from
these lists themselves, not from a separate setting that could disagree with
them.

Keys are encrypted with Windows DPAPI in the current user's scope, kept in their
own file rather than in `settings.json`, and never shown again after they are
saved: a row displays only the last four characters. A custom speech endpoint
must be HTTPS, because the key and the audio travel in the same request. Any key
a service echoes back in an error is removed before that error reaches the log.

## Data handling

- Live keyboard state is held only in the input-host process and is never
  persisted.
- Debug input traces are restricted to the configured command keys and gesture
  transitions. They do not record arbitrary keyboard scan codes or typed text.
- Temporary clipboard payloads used for paste opt out of Windows clipboard
  monitoring, clipboard history, and cloud clipboard. The original OLE data
  object is restored only while the temporary clipboard sequence is still
  current, so a newer clipboard change made by the user is never overwritten.
  A separately captured Unicode-text fallback prevents an unavailable
  short-lived clipboard owner from leaving the clipboard empty.
- Selected-text capture first uses UI Automation and therefore does not touch
  the clipboard when the target exposes `TextPattern`. A Copy fallback briefly
  lets the target application own the clipboard; the input host immediately
  re-owns the copied text with history/cloud exclusion formats, waits for
  delayed duplicate Copy messages, and restores the original formats. Windows
  exposes no API that can retroactively guarantee an external clipboard monitor
  did not observe that first target-owned sequence.
- Explicit **Copy** commands intentionally create a normal clipboard item.
- Dictation and translation history can be disabled and cleared in the
  application. Clear removes the primary, recovery, temporary, and quarantined
  history files.
- Screen OCR runs locally on CPU. Full-monitor frames exist only in memory for
  the active selection and are never written to disk, copied to the clipboard,
  included in diagnostics, crash metadata, or sent to a cloud translation
  provider. Only the selected crop can be persisted.
- Temporary selected crops and their result metadata are kept by default for
  up to 24 hours and 500 MB under the current user's SayFlex data directory.
  Oldest-first size eviction can delete them sooner; pinning does not extend
  their lifetime. Users can disable future retention, delete temporary crops
  immediately, or explicitly save a permanent image file. Expiry removes the
  pixels while leaving the text result and personal glossary independent.
- Optional source context can include the source executable, visible window
  title, application/game identity, document name, site URL when Windows or
  the application exposes it, and nearby text. Ambiguous or unavailable
  context is omitted. Temporary result metadata follows screenshot retention;
  a context field saved with a user-approved glossary entry remains until that
  entry is edited or deleted.
- The personal glossary changes only when the user saves a correction or
  explicitly confirms a translation. Machine completion, Copy, Pin, Close,
  screenshot deletion, and history clearing do not add or remove glossary
  records. Entries are stored separately and can be viewed, edited, or
  deleted.
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
- OCR text and source context are treated as untrusted structured data, not as
  model instructions. Structured translation validates the exact block ID set
  and protected values before accepting an answer. Compact and ordinary cloud
  adapters do not receive glossary or instruction context they cannot enforce.
- The low-level keyboard callback owns its physical-key state and performs no
  file, clipboard, UI, or blocking IPC work.
- Synthetic input is tagged with a private marker and cannot trigger a command
  gesture. The application releases only keys whose corresponding key-down it
  injected itself.
- Text insertion is rejected if the foreground target changed or physical
  command keys are still held. Long Unicode insertion revalidates the target
  between bounded chunks.
- Input into elevated applications is not supported from the normal
  non-elevated process because of Windows UIPI. SayFlex does not attempt to
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
