# Code signing policy

> **Draft for the SayFlex Foundation application to SignPath.** The items marked
> *owner decision* must be completed by the project owner before submission.

Free code signing for Windows releases is requested from
[SignPath.io](https://about.signpath.io/), with a certificate by the
[SignPath Foundation](https://signpath.org/). Until approval, released binaries
are unsigned.

## What is signed

Only binaries built from this repository's public source by its release
workflow on GitHub-hosted runners:

- `SayFlex.exe` and the workers `SayFlex.InputHost.exe`,
  `SayFlex.Speech.Cpu.exe`, `SayFlex.Speech.CpuCompat.exe`,
  `SayFlex.Speech.Vulkan.exe`, `SayFlex.Speech.Cuda.exe`,
  `SayFlex.Translation.exe`, `SayFlex.Ocr.exe`, `SayFlex.Companion.exe`;
- the Inno Setup installer `SayFlex-<version>-win-x64-setup.exe`.

## What the installer contains

All components are open source under OSI-approved licenses, listed with their
full texts in `THIRD_PARTY_NOTICES.md`, except:

- the Microsoft Visual C++ runtime DLLs (`vcruntime140*.dll`, `msvcp140*.dll`),
  redistributed unmodified under the Visual Studio redistribution terms;
- the Slint UI toolkit, used under the Slint Royalty-free Desktop License 2.0.
  *Owner decision:* SignPath Foundation accepts OSI licenses only; the options
  are a Slint license compatible with the Foundation's rules (for example the
  GPL-3.0 option for the whole application) or a confirmation from SignPath.

### NVIDIA CUDA

The installer contains **no NVIDIA files**. The CUDA speech worker is compiled
from open source (whisper.cpp/ggml, MIT) and links the CUDA runtime
**dynamically** (`cudart64_13.dll`); the release validator rejects a CUDA worker
that does not import it and any package that contains NVIDIA DLLs.

NVIDIA runtime files are **not downloaded during setup**. Inside the running
application, Settings → Dictation shows optional NVIDIA acceleration. Only when
the user presses **Accept and download**, after a notice that this accepts the
NVIDIA CUDA Toolkit EULA (with a link to it), SayFlex downloads from
`https://developer.download.nvidia.com/compute/cuda/redist/`:

| Archive | SHA-256 | Files kept |
|---|---|---|
| `libcublas-windows-x86_64-13.6.0.2-archive.zip` | `62e9fa30560c8f0a28e0cdcf9d6fc1fed347bcfab8847239b9ae1fdc1d86408a` | `cublas64_13.dll`, `cublasLt64_13.dll`, `LICENSE` |
| `cuda_cudart-windows-x86_64-13.3.29-archive.zip` | `1feb7dd266813ffe8dbc24e115183a5ac35a4795c8d34aca0df85ab616b64d9c` | `cudart64_13.dll` |

Each archive and each extracted file is verified by SHA-256 before it is used
(`crates/model-manager/src/cuda_runtime.rs`). The files are stored per user in
`%LOCALAPPDATA%\SayFlex\runtimes`, outside the installation, and are not signed
by SayFlex. Without them SayFlex uses its CPU or Vulkan speech workers.

## Team roles

*Owner decision:* fill in the people and their accounts.

- Committers and reviewers: …
- Approvers (approve each signing request): …

All accounts with these roles use multi-factor authentication. Every release
signing request is approved manually.

## Privacy

See [PRIVACY.md](PRIVACY.md). SayFlex sends no telemetry; network connections
happen only for downloads the user starts and for online providers the user
configures.
