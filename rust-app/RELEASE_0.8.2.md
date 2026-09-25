# SayFlex 0.8.2

## Reliability fixes

- Keeps dictation insertion and clipboard-based selection translation working
  when Firefox advertises a private clipboard format that it cannot materialize.
  Every readable text and image representation is still preserved and restored.
- Refuses the transaction without clearing the clipboard when none of its
  advertised formats can be preserved.
- Restores WezTerm selection translation through the shared `Ctrl+Insert`
  clipboard fallback instead of adding a terminal-specific workaround.

## Translation card

- Fills the complete compact translation popup for short results, removing the
  transparent strip that exposed the page underneath the card.
- Retains stable paragraph publication, the `Translating` shimmer, and smooth
  content-driven growth for longer translations.

## Upgrade

Run the 0.8.2 installer over an existing SayFlex installation. The installer
uses the existing application identity and replaces program files in place;
user settings, history, downloaded models and runtimes under the SayFlex data
directory are preserved.
