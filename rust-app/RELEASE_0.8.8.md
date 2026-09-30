# SayFlex 0.8.8

## Screen-result visibility fix

- Fix the result window remaining at the hidden `(-32000, -32000)` prewarm
  position after OCR and translation completed. The presenter parks the native
  window during preparation; the result is now positioned **after** that step
  and before waiting for its first rendered frame.
- Reject a reveal while a window is still parked. Such a failure is logged as
  an error instead of being reported as a successful presentation.
- Positioning now reports invalid native handles and records the actual window
  rectangle alongside the selected region in the diagnostic log.
- Regression tests exercised the native window path: the first result stayed
  off-screen before the fix and was placed on-screen afterward.

## Verification boundary

`cargo test --workspace --locked`, strict workspace Clippy, 37 release-script
tests, and the 8-locale UI translation check passed. The new native-window
regression tests failed on the prior ordering/false-success behavior and pass
with this change.

The automated workspace tests and release validation do not establish that
the installed interface looks or behaves correctly. The user will check the
installed Ctrl+Alt+G selection, visible result, close, repeat and restoration.
This local candidate is not a signed public release.

Earlier 0.8.7 changes and their separate verification status are documented
in [RELEASE_0.8.7.md](RELEASE_0.8.7.md).
