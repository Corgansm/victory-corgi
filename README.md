# victory-corgi

A fork of [VIctory](https://github.com/Marvisak/VIctory), the Rust compatibility layer that runs
an Xbox Series title on Windows from files you supply. This tree is tuned for NVIDIA cards and
tested with GTA VI (Xbox Series build 1.0.2.1, from the author's own console).

**No game files, code or assets are in this repository, and none are linked.** You need your own
copy of the title. Microsoft's GDK is not used and not included. The project is not affiliated
with the game's developer or publisher, Microsoft or NVIDIA. See
[docs/asset-policy.md](docs/asset-policy.md).

## Status (2026-11-27)

One test machine: Windows 11, Intel Core i9-14900K, GeForce RTX 3080 Ti (12 GB), 32 GB RAM,
Series S profile, rendered at 748x421 and upscaled to 1920x1080.

| Stage | State | Notes |
|---|---|---|
| Loads (PE loader, imports resolved) | ✅ | title executable and its DLLs mapped, console CPU instructions emulated |
| Boots (intro videos, loading screen) | ✅ | 120 fps with the frame rate cap at 120 |
| Menu | ✅ | |
| In-game: prologue and cutscenes | ✅ | 20-45 fps |
| In-game: first mission, on the boat | ✅ | 25-30 fps |
| In-game: open world | 🟡 | about 15-20 fps |
| Ray-traced light | 🟡 | experimental, behind switches, see below |
| Stability | 🟡 | runs of 9 to 11 minutes ended by hand; two known faults below |
| Saves | ✅ | load and are written |

GTA VI boots: the intro videos and the loading screen run at 120 fps.
GTA VI reaches the title screen and the main menu.
GTA VI is in-game: the prologue, the first mission and the open world all render.
GTA VI's first mission, on the boat, is playable at 25-30 fps.

## How it works

- **Loader.** A PE loader maps the title's executable and DLLs, resolves their imports against
  the layer's own modules (HLE of the console runtime) and emulates the few console CPU
  instructions a PC processor lacks.
- **Graphics.** The title's Direct3D 12 calls for the console (`d3d12_xs`) are replayed on the
  PC's D3D12: every command list, descriptor heap and pipeline has a host twin.
- **Shaders.** The console's shader code is recompiled to DXIL, about 4,200 programs, cached on
  disk; the layer's own passes are written in HLSL.
- **Textures.** BCPack textures are unpacked and detiled (the console's tiling undone) before
  upload.
- **Memory.** Title buffers get copies in the card's memory, and what shaders write there is
  brought back for the title to read.

## What this tree adds

- **Settings while the game runs**: `VICTORY_LIVE_FILE` names a file the game reads again
  whenever it changes; `scripts\live-menu.ps1` is a slider window over it and F8 shows the same
  settings in the game (ambient light, crease shade, air light, memory caps, ray-tracing switch).
- **Frame rate cap**: `VICTORY_FRAME_CAP`, 30 to 240.
- **A structure builder of the layer's own for ray tracing.** The console's builder DLL resets
  NVIDIA cards, so the layer stands in front of its exports (`VICTORY_BVH_HOOK=1`) and does the
  builds itself (`VICTORY_BVH_NATIVE=all`): the static world unpacked on the CPU, scene-wide
  structures written on the CPU, build-once meshes built on the CPU, and skinned meshes written
  by a compute shader placed in the title's command list at the build's position.
- **Tracing on the card's own ray tracing (DXR)**: `VICTORY_DXR=1` builds host acceleration
  structures from the above and `VICTORY_DXR_GUIDED=1` recompiles the title's ray shaders to ask
  those first.

Ray tracing is experimental: in one indoor scene the host's structure held 1,838 of the title's
1,895 instances, at 15-20 fps. Open points: its light has to be checked again on a fresh pipeline
cache, big scenes run short of the 512 MB the host structures are allowed, and a moving
character's shadow lags.

## Build and run

Windows 10/11 x64, a CPU with AVX2, an NVIDIA card with mesh shader support and shader model 6.6,
Rust (toolchain pinned), the MSVC build tools, and your own dump of the title.

```powershell
git clone https://github.com/Corgansm/victory-corgi
cd victory-corgi
copy victory.example.toml victory.toml
cargo build --locked --profile diag -p victory --features diagnostics
target\diag\victory.exe shaders <game folder>\shaders.rpf
scripts\play-highram.ps1 -Exe <game folder>\game_scarlett_master.exe
```

`-HighRam:$false` suits smaller cards, `-HalfScale` halves the render size, `-SeriesX` picks the
sharper and slower profile, `-More @{VICTORY_X='1'}` passes any other setting. Keep the game
folder outside this one, and block `victory.exe` in the firewall: the title tries to send crash
reports.

## Tests

`cargo test --locked -p victory-gpu --features diagnostics --lib` runs about 450 unit tests,
among them the decoders and builders of the ray-tracing structures. The shader crate has tests
that run translated programs on the card against a reference.

## Known faults

- The host pipeline cache goes bad after some builds and runs: wrong lighting, sunlight and haze
  through walls, broken water. Deleting the `pipelines` folder inside the cache folder
  (`VICTORY_CACHE_DIR`) cures it; the cause is still being looked for.
- After that folder is deleted, each pipeline is created again on first use, and the first
  minutes of the following run stall for seconds at a time.
- About one start in three ends 32 seconds in with an access violation in the title's own code.
  The following start works.
- Sliders in the live settings return to their defaults at every start.

## Credits

This tree stands on other people's work; the commit history names each author.
[Marvisak/VIctory](https://github.com/Marvisak/VIctory) is the project itself. The direct parent
is [Loydben/VIctory-NVIDIA-HighRAM](https://github.com/Loydben/VIctory-NVIDIA-HighRAM), whose
README is kept whole in [docs/README-loydben.md](docs/README-loydben.md) with its own measurements
and its credits to avatarmanz, trustno1-coder, cobrez, fxceless and the authors of the pull
requests it carries. Earlier READMEs: [docs/README-fxceless.md](docs/README-fxceless.md),
[docs/README-avatarmanz-cobrez.md](docs/README-avatarmanz-cobrez.md). Notes on the console's
ray-tracing structures: [docs/fork-ray-tracing.md](docs/fork-ray-tracing.md).

## License

GPL-3.0-or-later, as VIctory: [LICENSE](LICENSE).
