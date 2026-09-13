# SayFlex Clipboard STA Dispatcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the input-host OLE STA responsive for delayed clipboard formats so restored images cannot hang Firefox, Telegram, or another clipboard consumer.

**Architecture:** Replace the insertion worker's blocking Rust channel receive with an event-driven Windows message dispatcher. Commands live in a mutex-protected FIFO and wake the STA with a private `WM_APP` thread message; the worker drains commands before every native wait and dispatches all window messages while idle.

**Tech Stack:** Rust 2024, `windows-sys` Win32 messaging and OLE APIs, standard-library synchronization, existing input-host clipboard transaction layer.

**Spec:** `docs/superpowers/specs/2026-09-13-sayflex-clipboard-sta-dispatcher-design.md`

## Global Constraints

- Windows 10/11 x64 remains the supported desktop platform.
- Preserve exact clipboard-object restoration and insertion delivery semantics.
- Do not use polling, arbitrary delay, `OleFlushClipboard`, or a background thread that does not pump its STA.
- Keep command order FIFO and shutdown explicit.
- Do not claim the cross-process hang fixed until an installed-artifact delayed-image smoke passes.

---

### Task 1: Native STA command dispatcher

**Files:**
- Create: `rust-app/apps/input-host/src/sta_dispatcher.rs`
- Modify: `rust-app/apps/input-host/src/main.rs`

**Interfaces:**
- Produces: `StaCommandSender<T>::send(&self, command: T) -> Result<(), T>`.
- Produces: `StaDispatcher<T>::channel() -> (StaCommandSender<T>, StaDispatcher<T>)`.
- Produces: `StaDispatcher<T>::run(self, handler: impl FnMut(T) -> ControlFlow<()>)`.
- Consumes: `PostThreadMessageW`, `GetMessageW`, `TranslateMessage`, and `DispatchMessageW` on the dispatcher owner thread.

- [ ] **Step 1: Write the failing idle-message and FIFO tests**

  Add Windows-only tests that start a dispatcher thread, wait until its native
  queue exists, post a private non-command message, and assert that an observer
  sees it before a later command. Send three numbered commands followed by
  shutdown and assert `[1, 2, 3]` delivery order.

- [ ] **Step 2: Run the focused tests and verify RED**

  Run: `cargo test -p sayflex-input-host sta_dispatcher -- --nocapture`

  Expected: compilation fails because `sta_dispatcher` and its channel types do
  not exist.

- [ ] **Step 3: Implement the native message-backed FIFO**

  Create a shared `VecDeque<T>`, store the owner thread id only after forcing
  creation of its message queue with `PeekMessageW`, post one private
  `WM_APP + 0x53` wake per enqueue, drain the FIFO before `GetMessageW`, and
  dispatch every non-command message. If posting fails, remove and return the
  exact command that was just appended while still holding the queue lock.

- [ ] **Step 4: Run the focused tests and verify GREEN**

  Run: `cargo test -p sayflex-input-host sta_dispatcher -- --nocapture`

  Expected: idle-message, FIFO, shutdown, and failed-post tests pass.

- [ ] **Step 5: Commit the dispatcher**

  Run: `git add rust-app/apps/input-host/src/sta_dispatcher.rs rust-app/apps/input-host/src/main.rs && git commit -m "fix: keep clipboard STA message loop responsive"`

---

### Task 2: Move insertion commands onto the STA dispatcher

**Files:**
- Modify: `rust-app/apps/input-host/src/insertion.rs`
- Modify: `rust-app/apps/input-host/src/clipboard.rs`
- Test: `rust-app/apps/input-host/src/insertion.rs`

**Interfaces:**
- Consumes: `StaCommandSender<InsertionCommand>` and `StaDispatcher<InsertionCommand>` from Task 1.
- Preserves: `InsertionService::{insert,copy,copy_text,replace_selection,shutdown}`.
- Produces: `ClipboardPasteHost::pump_messages(&mut self)` as `pub(crate)` for transaction-time servicing only.

