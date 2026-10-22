# SayFlex

SayFlex is a local-first Windows utility for voice dictation, selected-text
translation, and RU/EN keyboard-layout correction. The current Rust/Slint
release candidate is version **0.8.12**.

Audio, local speech recognition, layout correction, and installed translation
models run on the user's computer by default. Optional cloud providers remain
off until the user supplies credentials and enables them.

## Features

- Hold `Ctrl + Alt + Space` to dictate into the currently focused application.
- Press `Ctrl + Alt + V` to insert the newest successful dictation.
- Press `Ctrl + Alt + F` to translate selected text.
- Press `Ctrl + Alt + G` to freeze the desktop, select a region, recognize its
  text locally, and show the translation either as text or on the captured
  image.
- Press `Ctrl + Alt + K` to correct the keyboard layout of selected text.
- Use the standalone Quick translation window for pasted text.
- Choose verified local Whisper and Qwen model packs or configure optional
  cloud speech and translation providers.
- Remove installed local model packs from Settings to recover disk space.
- Use CUDA, Vulkan, or CPU speech backends, with automatic verified fallback.
- Keep dictation and translation history locally, or disable and clear it.
- Correct or explicitly confirm screen translations to build a personal,
  context-aware glossary. Ordinary translation and Copy never add entries.
- Inspect aggregate and per-feature CPU, RAM, GPU and VRAM use on Resources;
  resident engines can be released and load again automatically on next use.

Global shortcuts are configurable in Settings. A conflicting replacement does
not discard the previous working shortcut.

## Privacy and downloads

The installer does not bundle multi-gigabyte model files. The first-run screen
asks before downloading the recommended local Whisper model, and translation
models are downloaded only after an explicit choice in Settings. Every model
and local runtime download is pinned and verified by byte length and SHA-256.

Cloud speech and translation are separate provider lists. They are disabled by
default, and the application explains when audio or text may leave the device.
Provider keys are protected with Windows DPAPI for the current user.

See [Security and privacy](rust-app/SECURITY.md) for the complete data-handling
and trust-boundary description.

## Release status

Version 0.8.12 is undergoing release hardening. A public installer is not ready
until the exact installed artifact passes the Windows interaction matrix and
carries a valid Authenticode signature. Do not treat files from a local `dist`
directory as an official release.

The current release notes are in
[rust-app/RELEASE_0.8.12.md](rust-app/RELEASE_0.8.12.md).

## Build and test

The maintained application lives in `rust-app`. The older C# implementation in
the repository root is retained for history and is not a build dependency of
SayFlex 0.8.12.

```powershell
cd rust-app
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
./scripts/check-ui-translations.ps1
./scripts/build-installer.ps1
```

Building the universal installer also requires the Windows C++ toolchain, LLVM,
the pinned CUDA/Vulkan build prerequisites, and Inno Setup. The release scripts
refuse dirty source trees, record the exact Git commit and dependency lock hash,
and validate the staged payload before packaging.

Architecture, Windows integration fixtures, and release commands are documented
in [rust-app/README.md](rust-app/README.md). The Store hand-off is documented in
[rust-app/docs/MICROSOFT_STORE_SUBMISSION.md](rust-app/docs/MICROSOFT_STORE_SUBMISSION.md).

## License

SayFlex-owned source files are available under the [MIT License](LICENSE).
Third-party components and optional downloaded models keep their respective
licenses; see [third-party notices](rust-app/THIRD_PARTY_NOTICES.md) and
[model licenses](rust-app/docs/THIRD_PARTY_MODELS.md).

The current release configuration consumes Slint under its royalty-free desktop
application license. That distribution choice may change before public release
to meet the requirements of the selected open-source signing program.
