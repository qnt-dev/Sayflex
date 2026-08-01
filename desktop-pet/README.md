# Desktop Pet

This directory is an independent Rust application and product workspace. It is
not a SayFlex component, worker, optional feature, or release dependency. The
two products share only the parent repository and the Rust engineering stack.

The current technical name `DesktopPet` is intentionally neutral until the
commercial product receives its own final name.

## Runtime contract

- `DesktopPet.exe` starts and runs on its own.
- It never launches SayFlex and SayFlex never launches it.
- The default runtime does not read stdin, install a keyboard hook, or open a
  microphone.
- `--stdin-control` explicitly enables a small product-neutral control protocol
  for automated checks and future integrations. EOF terminates only instances
  launched in that mode.
- The native layered overlay is created once and remains alive until
  `WM_CLOSE`, `WM_QUIT`, or an explicit `shutdown` control command.
- Runtime files are declared in `assets/runtime-assets.json` and published only
  by this workspace's own scripts.
- Traversal decisions and fixed, allocation-free action timelines live in the
  independent `pet_motion` crate. Rendering consumes those plans; it does not
  choose mechanics or run per-layer action timers.

## Character production assets

The approved visual contract is documented in
`docs/CHARACTER_ASSET_SPEC.md`. Prompts written specifically for sequential
OpenAI Image editing live in `assets/OPENAI_IMAGE_PRODUCTION_RIG_PROMPTS.md`.
Generate and approve the clean front master before any action pose; later
images inherit identity from that approved master.

Approved masters are converted into the small runtime coordinate system by
deterministic, hash-pinned build scripts:

```powershell
python .\assets\prepare_front_master.py
python .\assets\extract_stand.py
python .\assets\extract_profile_motion.py
```

`extract_stand.py` deliberately keeps the approved front pose intact.
Locomotion uses the right/left profile layers, with conservative overlap masks
around the jacket, pelvis and tail root so the flattened source cannot split
apart while moving.

Debug builds may load generated assets from the workspace. Published builds
load the allow-listed files from `assets` beside `DesktopPet.exe` and fail
visibly if the package is incomplete.

## Run locally

```powershell
cd C:\PRDJKT\qntflex\desktop-pet
cargo run --release -p pet_host
```

The default `confident` walk is the character's springy, self-assured motion.
The same articulated rig also supports a quieter, longer-support gait without
duplicating assets or bypassing IK:

```powershell
cargo run --release -p pet_host -- --walk-style woodland
```

Both styles keep the planted paw locked to the desktop. Running is a separate
procedural cycle with an aerial phase, not ordinary walking played faster.

The standalone process remains alive after launch. Close it through Task
Manager during this prototype phase, or send `WM_CLOSE` from a test harness.

## Build and validate the independent package

```powershell
.\scripts\publish.ps1 -Configuration Release
.\scripts\validate-release.ps1
```

Output: `dist\app\DesktopPet.exe` plus `dist\app\assets`.

## Engineering verification

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p pet_host
```
