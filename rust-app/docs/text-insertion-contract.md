# SayFlex text insertion contract

Automatic insertion has four non-negotiable invariants:

1. The payload sent to the target is the transcript owned by that session.
2. Automatic insertion never changes the user's system clipboard.
3. A delivery result describes only the guarantee made by its transport.
4. An ambiguous specialized-adapter failure is never followed by another
   transport, because that could duplicate text.

## Capability routes

| Route | Eligibility | Delivery guarantee | Clipboard |
| --- | --- | --- | --- |
| Native Edit | Focused `Edit`, `RichEdit`, or WinForms Edit control | Synchronous `EM_REPLACESEL` processed by the control | Untouched |
| WezTerm | Exact `wezterm-gui.exe` target with adjacent CLI | `wezterm cli send-text` returned success for the focused pane | Untouched |
| Unicode input | Any remaining foreground text target | Windows accepted the complete `SendInput` stream | Untouched |

Clipboard APIs remain available only for explicit user actions such as Copy and
for selection capture, where the original clipboard is restored as part of the
capture transaction. They are not an automatic insertion transport.

## Release evidence

The Windows insertion fixture must cover:

- exact multilingual and multiline delivery to a native edit control;
- exact delivery while the target delays input processing;
- a long Unicode payload;
- a concurrent clipboard observer;
- unchanged pre-existing clipboard content, including the regression payload
  `Заказ 6Б-017698831от 29.07.2026`.
