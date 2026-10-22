# SayFlex audit reliability implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development, with TDD and independent review for each owned subsystem.

**Goal:** Correct the reliability findings approved on 2026-10-09, without changing the visual design.

**Architecture:** Keep the existing workers and protocols. Tighten ownership of clipboard transactions, request lifetimes, retained data, and worker generations. Preserve the existing screen translation word fallback and all previously accepted results.

**Tech Stack:** Rust, Slint, Windows APIs, PowerShell, existing Cargo tests and installed smoke fixtures.

**Spec:** The audit and the user's explicit approval in this chat are authoritative. The user excluded design changes; only settings scrolling is authorized in that UI category.

## Constraints
- Reuse the existing release-hardening worktree and preserve all prior dirty work.
- No changes to window dimensions, layout, fonts, colors, screen editor geometry, or visual assets.
- Do not close a running installation until a verified replacement is staged. Preserve settings, models, history, credentials, and rollback.
- Do not commit unrelated baseline changes, push, or publish an official release.
- Use actual regression tests, including negative paths and competing operations. Do not reduce safety waiting limits without evidence.
- Shared files are owned by the coordinator: desktop main.rs, UI status translations, protocol, workspace metadata, release scripts.

## Tasks
- [x] 1. Input host: sequence-checked selection capture; terminal password/protected denial before delivery; truthful insertion acknowledgement. Files: input-host clipboard.rs, insertion_adapter.rs, inject.rs, target.rs and focused fixtures. Tests: foreign Copy during capture; failed publication must not claim ownership; Protected must not invoke paste; unchanged/native successful routes.
- [x] 2. Translation: Unicode numeric signs; cancellation and shared deadline during initial model startup; incremental validated cloud blocks retaining earlier success; terminal model cleanup for failures/cancellation. Files: translation-engine engine.rs/protected_values.rs and translation-worker main.rs/provider_actor.rs/block_translation.rs. Tests: sign loss; cold start cancellation/deadline; later cloud failure/local fallback; keep-warm matrix.
- [x] 3. Speech lifecycle: release the specific cloud-finalized session; drain and join history before normal worker exit. Files: speech-worker inference.rs, transcription.rs, history_actor.rs, main.rs and tests. Tests: success/failure/cancel and newer-session protection; shutdown after queued append.
- [x] 4. Desktop data: clear completed SessionRegistry entries and pending insertion on successful history clear; prevent pre-clear asynchronous screenshot writes from recreating removed data. Files: app-core session_registry.rs, desktop main.rs/screenshot_store.rs. Tests: insert-last after clear; pending completion; save-before-delete generation race; post-delete new capture permitted.
- [x] 5. Desktop events and settings: route scrolling to the region under pointer; preserve old-instance event protection for generic workers; allow bounded graceful shutdown with history acknowledgement. Files: desktop settings_view.rs, worker_supervisor.rs, main.rs. Tests: wheel over sidebar/right/child editor; stale Ready/history event; drained/ignored shutdown worker.
- [x] 6. Integration: component reviews, 1043 workspace tests, strict clippy/format, 37 release-script checks, all six builds, exact artifact binding, native input and all three real speech backends passed. Installed settings wheel routing passed. The complete physical screen-selection/OCR/ON IMAGE path remains awaiting the optional user gesture: production deliberately rejects injected mouse events. Do not count that path as passed.
- [x] 7. Verified candidate installed at 2026-10-09T20:46:19Z; replacement launched immediately and remains running. All 30 installed payload hashes, six identities, unchanged protected user data, CUDA readiness and Qwen loopback health verified. Rollback retained. Manual screen acceptance and the existing Windows Ctrl+Alt+P registration conflict remain explicit limitations.

## Integration contracts and overlap
| Tasks | Shared interface | Owner / invariant |
| --- | --- | --- |
| 1, 5 | insertion delivery status | Coordinator changes UI text; agent preserves distinct verified/read/dispatched outcomes. |
| 2, 5 | translation worker lifecycle | Translation agent owns worker; coordinator owns desktop event envelopes. |
| 3, 5 | shutdown deadline/history drain | Speech agent provides joined shutdown; coordinator supplies bounded grace without unsafe termination during normal flush. |
| 4, 5 | desktop main.rs | Coordinator alone edits. |

## Review focus
- A user's newer clipboard value must survive cancellation, failure and delayed duplicate Copy.
- A positive password signal must not become a generic unsupported adapter fallback.
- Clear/delete must invalidate queued old work while allowing genuinely new work.
- Cancellation must reach model startup as well as inference; every terminal path releases its owned state.
- A stopped worker's buffered events must not mutate the replacement's readiness or history.
- Existing visual source files remain byte-identical except explicitly approved text/status or scrolling behavior.
