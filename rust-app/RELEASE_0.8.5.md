# SayFlex 0.8.5

## Screen translation correctness

- OCR block rectangles now remain crop-local throughout image layout. Captures
  made on a monitor with a non-zero desktop origin no longer collapse every
  translated block into the upper-left corner.
- Correction saving releases the canonical document borrow before refreshing
  the UI, eliminating the re-entrant `RefCell` panic in the real Save path.
- Overlay text has a 12-pixel minimum and measured wrapping. Ordinary callouts
  avoid source regions and each other; dense results that cannot be arranged
  safely use one measured, scrollable side panel instead of overlapping text.
- The compact header has a 440-pixel minimum, and the expired-image state keeps
  its explanatory text and Copy action reachable.

## Persistence, privacy, and glossary behavior

- Screenshot pixels are written once and metadata revisions are serialized.
  A stale initial write cannot overwrite a completed translation or a user
  correction.
- Settings can reopen the latest unexpired completed screen translation after
  restart. The restored result remains an ordinary window until PIN is chosen.
- Expiry, Delete image, and Delete all release every matching in-memory Slint
  image, including pinned windows, while retaining text and glossary entries.
- Corrections can store an editable source phrase with the complete OCR block
  as its example. Later sentences match approved terms at word boundaries and
  rank game, site, document, application, and general context separately.
- Concurrent glossary actions keep independent operation identities, so an
  earlier acknowledgement cannot be discarded by a later click.
- Glossary phrases match later sentences at real word boundaries, including
  text in CJK scripts that does not use spaces between terms.

## Translation safety and responsiveness

- Structured screen translation is divided into token-bounded groups under one
  operation deadline. Capacity probing is logarithmic, cancellation covers
  request and response-body delivery, and malformed-group recovery is bounded.
- Numbers, signed values, URLs, placeholders, and protected symbols are checked
  as exact multisets. Substring changes such as `25` to `125` and dropped
  repeated values are rejected.
- A saved block keeps its real completion outcome across restart. A limited,
  cancelled, or failed block cannot be restored as a copy-safe completed result.
- Browser and PDF titles contribute bounded document context when available;
  selected OCR text remains explicitly untrusted contextual data. Translation
  continues without unavailable URL or document metadata.

## Verification status

### Release identity

- Source commit embedded in the installer:
  `6fb02e176e6909932b9f5dfd9ed8dc01072f40c1`.
- Installer: `dist/SayFlex-0.8.5-win-x64-setup.exe`.
- Installer size: `568,318,868` bytes.
- Installer SHA-256:
  `DB5C64593FB22A0CA7165F1A9DD9CFAC554B68344B9FF03BB8A90C2AE064EA69`.
- Installed application directory:
  `%LOCALAPPDATA%\Programs\SayFlex`.
- Installed `SayFlex.exe` SHA-256:
  `9C460EC22DF75E7B1B62EF6977BE8185B556302E53DD7709598B70835C408768`.
- Installed manifest reports version `0.8.5`, protocol `31`, the source commit
  above, and 30 hashed payload entries. The previous build was replaced in
  place; no second SayFlex installation was retained.
- The installer is not Authenticode-signed. SignPath signing remains a separate
  public-release gate.

### Verified

- `cargo fmt --all -- --check` completed successfully.
- The desktop suite with screen translation enabled passed: 320 tests.
- The workspace suite excluding the feature-matrix speech worker passed with
  all features enabled.
- Speech-worker CPU, CUDA, and Vulkan configurations each passed their 44 unit
  tests and 4 integration tests.
- Strict Clippy completed successfully for the non-speech workspace and for
  the CPU, CUDA, and Vulkan speech-worker configurations.
- The localization validator passed with 558 messages in each of 8 locales.
- The release build produced and validated CPU, CUDA, Vulkan, OCR, model, and
  installer payloads. The staged and installed file sets match the build
  manifest.
- Silent in-place installation completed with exit code zero. Installed-file,
  build-manifest, Cargo.lock, backend-manifest, version, protocol, checksum,
  and uninstall-registration validation passed.
- The installed 0.8.5 process tree started from the installed directory. The
  desktop, input host, CUDA speech worker, and translation worker reported
  ready; CUDA detected the NVIDIA GeForce RTX 5070 Ti. Speech-model loading
  took 1,585 ms plus 135 ms warm-up, and translation-model loading took
  4,719 ms in this witnessed launch.
- DXGI screen capture and selection-overlay presentation were exercised in the
  installed build. Capture returned three outputs in 315 ms and the overlay was
  presented in 84 ms.
- Dense-result headless fixtures were rendered and inspected at 100%, 125%,
  and 200% scale.
- The compact translation card exposes the `Translating` status to accessibility
  only while processing is active; terminal and error states do not retain an
  invisible stale status element.

### Not verified as completed user scenarios

- The installed-artifact smoke receipt records 1 artifact-identity PASS,
  0 FAIL, and 39 NOT_COMPLETE checks. It is not evidence of 39 interactive
  passes.
- A complete physical screen-selection -> OCR -> translation -> correction ->
  pin/drag -> delete/expiry lifecycle has not been witnessed on the installed
  build. The automated drag was deliberately rejected by the product's
  injected-input protection and the selection watchdog cancelled it after
  15 seconds. That protection was not weakened for testing.
- Restart restoration could not be verified using the retained 0.8.4 record:
  that older metadata did not store source and target languages. Version 0.8.5
  refuses to guess them. The old record remains on disk and is not presented as
  a valid 0.8.5 restore test.
- Russian- or German-Windows GPU-counter localization has not been exercised on
  a corresponding operating-system installation.
- The aggregate VRAM/RAM budget and game-frame-time impact have not been
  measured with a game running.
- Browser URL extraction is not claimed. Translation proceeds with the
  application/title/document context that Windows actually exposes.
- One pre-existing Pause/Resume global-hotkey registration conflict was logged
  as Windows error 1409 during startup. It did not prevent the witnessed worker
  startup, but the conflicting owner was not identified in this release check.

The detailed artifact-only smoke receipt is
`dist/smoke-receipt-0.8.5-final.json`. Public release still requires the
separate Authenticode/SignPath step and user verification of the interactive
screen-translation lifecycle listed above.
