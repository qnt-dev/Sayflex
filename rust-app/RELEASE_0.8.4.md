# SayFlex 0.8.4

## Screen-translation interaction repair

- Dense OCR output is reconstructed into bounded semantic blocks instead of
  one full-image translation surface. List items remain distinct, ordinary
  wrapped paragraphs remain intact, and long dense columns are capped at six
  source lines per editable block.
- Double-clicking any translated block opens the correction editor inside the
  same image window. The field receives keyboard focus immediately; no
  parallel editor window can remain after the result is closed.
- The screen result is an ordinary movable window until PIN is selected. PIN
  changes only its z-order and no longer switches compositor presentation or
  flashes the captured image over the desktop.
- The title strip is draggable. Close and temporary-image deletion are now
  separate actions: `×` closes the complete result, while `DEL` removes only
  the retained crop and keeps its text translation available.
- Retiring or replacing a result closes only the editor owned by that result,
  and a snapshot refresh cannot resurrect a cancelled editor.

## Upgrade

Install 0.8.4 over the existing SayFlex installation. The in-place upgrade
preserves settings, history, downloaded model packs, runtimes, and the personal
glossary stored outside the program directory.

This candidate remains unsigned until the public signing workflow is complete.
Automated and installed-artifact checks establish technical consistency, not a
claim about every external game, monitor layout, or protected-content surface.

## Final artifact evidence

- Source commit: `51764cb662814e10883c67911617ff8997e55208`.
- Installer: `SayFlex-0.8.4-win-x64-setup.exe`, 568,267,136 bytes,
  SHA-256 `9C44B821D2090BF5FCC8E15D415B5EC60E14D80B909E5CA6873C5FA7D7B8636D`.
- Standard-path in-place installation completed without a restart. Installed
  files matched staging byte-for-byte, and release validation confirmed the
  file set, build manifest, Cargo.lock, binary build identity, versions,
  protocol 31, installer checksum, installed hashes, and uninstall
  registration.
- The installed desktop, InputHost, CUDA speech worker, and translation worker
  remained alive after startup. CUDA speech and the selected Qwen translation
  model reached ready state. A system-level conflict for `Ctrl+Alt+P` was
  reported as a typed per-action rejection without terminating InputHost.
- The generated installed-smoke receipt records `PASS=1`, `FAIL=0`, and 39
  honest `NOT_RUN` interaction/environment checks. Live capture, drag, inline
  edit, Close, Delete image, and Pin/unpin were not witnessed by the current
  automation surface and remain pending user verification.
