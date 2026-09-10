# SayFlex Release Hardening Design

## Status

Approved and frozen by the product owner on 2026-09-10. This document turns the
approved release list into implementation contracts; it does not add product
scope.

## Goal

Ship one coherent SayFlex release that fixes settings interaction, makes global
shortcuts editable, localizes every user-facing surface in the agreed languages,
removes fragile provider state and cancellation behavior, preserves settings
through interrupted writes, and provides the compliance and release gates needed
for distribution.

## Global constraints

- Product name is always `SayFlex` in user-facing text.
- Windows x64 remains the only supported desktop platform for this release.
- Slint is a renderer and event source; durable and transient application state
  belongs to Rust owners with testable contracts.
- Network I/O never runs while holding a shared state mutex.
- Queues that can receive user work are bounded and declare an overload policy.
- Superseded speech and translation work cannot publish a result or delay a
  control acknowledgement.
- The existing clipboard-safe insertion contract must not change.
- Existing user settings and encrypted credential files must migrate without
  losing unknown provider rows.
- Chinese, Japanese, Hindi, and Arabic remain available as speech/translation
  languages but are not UI locales in this release.
- UI locales are System, English, Russian, Ukrainian, Spanish, Brazilian
  Portuguese, German, French, Italian, and Korean.
- No public-ready claim is allowed for an unsigned artifact.

## 1. Settings scrolling

Each page has exactly one vertical scroll owner. Slint's `Flickable`/`ListView`
handles wheel and touchpad input. SayFlex does not animate the bound viewport
position and does not intercept native wheel messages. The custom graphite rail
is a projection of the native viewport and remains draggable.

Acceptance:

- Every wheel notch contributes its full delta, including repeated notches in one
  event-loop batch.
- Models, Providers, Translation, History, and Hotkeys reach both ends with wheel,
  touchpad, and rail drag.
- Resizing does not create a second vertical owner or a horizontal bottom rail.

## 2. Editable global shortcuts

`HotkeySpec` is a protocol value containing canonical modifier bits and one
physical virtual-key code. `HotkeyBindings` contains dictation, insert-last,
translation, palette, and pause/resume bindings. Defaults preserve the current
gestures.

The input host owns registration. A `SetHotkeys` command validates the entire new
set, attempts all `RegisterHotKey` registrations, and commits only if every
binding succeeds. Failure restores the previous registrations and returns a
typed result naming the conflicting action. The low-level dictation hook receives
its binding through the same atomic configuration snapshot. Disabled features do
not register their command shortcuts. Pause/resume is a real host state that
suppresses dictation, translation, palette, and insert-last until resumed.

The UI records one chord at a time, rejects modifier-only or duplicate chords,
shows registration errors, supports reset-to-default, and persists only a fully
accepted set.

## 3. UI localization

Static Slint strings use `@tr`. The build bundles gettext catalogs. Rust-created
rows and diagnostics use a small typed `UiText` catalog keyed by stable semantic
identifiers; raw English prose is not used as a key. Selecting a locale updates
the Slint bundle and then rebuilds projected rows. `System` resolves the Windows
user locale to the closest supported language and falls back to English.

All visible surfaces are in scope: navigation, pages, controls, tray menu,
recording/translation overlays, provider/model/history rows, validation and
error messages, installer/uninstaller copy, and accessibility labels. Brand
names, dictated content, translated content, file paths, model identifiers, and
diagnostic logs are not translated.

## 4. Provider configuration and request ownership

Provider editor drafts live in Rust, keyed by `(provider_kind, provider_id)`.
Reordering rows or receiving status updates cannot replace a draft. A save uses
an operation id and closes the editor only after the matching worker
acknowledgement. A timeout reports an unknown outcome and refreshes real status;
it never claims success or clears typed text.

Azure Translator's region is optional. The UI explains that it is needed only
for regional or multi-service resources. Google Translation is removed from the
known/default/user-visible list while unknown stored rows remain round-trippable.
Commercial quota copy is neutral and does not promise hard-coded allowances.

Speech routing uses a bounded mailbox with a priority control lane. Cancellation
is tracked per session until terminal acknowledgement, never as one mutable
`Option<SessionId>`. Completed audio uses a bounded queue; if superseded before
network dispatch, it is discarded without consuming quota.

Translation uses one provider actor that exclusively owns provider state.
Requests use latest-request-wins and cancellation generations. The actor does not
spawn an unbounded OS thread per request. Network execution is outside shared
application locks; late results are discarded by request id and generation.
Credential and status commands remain responsive while a request is in flight.

## 5. Translation direction and placement

Automatic direction remains a convenience, not a guarantee. Settings expose the
primary/secondary policy, and the popup exposes the resolved source and target,
swap, and retry. An explicit retry sends explicit languages. All supported
language pairs are mapped independently for each provider.

The popup anchors to the UI Automation selection rectangle when present, then to
the caret rectangle, then to the pointer/work-area fallback. It is clamped to the
monitor work area and never defaults to the top edge merely because the selected
text is long.

## 6. Settings durability and secrets

Settings are serialized to a same-directory temporary file, flushed, and
atomically replace the primary using Windows replacement semantics. Startup
recovers the newest valid document from primary, previous, and temporary
candidates, preserving the damaged file for diagnosis. Tests inject failure at
each replacement boundary.

DPAPI credential files remain separate. Only `NotFound` means an empty store;
permission and I/O errors surface as recovery errors. UI and comments state the
truth: secrets pass transiently through the desktop callback and IPC but are not
stored in plaintext or in `settings.json`.

## 7. Visual previews, attribution, accessibility, and release gates

Every selectable visual has a transparent live preview driven by the same
animation data as the recording surface. If a renderer cannot be embedded, the
choice is explicitly disabled rather than represented by a static fake.
Broadcast Mic retains white, blue, and red palettes.

An About page is reachable from the top-level navigation and renders Slint's
`AboutSlint` attribution widget. The distribution includes a generated inventory
of dependency licenses plus project/model notices.

Custom controls expose role, label, enabled state, focus behavior, and default
action. Every settings operation is reachable by keyboard and visible focus does
not depend on color alone.

CI runs locked fmt, clippy, tests, cargo-deny, release packaging, manifest and
checksum validation. Tag builds support Authenticode signing through secrets and
must fail the public-release job if signing credentials are absent. The release
smoke script verifies the exact installed artifact and records editor, browser,
terminal, provider, microphone-switch, and upgrade results.

## Deferred deliberately

- Moving all desktop orchestration out of `main.rs`.
- Replacing bounded JSON history with SQLite.
- A full automatic updater; this release provides signed artifacts, checksums,
  and a signed-manifest-ready publication boundary.
- UI locales for Chinese, Japanese, Hindi, and Arabic.

## Completion evidence

Completion requires clean source status, locked workspace tests, clippy with
warnings denied, fmt, cargo-deny, installer construction, installed-file hash
comparison, signature verification for a public artifact, and the live smoke
matrix. A green unit suite alone is not release acceptance.
