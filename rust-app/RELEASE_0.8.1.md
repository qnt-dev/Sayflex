# SayFlex 0.8.1

## Highlights

- Adds built-in screen translation with local OCR and multi-monitor capture.
- Adds compact language-specific translation packs alongside the multilingual
  quality models.
- Adds a resources page that reports current CPU, RAM and GPU memory use and
  lets the user release resident speech, translation and OCR resources.
- Publishes completed translation paragraphs progressively in a stable,
  smoothly resizing result card.
- Refreshes the keyboard-layout correction card to match the compact SayFlex
  overlay design and removes its obsolete instruction footer.

## Reliability

- Preserves clipboard data across retry paths and avoids blocking restored
  delayed-rendering clipboard images.
- Makes model-download cancellation interrupt stalled reads.
- Rejects incomplete translation output instead of presenting it as complete.
- Releases a previous local translation model before loading its replacement.
- Rebuilds screen-selection surfaces after monitor or DPI changes and keeps
  repeated capture hotkeys bounded.

## Upgrade

Run the 0.8.1 installer over an existing SayFlex installation. The installer
uses the existing application identity and replaces program files in place;
user settings, history, downloaded models and runtimes under the SayFlex data
directory are preserved.
