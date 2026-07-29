# StyleFlex 0.1.3

This release replaces the unsafe `Ctrl + Windows` implementation from 0.1.2.

## Hold-to-talk

- The default gesture is now `Ctrl + Alt + Space`.
- `Space` is the only suppressed trigger key.
- Physical `Ctrl`, `Alt`, `Shift`, and Windows keys are never released through
  synthetic input.
- The input host uses deterministic `Idle`, `Active`, and `Draining` states.
- Own and foreign injected events cannot start or end dictation.
- Auto-repeat cannot create duplicate recording sessions.
- Text insertion waits for a stable release of physical modifiers.

## Recording indicator

- The overlay is created transparently and receives its non-activating Windows
  styles before becoming visible.
- It is positioned before each show and no longer restores focus through
  `SetForegroundWindow`.
- One accepted session owns one continuous `Listening -> Processing` lifecycle.

## Recovery

- `Shift + Alt + X` still copies the latest completed transcript.
- `Ctrl + Alt + K` still opens explicit layout-correction choices.
- The existing C# application is unchanged.
