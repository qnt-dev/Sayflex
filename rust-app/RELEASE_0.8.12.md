# SayFlex 0.8.12

Fixes from the independent audit of 0.8.11 (2026-10-21). Each item names the
audit finding it closes.

## Dictation

- Silence and noise no longer become text such as "Thank you.". The Silero
  voice activity model checks every recording before Whisper, and segments
  with Whisper's own no-speech signature are dropped (SPE-01).
- With "Auto", the language is chosen among the languages set up in Windows
  (display languages and keyboard layouts), not among all 99 (SPE-02).
- On a computer without a dedicated graphics card, the first run recommends
  Whisper Small instead of Large v3 Turbo, and the CPU worker no longer spends
  seconds on a warm-up pass at every start (SPE-03).
- A processor without AVX2 runs a speech worker built for it, and the compact
  translator uses its portable kernel there (SPE-04).

## Inserting text

- When the window changed before the paste, a notice says so and offers
  **Insert** (into the window now in front) and **Copy**. The text is never
  inserted elsewhere on its own (INS-02).
- A notification or tooltip that takes the foreground for a moment no longer
  cancels the insertion (INS-01).
- The visualizer disappears as soon as the paste is sent (INS-04).
- Applications that read only the old ANSI clipboard format receive the text
  in it (INS-03).
- New option Settings → Dictation → Insertion → **Type the text** keeps
  dictation off the clipboard (INS-06).

## Translation

- Numbers, dates, times and links are checked after translation; a changed
  value is translated again with a marker and restored. What still differs is
  pointed out (TR-01).
- A lone acronym such as "OK" is left as it is.
- A phrase read as a language without an installed package falls back to an
  installed one of the same script instead of failing (SPE-02).
- New installations translate into the Windows language.

## Screen translation

- Each line is read with the language model that reads it best, mixed
  Latin/Cyrillic look-alikes are corrected, and vertical text is read (SCR-02).
- Selecting an area no longer stops after 15 s while the mouse moves (SCR-01).
- While a block is edited, the others fade without showing the source text
  under them (SCR-03).

## Interface and resources

- Starting SayFlex again shows the running instance's settings (UI-01).
- Missing translations, screen-reader names and number agreement fixed (UI-02);
  clearer Overview states and statuses without raw milliseconds (UI-03).
- The 60 Hz animation timer and the global mouse hook run only while something
  needs them; the sphere visualizer costs about a third less (PERF-01/02/04).
- The speech worker no longer polls in the background (PERF-03).
- An idle contextual translation model is released after ten minutes (O15).

## Installation, licensing and security

- No NVIDIA file is in the installer. The CUDA worker links the CUDA runtime
  dynamically; cuBLAS and cudart are downloaded inside the app on request
  (SignPath requirements).
- The Visual C++ runtime is installed with SayFlex (INST-01); uninstalling asks
  whether to delete models and personal data (INST-02); the installation is no
  longer modified by the app, and only the parts of llama.cpp that SayFlex runs
  are kept (INST-03).
- The GPL Battle for Wesnoth screenshot is no longer in the program (LIC-01);
  third-party notices contain the full license texts (LIC-02).
- Dependency advisories fixed (`cargo deny` passes), every download requires
  HTTPS, and the local translation server's port is checked (SEC-01/02/03).
- Privacy policy (`PRIVACY.md`) and a draft code signing policy.