- [ ] **Step 1: Write the failing worker-liveness test**

  Add a test-only insertion command handler that restores a synthetic command,
  remains idle, receives a posted native message, and then handles shutdown.
  Assert that native message servicing occurs between the two commands without
  a polling timeout.

- [ ] **Step 2: Run the input-host tests and verify RED**

  Run: `cargo test -p sayflex-input-host insertion::tests::idle_insertion_worker_keeps_pumping_its_sta -- --nocapture`

  Expected: the test fails because the worker still blocks in
  `Receiver::recv()`.

- [ ] **Step 3: Replace the blocking receiver**

  Construct the STA dispatcher before spawning the insertion thread. After OLE
  and `ClipboardPasteHost` initialize, run the dispatcher with the existing
  `match InsertionCommand` body as its handler. Return `ControlFlow::Break(())`
  only for `Shutdown`; all other variants return `ControlFlow::Continue(())`.
  Preserve pending-insert cleanup when command posting fails.

- [ ] **Step 4: Run the package regression suite**

  Run: `cargo test -p sayflex-input-host --no-fail-fast`

  Expected: all pre-existing tests plus the new dispatcher and liveness tests pass.

- [ ] **Step 5: Run static Rust gates**

  Run: `cargo fmt --all -- --check`

  Run: `cargo clippy -p sayflex-input-host --all-targets -- -D warnings`

  Expected: both commands exit 0 with no warnings.

- [ ] **Step 6: Commit the insertion migration**

  Run: `git add rust-app/apps/input-host/src/insertion.rs rust-app/apps/input-host/src/clipboard.rs && git commit -m "fix: service restored clipboard objects while idle"`

---

### Task 3: Delayed-image cross-process release smoke

**Files:**
- Create: `rust-app/scripts/test-delayed-clipboard-image.ps1`
- Modify: `rust-app/scripts/installed-smoke.ps1`
- Modify: `docs/release-checklist.md`

**Interfaces:**
- Consumes: installed `SayFlex.InputHost.exe` and a known PNG fixture.
- Produces: a JSON receipt containing source process, consumer process, delay seconds, source SHA-256, received SHA-256, responsiveness result, and process exit codes.

- [ ] **Step 1: Add the failing receipt-schema test**

  Extend the release-script tests to require four receipts with delays
  `[0, 1, 10, 60]`, non-empty process identities, equal source/received hashes,
  and `responsive=true`.

- [ ] **Step 2: Run the script tests and verify RED**

  Run: `pwsh -File scripts/test-release-scripts.ps1`

  Expected: failure because the delayed-image smoke script and receipt fields
  do not exist.

- [ ] **Step 3: Implement the installed smoke harness**

  Make the script place a PNG on the clipboard through a dedicated delayed-data
  owner process, invoke one real SayFlex insertion, wait for each configured
  delay, request the original clipboard image from the chosen consumer adapter,
  and write the receipt only after byte-for-byte hash comparison and a bounded
  responsiveness probe. Never terminate or restart the consumer as a success
  path.

- [ ] **Step 4: Run automated release-script gates**

  Run: `pwsh -File scripts/test-release-scripts.ps1`

  Expected: receipt-schema and safe-target checks pass. Firefox and Telegram
  remain explicitly `NOT_RUN` until the installed interactive smoke.

- [ ] **Step 5: Run the installed Firefox and Telegram matrix**

  Run: `pwsh -File scripts/test-delayed-clipboard-image.ps1 -Installed -Consumers Firefox,Telegram -DelaySeconds 0,1,10,60`

  Expected: eight PASS receipts, unchanged image hashes, responsive consumers,
  and no SayFlex/InputHost cross-process hang event.

- [ ] **Step 6: Commit the release gate**

  Run: `git add rust-app/scripts/test-delayed-clipboard-image.ps1 rust-app/scripts/installed-smoke.ps1 docs/release-checklist.md && git commit -m "test: gate restored image clipboard behavior"`
