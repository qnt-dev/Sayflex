# Wispr Flow Windows reference and decommission audit

Audit completed: 2026-07-25  
Installed version inspected: 1.6.224

This is a clean-room behavioral, configuration, and public-documentation analysis
of the locally installed Windows application. It records reusable product
contracts and Windows design ideas, not proprietary source code or graphics. No
transcript contents, dictated text, credentials, tokens, or audio were collected
for this document, and no Wispr setting was changed.

## Conclusion

The installed application has now been inspected deeply enough for Keyboop
development. Keeping Wispr Flow installed is no longer required for architectural
or behavioral reference.

One decision remains before complete removal: whether the user wants to preserve
any personal Wispr data, especially Dictionary, Snippets, Scratchpad notes, or
transcription history. The audit found about 4.8 GB across the local installation
and roaming application-data directories, but deliberately did not inspect the
private records. Uninstalling the program and deleting all residual data are
separate actions.

## Evidence levels

- **Observed locally** means confirmed in the installed 1.6.224 binaries,
  configuration, process layout, or visible settings.
- **Documented** means described in current official Wispr documentation.
- **Inference** means an architectural conclusion from those observations; it is
  not a claim about Wispr's private implementation.

## Observed Windows shape

- The visible application is Electron 42.3.
- A separately signed `Wispr Flow Helper.exe` owns native Windows input behavior.
  The inspected main executable and helper had valid Authenticode signatures from
  Wispr AI, Inc.
- The helper is a .NET 10 NativeAOT Windows binary.
- The installation uses Squirrel and includes an `Update.exe` uninstaller, an
  autostart shortcut, and both the current and a previous application version.
- English and Russian were enabled in the visible language settings.
- The current shortcut editor supports multiple bindings per action and conflict
  warnings.

### Current default shortcuts observed in 1.6.224

| Action | Wispr Flow shortcut |
| --- | --- |
| Push to talk | `Ctrl+Win` |
| Hands-free | `Ctrl+Win+Space` |
| Command Mode | `Ctrl+Win+Alt` |
| Paste last transcript | `Alt+Shift+Z` |
| Press Enter | optional user binding |

`Shift+Alt+X` is the deliberate Keyboop binding for repeating the last successful
transcript. It is not the current Wispr default.

## Native hotkey lessons

Imported Windows APIs and observed behavior support this design:

- a low-level keyboard hook (`SetWindowsHookExW` / `CallNextHookEx`);
- physical-state checks (`GetAsyncKeyState`);
- synthetic input (`SendInput`);
- foreground/focus observation (`SetWinEventHook`);
- UI Automation and raw-input device enumeration;
- stale-key cleanup and explicit Windows-key release handling.

The reusable lesson is a behavioral contract, not a particular API. A
push-to-talk chord that suppresses the Windows key needs:

1. side-specific physical modifier state;
2. exact-chord matching rather than remembered aggregate modifiers;
3. a bounded session state machine;
4. handling for missing or reordered key-up events;
5. a watchdog that releases suppression and fails open.

This directly explains the class of bug fixed in Keyboop 0.1.6, where a stale
modifier made a standalone Windows key start dictation and an ordinary `X` repeat
the previous transcript.

## Recording and indicator behavior

Observed and documented product contracts worth retaining:

- The recording indicator appears immediately on hotkey activation and remains a
  single visual object through listening, processing, success, and failure.
- Push-to-talk and hands-free are separate modes. Hands-free can also be locked by
  double-pressing the push-to-talk shortcut.
- A visible stop/check action commits the recording; cancel discards it.
- A desktop session warns near 19 minutes and stops at 20 minutes.
- Observed audio constants use 16 kHz audio and 40 ms chunks.
- Music muting and dictation/notification sounds are independent preferences.
- New dictation is blocked while the previous recording is in a critical
  processing/retry state.

Keyboop should retain its local, persistent Whisper worker and waveform design. It
should adopt the state and recovery contracts, not Wispr's visual assets or
cloud-only transcription architecture.

