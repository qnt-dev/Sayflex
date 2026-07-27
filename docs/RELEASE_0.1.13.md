# Keyboop for Windows 0.1.13

This release makes successfully recognized dictation recoverable even when the
original target loses focus or rejects insertion.

## Recovery

- Delay `Shift+Alt+X` recovery until Shift, Alt, and X have all been physically
  released, preventing retained modifiers from corrupting the recovery action.
- Put the most recent successful transcript in the clipboard and leave it there.
- Paste explicitly with `Shift+Insert` in WezTerm or `Ctrl+V` elsewhere. This
  avoids focus races, terminal-specific shortcuts, and accidental duplicate
  insertion.
- Restore the most recent transcript from durable history after SpeechWorker
  restarts.

## History

- Add opt-in local transcript history capped at 200 entries.
- Persist a successful transcript atomically before returning it for insertion.
- Store text, language, creation time, audio duration, and processing duration.
- Continue deleting temporary raw audio after recognition.
- Add a settings-window history surface with inspect, copy, delete, and clear
  actions.
- Keep history disabled by default for new users; this installation enables it
  from the user's explicit request.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification passed.
- Automated tests: 77 passed (`Core` 49, `InputReplay` 28), zero failed.
- Installer:
  `Keyboop-0.1.13-win-x64-setup.exe`, 193,277,950 bytes,
  SHA-256
  `F685691600AC2BC070081730FC4E1F8F909717959848AE8123C787D1647D9C6F`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.13`.
- SpeechWorker loaded durable history after a full restart and returned the
  newest stored entry as its last transcript. The original rescued recording
  and later successful dictations remained present; no transcript text was
  emitted into diagnostics.
- An installed hook-level `Shift+Alt+X` probe replaced a sentinel clipboard
  value with the exact newest stored transcript and did not type into the
  foreground editor. The clipboard can therefore be pasted with
  `Shift+Insert` in WezTerm.
- The installed settings window showed history enabled, four visible entries at
  inspection time, and working copy, delete, and clear controls.

Physical user acceptance in WezTerm remains the final interaction check.
