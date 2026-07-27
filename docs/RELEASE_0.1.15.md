# Keyboop for Windows 0.1.15

> Superseded before installation. Testing showed that ordering `End` ahead of
> `RecoverLast` still relied on a mutable global transcript. Version 0.1.16
> replaces this patch with session-addressed finalization and recovery.

This release fixes the remaining race that could still make `Shift+Alt+X`
recover the penultimate dictation.

## Root cause

Version 0.1.14 correctly waited for finalizations already registered in the
speech action queue. However, dictation end can be detected by either the
keyboard-hook thread or the physical-key watchdog. In one narrow interleaving,
the recovery thread queued `RecoverLast` just before the watchdog queued
`EndDictation`. Recovery therefore saw no pending finalization and copied the
previous transcript.

## Changes

- Put speech `Begin`, `End`, and `RecoverLast` actions behind one synchronized
  session gate.
- Enqueue `EndDictation` before publishing a dictation session as inactive.
- Make recovery atomically end an active session before it queues itself.
- Retain the pending-finalization wait added in 0.1.14.
- Add concurrent ordering tests that race end and recovery 100 times and require
  `Begin → End → Recover` in every iteration.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification passed.
- Automated tests: 83 passed (`Core` 49, `InputReplay` 34), zero failed.
- Package, installation, file-integrity, and physical user checks remain to be
  recorded.
