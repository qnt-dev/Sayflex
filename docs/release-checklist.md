# SayFlex release checklist

Use this checklist only with the exact installer named in the smoke evidence.
Close every older SayFlex instance before installing or launching the candidate,
because both builds own the same global shortcuts and tray identity.

## Before packaging

- Run `pwsh -File rust-app/scripts/check-ui-translations.ps1`.
- Run `cargo test --workspace --no-fail-fast` from `rust-app`.
- Run `cargo clippy --workspace --all-targets -- -D warnings` from `rust-app`.
- Run `cargo fmt --all -- --check` from `rust-app`.
- Confirm the support destination is the approved HTTPS URL, or build without
  `SAYFLEX_SUPPORT_URL` so the support action stays hidden.
- For a public direct installer, supply the approved Authenticode certificate.

## Installed interaction matrix

Record every row separately in witnessed smoke evidence. Use `BLOCKED` or
`NOT_RUN` when it was not actually observed; never turn an unavailable test
into a pass.

| Check id | Witnessed result |
| --- | --- |
| `dictation_editor` | Dictation reaches an editor once. |
| `dictation_browser` | Dictation reaches a browser field once. |
| `dictation_terminal` | Dictation reaches the supported terminal path once. |
| `translation_popup` | Selected-text success or terminal error opens at the selection anchor. |
| `translation_orig` | ORIG displays the original text correctly. |
| `translation_dual` | DUAL displays the paired original and translation. |
| `translation_pin_resize` | A pinned popup remains usable after resize. |
| `translation_multi_pin` | Multiple pinned popups keep independent content and position. |
| `translation_click_away` | An unpinned popup dismisses on click-away. |
| `translation_quota_fallback` | A typed quota failure names the provider and a later provider still completes the translation. |
| `translation_bad_key` | A rejected credential produces the localized credential action. |
| `translation_no_providers` | An entirely disabled provider list produces the localized no-provider state. |
| `translation_timeout` | An unavailable or timed-out provider produces its localized retry state. |
| `translation_unsupported_pair` | An unsupported language pair produces its localized terminal state. |
| `translation_empty_source` | Empty input is rejected locally without a provider request. |
| `translation_ctrl_enter` | `Ctrl+Enter` starts exactly the same command as the Translate button. |
| `translation_copy` | The icon action copies the displayed result exactly once. |
| `translation_swap` | The icon action swaps direction and fields without stale state. |
| `microphone_switch` | Changing the input device keeps subsequent dictation functional. |
| `provider_fallback` | Provider order and fallback match the configured rows. |
| `settings_wheel_touchpad_rail` | Wheel, touchpad, and scroll rail remain responsive on every long settings page. |
| `recording_visuals` | Every shipped visualizer has a live preview and recording state. |
| `fresh_install` | First run waits for explicit Continue and respects the model-download choice. |
| `upgrade_install` | Existing settings, models, provider order, and shortcuts survive upgrade. |

The evidence may contain provider identity, typed failure reason, timing, and
the visible UI state. It must not contain API keys, selected source text,
dictation text, or translated output.

## Artifact closure

- Install the exact candidate and run `rust-app/scripts/smoke-installed.ps1`
  with its installer path and witnessed evidence JSON.
- Require a complete receipt before publishing.
- Verify staged, installed, and packaged hashes refer to the same commit.
- Run dictation and translation once more from the installed copy, not `target`.
