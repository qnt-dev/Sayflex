# SayFlex Screen Translation Design

## Scope

This change adds screen-region translation to the current SayFlex release
candidate before signing. A configurable global action captures a frozen image
of the visible desktop, lets the user select a region, recognizes text locally,
and routes that text through the existing translation worker and compact result
window.

The feature does not use the clipboard, persist screenshots, or add a cloud OCR
provider. It does not attempt in-place image rewriting, whole-window capture,
clipboard-image translation, UI Automation rectangle traversal, handwriting
recognition, or table reconstruction. Those are separate future features.

## Interaction contract

The initial configurable binding is `Ctrl+Alt+G`. It participates in the same
atomic hotkey validation and conflict handling as the existing actions, so a
rejected replacement cannot discard a previously working binding.

When the binding is pressed:

1. SayFlex captures a frozen frame for every active monitor before showing any
   application-owned overlay.
2. One non-activating selection surface is placed on each monitor. It displays
   the frozen frame under a dim layer and keeps the selected rectangle clear
   with the SayFlex signal border.
3. Left-button drag creates a selection in physical desktop pixels. A drag may
   cross monitor boundaries. `Esc` or right-click cancels without opening a
   result. A zero-area or accidental click also cancels.
4. Releasing the left button closes every selection surface immediately. A
   compact processing card is anchored to the selected desktop rectangle while
   recognition and translation continue off the UI thread.
5. Successful recognition is translated through the existing ordered provider
   route. The result card can show the recognized source (`ORIG`), the paired
   source and translation (`DUAL`), or the translation alone.

The compact card's permanent toolbar contains only `ORIG`, `DUAL`, and `PIN`.
It does not contain a language label, language-swap action, persistent retry
action, or replace-selection action. Screen pixels do not identify an editable
source target, so replacement is never offered. Terminal errors are designed
content states in the same card, not extra permanent toolbar controls.

The selection surfaces never activate or steal focus from the application that
was visible when capture began. The input host temporarily owns cancellation:
it consumes `Esc` only while a screen-selection request is active and reports a
typed cancellation event to the desktop. Right-click cancellation is handled by
the selection surface. The temporary mode is cleared on completion, explicit
cancellation, overlay failure, desktop disconnect, or watchdog timeout.

## Capture architecture

Production capture uses DXGI Desktop Duplication per active output. The desktop
coordinator requests one immutable capture generation, and the platform layer
normalizes every output to top-down RGBA plus its physical virtual-desktop
rectangle and rotation. The compositor crops the user's physical selection
from those immutable frames. Mixed DPI does not rescale source pixels; Slint
logical coordinates are converted at each monitor boundary before they enter
the capture contract.

The existing GDI `BitBlt` backdrop capture remains a bounded fallback for an
ordinary composited desktop when DXGI is temporarily unavailable. It is not
treated as equivalent coverage for exclusive fullscreen or rotated displays.
Protected video, `SetWindowDisplayAffinity` content, the UAC secure desktop,
and other Windows-protected surfaces are allowed to return black or unavailable
pixels. SayFlex reports a localized capture-unavailable state and never tries
to bypass platform protection.

The frozen frames and crop are bounded by the current virtual desktop size and
live only for the request. They are released after OCR accepts the crop or the
request terminates. No pixel buffer enters history, diagnostics, crash metadata,
temporary files, or the clipboard.

## OCR isolation and engine gate

OCR runs in a new supervised process, `SayFlex.Ocr.exe`, with a versioned JSON
protocol and the same Job Object lifetime boundary as the existing workers. A
request carries an opaque request ID, image dimensions, pixel format, candidate
source-language policy, and image bytes over the private worker channel. A
result carries ordered lines, word or line rectangles, confidence, recognized
language or script when available, and engine timing. It never echoes image
bytes in diagnostics.

The desktop and translation worker depend on an engine-neutral OCR result, not
on a particular vendor API. Before the production engine is locked, the
implementation must compare these local candidates on one checked-in fixture
corpus that includes light and dark UI, small text, mixed Latin/Cyrillic text,
multiple columns, scaled displays, and the currently supported SayFlex source
languages:

- PP-OCR mobile models through ONNX Runtime on CPU;
- Tesseract 5 with the matching fast language data;
- Windows Media OCR as a packaged-app benchmark where package identity and the
  requested recognizer language are available.

The selected portable engine must work in both the direct installer and Store
MSIX, use redistributable components compatible with the project's final OSS
license, beat Tesseract on aggregate character accuracy for the fixture corpus,
and complete a normal screen-text crop within 500 ms at p95 on the release test
machine. If no portable engine meets those gates, the feature remains excluded
from public packaging rather than silently becoming MSIX-only or cloud-only.

Windows Media OCR is not the sole production implementation because Microsoft
currently restricts that desktop API to applications with package identity.
CUDA is not part of the first OCR path: small region inference starts on CPU to
avoid another proprietary runtime boundary. Any downloadable OCR model follows
the existing model-manager contract: pinned origin, license, byte length,
SHA-256, resumable verification, atomic installation, explicit user consent,
visible disk size, and complete removal.

## Text reconstruction and translation

OCR preprocessing is engine-specific. The shared coordinator may crop, clamp,
and convert pixel format, but it does not always upscale, binarize, or invert an
image. Those transformations are selected and tested with the engine so a
heuristic useful to Tesseract cannot damage a neural recognizer.

The OCR worker reconstructs reading order from geometry before translation. It
groups compatible neighboring lines, preserves paragraph boundaries, handles a
trailing hyphen only when the next line is a lexical continuation, and keeps
columns separate. Low-confidence noise is excluded. An empty usable result
produces the localized `No text found in the selected area` outcome.

The reconstructed text enters the existing translation route once, with a new
screen-region origin and the selected rectangle as its presentation anchor.
Existing language policy, provider ordering, fallback attempts, cancellation,
streaming, structured errors, history preference, and local-engine residency
remain authoritative. The OCR layer does not run a second independent language
router. If the configured translation route uses a cloud provider, only the
recognized text may leave the machine under the existing provider disclosure;
the screenshot never does.

Superseding a request cancels OCR and translation by the same request identity.
Late OCR or translation output from an older generation is discarded. Closing
an unpinned result cancels outstanding work; pinning preserves the completed
content but does not retain the source bitmap.

## Failures and diagnostics

The result card distinguishes capture unavailable, protected content, empty
selection, no text found, OCR model unavailable, OCR language unavailable,
OCR worker failure, translation provider failure, and cancellation. Provider
quota, credential, timeout, unsupported-pair, and fallback-success messages
retain their existing typed localized behavior.

Diagnostics may record request ID, monitor count, crop dimensions, capture/OCR
and translation timings, engine identity, confidence summary, and typed failure
kind. They never record pixels, recognized text, translations, API keys, window
titles, or foreground document identity.

## Verification and visual approval

Automated verification covers:

- protocol completeness and stale-generation rejection;
- negative virtual-desktop origins, mixed DPI, rotation, and cross-monitor crop
  geometry;
- capture-before-dim ordering and overlay teardown on every terminal path;
- cancellation without stealing focus or leaving `Esc` suppressed;
- deterministic paragraph reconstruction and confidence filtering;
- light, dark, small-text, Latin/Cyrillic, column, blank, and protected-content
  fixtures;
- worker crash/restart and bounded memory after repeated capture cycles;
- exact reuse of translation provider and structured-error behavior;
- absence of screenshots and OCR text from clipboard, disk, history when
  disabled, and diagnostics;
- compact-card controls limited to `ORIG`, `DUAL`, and `PIN` in every locale.

Installed Windows checks cover mixed-scale multiple monitors, a browser, an
Electron application, a PDF, an ordinary game window, protected content, and
both direct-installer and MSIX identities. The dim layer, selection border,
processing state, and result placement require user visual approval before the
feature is admitted to the signed release candidate.
