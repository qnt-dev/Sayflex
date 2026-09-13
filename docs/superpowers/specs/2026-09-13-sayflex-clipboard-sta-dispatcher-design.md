# SayFlex Clipboard STA Dispatcher Design

## Problem

The input-host insertion worker initializes OLE as an STA, captures the current
`IDataObject`, temporarily publishes dictated text, restores the original data
object with `OleSetClipboard`, and then blocks in `std::sync::mpsc::Receiver::recv`.
An image or another delayed-rendered clipboard object can therefore remain
owned through an STA that is not dispatching Windows messages. Firefox and
Telegram can block while requesting that restored data through OLE.

## Required behavior

- The OLE apartment that captures and restores `IDataObject` must continuously
  dispatch Windows messages for its entire lifetime, including while idle.
- Command delivery must wake the same native message loop without polling.
- A command wake consumed while a clipboard transaction is already pumping
  messages must not lose or strand the command.
- FIFO command order and insertion coalescing remain unchanged.
- Shutdown remains explicit and bounded.
- Text insertion must still restore the exact pre-existing clipboard object
  when the transaction owns the clipboard at settlement time.
- No `OleFlushClipboard`, arbitrary sleep, or second unpumped STA may be used as
  a substitute for apartment servicing.

## Architecture

Add a small Windows STA dispatcher owned by input-host. A cloneable sender
pushes commands into an `Arc<Mutex<VecDeque<T>>>` and calls
`PostThreadMessageW` with a private `WM_APP` message. The worker uses
`GetMessageW`, `TranslateMessage`, and `DispatchMessageW` whenever the command
queue is empty. It always drains the shared queue before blocking again, so a
wake message removed by the clipboard transaction's existing message pump does
not strand its corresponding command.

The dispatcher is transport only. Clipboard capture, paste verification,
restoration, target checks, and event reporting remain in their current owners.

## Verification

- A Windows unit test leaves the dispatcher idle, posts an unrelated thread
  message, and proves that message is observed before any command is sent.
- Unit tests prove FIFO delivery, explicit shutdown, and failed-post recovery.
- Existing input-host tests remain green.
- A manual cross-process smoke matrix restores a delayed image and requests it
  after 0, 1, 10, and 60 seconds while SayFlex remains idle. Firefox and
  Telegram must remain responsive and receive the original image.

## Release boundary

Unit tests establish the dispatcher contract. The clipboard hang is not called
closed until the delayed-image cross-process matrix is run against an installed
artifact; absent that evidence the status is `awaiting installed verification`.
