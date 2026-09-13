# SayFlex Release UX Follow-up Design

## Scope

This change set follows the 0.8.0 model-lifecycle release candidate. It fixes
translation feedback, removes duplicated settings chrome, makes first-run model
downloads consensual, and adds a restrained support entry. New recording
visualizers and the goals subsystem have separate visual approval gates and are
not part of this implementation boundary.

## Translation feedback contract

The provider router already knows why each provider was skipped or failed, but
the worker currently reduces that information to one English error string. The
wire protocol will carry typed request outcomes instead:

- provider id;
- failure kind: credential, configuration, quota, rate limit, unavailable,
  unsupported request, or empty response;
- optional retry delay;
- redacted diagnostic detail.

A successful result also carries the failed attempts that preceded it. The
standalone translator and selection popup can therefore say, for example, that
DeepL exhausted its quota and SayFlex completed through the local model. Raw
provider bodies and secrets are never used as primary UI copy.

Every selection-translation request produces visible terminal feedback. A
successful request shows the translation; a failed request shows a designed
error state in the same popup near the selection. Missing source text and an
entirely disabled provider list are validated before network work and use their
own localized messages.

## Standalone translator interaction

The source and result areas use one light graphite surface in all focus states.
Focus changes the source border to the SayFlex signal color without changing
the field fill. While a request is processing the result border indicates
activity; when a result arrives that result border receives the signal accent.

`Ctrl+Enter` and the Translate button perform the same action. Swap and copy are
compact icon actions with tooltips and accessibility labels; no unlabeled glyph
is acceptable. Processing, fallback success, and terminal error are separate
states rather than one free-form status string.

## Settings shell

Quick translation is the persistent top-right action. The footer does not
terminate the process. The bottom-right page number is removed because the
page header already owns the `current / total` position. Closing the settings
window hides it; process exit remains available from the tray menu.

## First-run local model consent

A fresh profile must not start a multi-gigabyte download merely because the
speech worker starts. Before worker initialization, the app shows one first-run
choice with `Download the recommended local model` checked by default. The
screen states the exact catalog model name and displayed download size. Only an
explicit Continue persists the choice and initializes the worker.

If unchecked, local speech starts disabled and no local model download command
is sent. Cloud providers can be configured immediately. Existing profiles are
not shown the first-run prompt and retain their provider order. This in-app gate
is used for both direct installer and Store installs; packaging UI is not the
authority for runtime consent.

## Support action

The settings footer may show one secondary `Support the developer` action with
the approved coffee icon on its right. Its destination is build configuration,
not a hard-coded guessed account. When no HTTPS support URL is supplied, the
action is omitted. Opening the URL uses the Windows shell and failure is
reported in the existing status area.

## Localization and layout

Interface locales are System, English, Russian, Ukrainian, Spanish, Brazilian
Portuguese, German, French, Italian, and Korean. Chinese, Japanese, Hindi, and
Arabic are not interface locales in this release. Translation/dictation content
language support is independent of interface localization.

All new copy is reviewed as UI copy in all nine catalogs. Minimum-size layout
tests cover translator idle, focused, processing, fallback, result, and error;
first-run checked and unchecked; settings with and without a support URL; and
the settings footer in every locale.

## Acceptance boundary

Automated tests and preview receipts establish implementation correctness. The
translator, first-run page, and settings support placement require user visual
approval before packaging. An installed candidate must then pass translation
success, fallback, quota, missing-key, no-provider, timeout, and unsupported
pair smokes.