Official reference:
[hands-free behavior and limits](https://docs.wisprflow.ai/articles/6391241694-use-flow-hands-free).

## Text insertion behavior

The installed helper contains compatibility paths for clipboard insertion,
delayed clipboard rendering, format preservation/restoration, `Ctrl+V`,
`Shift+Insert`, focus validation, clipboard-sequence checks, and elevated/UIPI
target detection.

The documented user-facing contract is:

- insertion temporarily uses the clipboard and restores the old clipboard;
- if direct insertion fails, the recognized text remains recoverable for manual
  paste;
- terminal compatibility is target-specific;
- non-QWERTY layouts and elevated applications are known edge cases.

Keyboop's guarded clipboard transaction remains the right default:

1. remember the foreground target;
2. materialize clipboard formats on a dedicated STA thread;
3. put final text on the clipboard;
4. revalidate target and focus;
5. send the target-specific paste chord;
6. restore the old clipboard only if its sequence still belongs to the operation;
7. fall back to Unicode input or leave the text available for explicit paste.

Keyboop should **not** copy Wispr's published suggestion to run the whole
application as administrator. Elevated targets should remain unsupported by
default, with a separately designed and signed helper only if a future requirement
justifies it.

Official references:
[clipboard restoration](https://docs.wisprflow.ai/articles/7971211038-fix-text-not-pasting-after-dictation),
[terminal behavior](https://docs.wisprflow.ai/articles/6478598909-using-flow-with-linux-wsl-and-terminal-applications),
[non-QWERTY limitations](https://docs.wisprflow.ai/articles/1621472516-flow-fails-to-detect-text-fields-or-inserts-incorrectly-on-non-qwerty-keyboard-layouts).

## Failure and retry contracts

These are more valuable than visual parity:

- A failed recognition retains bounded local recovery data and exposes Retry.
- Automatic retry must not create a second concurrent dictation.
- A crash during active recording may be unrecoverable and must be reported
  honestly.
- “Taking longer than usual” is a visible state, not a silent frozen indicator.
- Microphone, model/runtime, storage, insertion, and target-focus failures are
  distinct error classes.
- Paste failure keeps the final text recoverable through clipboard/manual paste.
- A transformation timeout or failure inserts the original untransformed
  dictation.
- Retry reprocesses the original input, not a partially transformed result.

Wispr documents retaining failed desktop audio for up to 14 days. Keyboop should
instead use a much shorter bounded default or an explicit opt-in because its ASR is
local and normally does not need cloud retry.

Official references:
[retry behavior](https://docs.wisprflow.ai/articles/2503460374-retry-failed-transcriptions),
[processing and storage failures](https://docs.wisprflow.ai/articles/4984532368-fix-taking-longer-than-usual-and-transcription-errors),
[microphone troubleshooting](https://docs.wisprflow.ai/articles/4351452717-troubleshooting-mic-issues).

## Useful product features

### High-value additions

1. **Configurable global shortcuts** with several bindings per action, conflict
   detection, and reset-to-default.
2. **Hands-free dictation** with double-press lock, explicit commit, and cancel.
3. **Per-application insertion profiles** for clipboard paste, `Shift+Insert`,
   Unicode input, and unsupported targets.
4. **Recoverable failed dictation** with bounded temporary audio, Retry, Copy, and
   Paste actions.
5. **Voice “press enter”**, enabled explicitly and recognized only at the end of an
   utterance.
6. **Dictionary** with two different concepts: vocabulary boosting for names and
   deterministic spoken-form replacement.
7. **Snippets** with whole-word voice triggers, static expansion, validation, and
   import/export.
8. **Selected-text commands** such as polish, translate, summarize, and answer,
   with cancel, original-text fallback, diff, accept, retry, and undo.
9. **Per-application style/format profiles**, especially message, email, document,
   code editor, and terminal.

### Useful later, with privacy boundaries

- Nearby-text context for capitalization, spacing, names, and app-specific style.
  This must be local-only, explicitly enabled, bounded around the caret, disabled
  for passwords/terminals/sensitive apps, and never written to diagnostics.
- Code-editor variable and filename recognition. This should be opt-in and limited
  to known editors, never a universal screen/context reader.
- Bulk Dictionary/Snippet import with preview, duplicate handling, size limits,
  validation, and an all-or-nothing commit.
- A local scratchpad can be useful, but it is a separate notes product rather than
  a prerequisite for excellent dictation.

### Deliberately out of scope

- cloud-required ASR, cloud history, team administration, referrals, streaks, and
  creator branding;
- telemetry, screenshot collection, or context collection by default;
- indefinite audio/transcript retention;
- an Electron UI shell;
- copied Wispr graphics, bundled code, prompts, or private implementation details;
- automatic mutation of selected or dictated text without an original-text
  fallback and explicit undo contract.

Official references:
[shortcut capabilities](https://docs.wisprflow.ai/articles/2612050838-supported-unsupported-keyboard-hotkey-shortcuts),
[Dictionary](https://docs.wisprflow.ai/articles/4052411709-teach-flow-your-words-with-the-dictionary),
[Snippets](https://docs.wisprflow.ai/articles/5784437944-create-and-use-snippets),
[Transforms](https://docs.wisprflow.ai/articles/8068950331-how-to-use-transforms-beta),
[Smart Formatting and Backtrack](https://docs.wisprflow.ai/articles/5373093536-how-do-i-use-smart-formatting-and-backtrack),
[Styles](https://docs.wisprflow.ai/articles/2368263928-how-to-setup-flow-styles),
[Scratchpad](https://docs.wisprflow.ai/articles/9618237082-using-the-scratchpad-to-save-and-edit-notes).

## Privacy lessons

Wispr separates model-training consent, cloud synchronization, and local retention.
Keyboop should expose similarly separate controls rather than one ambiguous
“privacy” switch:

- local ASR and local transformation are the default;
- transcript history is off by default;
- failed-audio retention has an explicit duration and size cap;
- Dictionary/Snippets and Scratchpad have separate persistence choices;
- diagnostics contain states, timings, app identity, and error codes, never typed
  or dictated content;
- any future network provider is opt-in per feature.

Official references:
[privacy policy](https://wisprflow.ai/privacy) and
[Privacy Mode, Cloud Sync, and local storage](https://docs.wisprflow.ai/articles/4709791908-understanding-privacy-mode-and-cloud-sync).

## Prioritized Keyboop backlog

### P0 — reliability before claiming replacement readiness

- Complete physical user acceptance of the current installed release's hotkeys
  and insertion.
- Make hotkeys configurable and detect conflicts.
- Add target compatibility profiles and an insertion acknowledgement/recovery UI.
- Detect password, secure, elevated, and unsupported targets.
- Add a bounded processing/retry state machine with explicit local retention.
- Distinguish microphone, local-model, storage, focus, and insertion failures.
- Add startup/crash recovery and stale worker detection.

### P1 — functional expansion

- Hands-free mode and double-press lock.
- Optional end-of-utterance “press enter”.
- Dictionary learning and deterministic replacement rules.
- Safe snippets and validated bulk import/export.
- Selected-text translate/polish/transform with diff and undo.
- Per-application cleanup/style profiles.
- Opt-in code-editor variable and filename awareness.

### P2 — optional product surfaces

- Local scratchpad and bounded history.
- More local translation/LLM backends.
- Sync only if a future user requirement justifies the privacy and operational
  cost.

## Safe removal checklist

The reference audit is complete, but removal must be a separate explicit action.

Before uninstalling:

1. Decide whether to preserve or discard Dictionary, Snippets, Scratchpad notes,
   and transcription history. Do not copy private data merely “just in case”.
2. If preservation is requested, export supported objects or make a clearly named,
   user-approved private backup before deleting anything.
3. Exit Wispr Flow so it no longer competes with Keyboop for `Ctrl+Win`.
4. Run the registered per-user uninstaller.
5. Verify and, only with explicit approval, remove residual install/data
   directories and the Startup shortcut.
6. Verify no Wispr process, startup item, or updater remains.
7. Launch Keyboop and run the physical hotkey/insertion acceptance pass.

The official complete-removal guide notes that normal uninstall may leave local
data and startup artifacts:
[completely removing Wispr Flow](https://docs.wisprflow.ai/articles/3884018196-completely-removing-wispr-flow-from-your-device).
