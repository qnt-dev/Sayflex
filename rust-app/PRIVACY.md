# SayFlex privacy policy

SayFlex is a Windows application for dictation, translation of selected text
and translation of a screen area. It is designed to work on your computer. This
page lists everything SayFlex stores, everything it sends over the network, and
when.

## What stays on your computer

By default, speech recognition, text translation with an installed language
package, and screen text recognition run locally. Audio, text and screenshots
are not sent anywhere.

SayFlex stores its data in `%LOCALAPPDATA%\SayFlex`:

- **Settings** (`settings.json`).
- **Dictation history** (`history.json`) and **translation history**
  (`translation-history.json`), up to 250 entries each, as plain text. History
  can be switched off in Settings and cleared there at any time.
- **Screen translation images** (`screen-translations`), kept for at most 24
  hours by default; the retention can be changed or switched off.
- **Service keys** you enter for optional online providers (`credentials.dat`,
  `translation-credentials.dat`), encrypted with Windows DPAPI for your user
  account.
- **Downloaded models and runtimes** (`models`, `translation-models`,
  `runtimes`).
- **Diagnostic logs** (`logs`). Logs record timings, window classes, process
  names and error codes. They do not contain dictated or translated text,
  window titles, or keys; an insertion is identified by its length and a
  SHA-256 fingerprint only.

## Network connections

SayFlex connects to the internet only in these cases, and only over HTTPS:

| When | Where | What is sent |
|---|---|---|
| You download a speech model | huggingface.co | a file request |
| You download a translation package | Mozilla's Firefox Translations storage, huggingface.co | a file request |
| You choose a contextual translation model | github.com (llama.cpp release) | a file request |
| You press **Accept and download** for NVIDIA acceleration | developer.download.nvidia.com | a file request |
| You enable an online speech or translation provider and enter its key | the provider you chose | the audio or text of that request, as that provider's terms describe |

Every downloaded file is checked against a pinned SHA-256 value before use.
SayFlex sends no telemetry, analytics or crash reports.

### NVIDIA components

The installer contains no NVIDIA files. On a computer with an NVIDIA graphics
card, Settings → Dictation offers optional CUDA acceleration. Only after you
press **Accept and download** (which accepts the NVIDIA CUDA Toolkit EULA,
linked next to the button) does SayFlex download two archives from
`developer.download.nvidia.com/compute/cuda/redist/`: `libcublas` and
`cuda_cudart` from CUDA 13.3.1. It keeps `cublas64_13.dll`,
`cublasLt64_13.dll`, `cudart64_13.dll` and the NVIDIA license in
`%LOCALAPPDATA%\SayFlex\runtimes`, checks each archive and file by SHA-256,
and never installs a driver or the CUDA Toolkit.

## The clipboard

To insert dictated text into applications that do not accept it directly,
SayFlex by default places the text on the clipboard for the duration of one
paste and then restores what was there before. The text is marked so that
Windows clipboard history and cloud clipboard do not keep it. Programs that
watch the clipboard can still read it during that moment: clipboard managers,
and the clipboard synchronization of remote sessions such as Remote Desktop and
WSLg. If that is not acceptable, choose **Type the text** under Settings →
Dictation → Insertion: SayFlex then types the text key by key and does not use
the clipboard for dictation. Typing is slower, and editors with automatic
completion may change what is typed.

## Removing SayFlex and your data

Uninstall SayFlex from Windows Settings → Apps. The uninstaller asks whether to
delete the downloaded models and, separately, your history, settings and keys.
To remove the data by hand, close SayFlex and delete
`%LOCALAPPDATA%\SayFlex`.

## Contact

Questions about privacy: open an issue in the SayFlex repository.
