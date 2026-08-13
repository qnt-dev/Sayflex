# SayFlex text insertion contract

Automatic insertion has five non-negotiable invariants:

1. One immutable payload belongs to one dictation session.
2. A route is attempted at most once; ambiguous delivery is never followed by
   another transport because that can duplicate text.
3. `SendInput` acceptance is never reported as text delivery.
4. A compatibility clipboard transaction restores the captured OLE
   `IDataObject` only while SayFlex still owns the clipboard. A newer user Copy
   always wins and is never overwritten.
5. The result names the strongest observation actually made: `committed`,
   `payload_read`, `verified`, `dispatched`, `not_delivered`, `superseded`, or
   `ambiguous`.

## Capability routes

| Route | Eligibility | Delivery observation | Clipboard |
| --- | --- | --- | --- |
| Native Edit | Focused `Edit`, `RichEdit`, or WinForms Edit control | Synchronous `EM_REPLACESEL` completed | Untouched |
| WezTerm | Exact `wezterm-gui.exe` target with adjacent CLI and `gui-sock-{pid}` | `wezterm cli send-text` returned success for that GUI's focused pane | Untouched |
| Clipboard transaction | Foreground target without an acknowledged adapter | Exact target requested delayed payload; UI Automation optionally observed one new exact payload occurrence; otherwise `dispatched` after a bounded payload lease | Previous OLE object restored if SayFlex still owns the clipboard |

The compatibility route publishes only `CF_UNICODETEXT` plus the Windows
privacy formats `ExcludeClipboardContentFromMonitorProcessing`,
`CanIncludeInClipboardHistory=0`, and `CanUploadToCloudClipboard=0`. A
long-lived serialized insertion worker owns the delayed-render window and the
OLE apartment. There is no Unicode keystroke fallback.

Clipboard synchronizers such as Remote Desktop may request delayed data before
the target. Such a read is not treated as delivery and an anonymous
`OpenClipboard(NULL)` read is never attributed to the foreground process.
UI Automation is an optional verification channel, not an eligibility gate.
If a broker materializes the delayed payload before an unobservable target
reads it, SayFlex keeps the transaction alive for a bounded compatibility
lease, restores the previous clipboard only while it still owns it, and
reports `dispatched` rather than claiming verified delivery.

## WezTerm instance binding

InputHost is normally started by SayFlex and therefore does not inherit
`WEZTERM_UNIX_SOCKET`. The adapter derives the socket filename from the
captured `wezterm-gui.exe` process id, verifies that it exists, injects it into
both `list-clients` and `send-text`, and then selects the client entry with the
same process id. This prevents both silent failure and cross-instance delivery.

## Release evidence

The Windows insertion fixture must cover:

- exact multilingual and multiline native-edit delivery;
- a target that delays paste processing longer than one second;
- a long compatibility payload delivered in one operation;
- a compatibility target that accepts paste while UI Automation verification
  is unavailable;
- unchanged final clipboard contents, including the regression payload
  `Заказ 6Б-017698831от 29.07.2026`;
- a read-only probe of the live WezTerm instance through its exact socket;
- validation against the installed release binaries, not only debug output.
