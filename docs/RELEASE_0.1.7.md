# Keyboop for Windows 0.1.7

This release removes the grey translucent corner artifacts from the dictation
indicator and makes the `Ctrl+Win` chord independent of key order.

## Root cause

### Intermittent dictation start

The hook evaluated dictation only on a Windows-key-down event. It therefore
worked when Ctrl was already held but ignored the same chord when the Windows
key happened to go down first and Ctrl completed the chord.

### Indicator corners

The rounded indicator border filled the entire transparent WPF window while a
blurred drop shadow was rendered against the same rectangular bounds. The shadow
remained visible in the otherwise transparent corner pixels, especially over
white backgrounds.

## Changes

- Recognize `Ctrl+Win` when either Ctrl or Windows completes the chord.
- Apply the current low-level hook event to the physical-state snapshot so the
  decision does not depend on the asynchronous keyboard-state update order.
- Keep Windows key-down and key-up suppression balanced: when Windows was pressed
  first and reached the operating system, its matching key-up is also allowed
  through.
- Added regression coverage for left/right Ctrl and left/right Windows as the
  second key.
- Removed the clipped drop shadow from the dictation indicator.
- Enabled layout rounding and device-pixel snapping on the transparent window.
- Preserved the 112×42 size, 10 px corner radius, dark background, light frame,
  yellow waveform, status control, and listening/processing animation.

## Verification

- Release build: zero warnings and zero errors.
- Formatting verification passed.
- Automated tests: 71 passed (`Core` 49, `InputReplay` 22), zero failed.
- Installer:
  `Keyboop-0.1.7-win-x64-setup.exe`, 193,249,842 bytes,
  SHA-256
  `81CF4E91991F855AB962DB20A9F93C88EB8E459BF5017E19FCEFCAB846023101`.
- Silent per-user installation completed with exit code 0. All 307 installed
  product files matched the published files by SHA-256; installed product
  version is `0.1.7`.
- Installed hook-level probes passed for both `Ctrl` then `Win` and `Win` then
  `Ctrl`; in both cases the indicator appeared and returned to its baseline
  state after release.
- The direct installed-runtime indicator probe appeared in 115–116 ms. Visual
  inspection over a light web page showed clean transparent corners without the
  former grey shadow artifacts.
- Physical hotkey behavior still requires live user acceptance after
  installation.
