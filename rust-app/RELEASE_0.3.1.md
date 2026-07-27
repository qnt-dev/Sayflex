# Rust Voice 0.3.1

Critical Windows terminal safety hotfix:

- selection capture now uses `Ctrl+Shift+C` in WezTerm;
- ordinary `Ctrl+C` is never injected into WezTerm, so text actions cannot send
  `SIGINT` to the active terminal process;
- synthetic shortcut recovery releases only keys owned by the application;
- terminal copy policy is covered by regression tests.

No speech, translation model, settings, or history formats changed.
