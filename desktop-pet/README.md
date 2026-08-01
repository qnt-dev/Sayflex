# SayFlex desktop pet

The desktop pet is an optional, isolated Windows process. It owns one native
layered `HWND`, renders premultiplied BGRA frames on the CPU and never installs
a keyboard hook or opens the microphone.

## Runtime contract

- SayFlex starts `SayFlex.Pet.exe` in the same kill-on-close Job Object as its
  workers.
- The parent writes bounded activity events to the pet's stdin. No key codes,
  dictated audio or text cross this pipe.
- `shutdown`, parent EOF, `WM_CLOSE` and Job Object termination are all valid
  stop paths.
- The overlay is created once and never hidden or shown again; transparency is
  entirely per-pixel through `UpdateLayeredWindow`.
- Runtime files are declared once in `assets/runtime-assets.json`. Publishing
  and release validation consume that same manifest.

Installed builds resolve assets from `pet-assets` beside the executable.
Debug builds additionally fall back to this workspace's `assets` directory;
release builds deliberately do not, so packaging mistakes fail visibly.

## Verification

```powershell
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p pet_host
```

`rust-app/scripts/publish.ps1` builds and packages the process as part of the
normal SayFlex release, and `validate-release.ps1` hashes the executable plus
every manifest-listed runtime asset.
