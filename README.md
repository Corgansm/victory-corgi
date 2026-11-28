# VIctory on NVIDIA, for PCs with plenty of RAM and video memory

A fork of [VIctory](https://github.com/Marvisak/VIctory), the compatibility layer that runs an Xbox
Series title natively on Windows from files you supply, put together for **NVIDIA cards on PCs with
a lot of memory**. It runs GTA VI (Xbox Series build 1.0.2.1, your own copy) at about
**16-25 fps** on one RTX 5090 Laptop GPU, in cutscenes and on the boat of the first mission.

**This repository contains no game files, code or assets.** You need your own copy of the game.
It is not affiliated with or endorsed by the game's developer or publisher, Microsoft or NVIDIA.
See [docs/asset-policy.md](docs/asset-policy.md).

## This tree (Corgansm/victory-corgi), 2026-11-27

This repository is that fork carried on, on a second, smaller PC: Windows 11, Intel Core
i9-14900K, NVIDIA GeForce RTX 3080 Ti (12 GB), 32 GB RAM, Series S profile, the world rendered at
748x421 and upscaled to 1920x1080. Everything in this section was measured on that one machine.
The sections after it are the parent fork's own, as it wrote them on 2026-11-25 and 26.

| | |
|---|---|
| Loading screens and videos | **120 fps** (with the frame rate cap at 120, `VICTORY_FRAME_CAP`) |
| Without ray tracing, first mission, on the boat | **25-30 fps** |
| Without ray tracing, cutscenes | **20-45 fps** |
| Without ray tracing, open world | about **15-20 fps** |
| With the native ray tracing below, open world and prologue | 15-20 fps |
| Longest runs on 2026-11-27 | 9 to 11 minutes each, ended by hand |

### Settings while the game runs

`VICTORY_LIVE_FILE=<file>` names a file of `NAME=value` lines that `victory.exe` writes when a run
starts and reads again whenever it changes. `scripts\live-menu.ps1` is a window of sliders over
that file, and F8 opens the same settings in the game: render scale (for the next start), ambient
light, crease shade and radius, air light limit, the card-buffer and BAR-window caps, the stats
panel, the ray-tracing switch and a frame rate cap (`VICTORY_FRAME_CAP`, 30 to 240). The window's
"Restart game" closes the game and starts it again. Each start writes the launch values over the
file, so sliders go back to their defaults.

### Native ray tracing (experimental, off by default)

The parent fork leaves the title's ray tracing off, because the console's structure builder
(`XboxBvhBuilder.dll`) resets NVIDIA cards. This tree replaces the builder step by step and
traces with the card's own ray tracing (DXR):

- `VICTORY_BVH_HOOK=1` puts the layer in front of the builder's exports; `VICTORY_BVH_NATIVE=all`
  has the layer do the builds itself: the static world's structures unpacked on the CPU, the
  scene-wide structures written on the CPU, the bottom levels the title builds once built on the
  CPU, and the ones of meshes the title skins on the card written by a compute pass of the
  layer's at the build's place in the title's command list.
- `VICTORY_DXR=1` builds host acceleration structures from those, and `VICTORY_DXR_GUIDED=1`
  translates the title's ray programs so that a walk asks the host's structures first.
  `VICTORY_RAY_TRACING` in the live file switches it: 0 off, 1 the title's own walk, 2 and 3 the
  host's structures.

All of it together (leave `VICTORY_BVH_MISS` empty and `VICTORY_BVH_BUILDER` unset):
`VICTORY_BVH_HOOK=1 VICTORY_BVH_NATIVE=all VICTORY_DXR=1 VICTORY_DXR_GUIDED=1`. It was run through
the author's own launcher with these settings, not through `scripts\play-highram.ps1`.

Where it stands: in one indoor scene the host's scene-wide structure held 1,838 of the title's
1,895 instances, and five starts in a row went through loading into the world without the card
being lost (before a fix of 2026-11-27, about two of three did). It is not ready to play with:

- Whether the light is right with it is not known. The runs that showed sunlight and haze passing
  through walls used a pipeline library that had gone bad (below); it has not been run on a fresh
  one since.
- The host's structures are allowed 512 MB and a 12 GB card is near its budget with them: in big
  scenes several hundred structures wait for room while the scene loads.
- The host's copy of a moving mesh is rebuilt only every 60 frames, so a moving character's
  shadow may have holes.
- Traced shadows have blocky edges on characters.

`docs/fork-ray-tracing.md` describes the console's structures.

### Known to be wrong in this tree

- **The host pipeline library goes bad** after some builds and runs: flat or wrong lighting,
  sunlight and haze through walls, broken water. It happened twice on 2026-11-27. Deleting the
  `pipelines` folder of the cache folder (`VICTORY_CACHE_DIR`) puts it right; the translated
  shaders in the same cache were byte for byte the same before and after, so they need no
  recompiling. Why it goes bad is not found yet.
- After that folder is deleted every pipeline is made again on first use: the first minutes of the
  next run stall for seconds at a time (about 2,000 pipelines in the run measured).
- Now and then the title stops about 32 seconds after the start (an access violation in its own
  code). Starting again works.

## Where the parent fork stood (2026-11-25)

Tested on one machine only: Windows 11, Intel Core Ultra 9 275HX, NVIDIA GeForce RTX 5090 Laptop
GPU (24 GB), 64 GB RAM, Series S profile.

| | |
|---|---|
| Menus, videos, loading screens | 30 fps |
| Opening cutscenes | 40-42 ms a frame (**24-25 fps**) |
| First mission, on the boat | 60-75 ms a frame (**13-16 fps**) with the launcher's settings |
| Longest session so far | 30 minutes and still running, no crash and no device loss |
| Waiting for the graphics card each frame | 0 ms (it was 58 ms before `VICTORY_CARD_ALL`) |
| Mouths move in cutscenes | yes |
| Boats float | yes |

The same build with the parent forks' settings ran at 9-13 fps on this PC; the jump came from one
switch, `VICTORY_CARD_ALL=1` (below).

New on 2026-11-26: **no blue blocks in the sky and no black slabs on the water.** NVIDIA's
driver refused eight of the title's compute programs ("Shader wants wave lane count of 64":
its waves have 32 lanes, the console's 64). fxceless pull request 854 (PiloteCode) translates
such programs in a paired form, each console wave in one host wave with two lanes a thread; it
is ported here, and the alpha transmission prefilter in that form removes the blue blocks in
the sky. The same form for the water's transmission volumetrics drew black slabs under
islands, piers and bridges, so those stay out (`VICTORY_PAIRED_ONLY` in the launcher); and the
PR also moved 60 programs that already worked (the temporal resolves among them) to that
form, which here melted the picture: they keep their earlier translation. Float compares now
tell NaN by its bits (`VICTORY_NAN_EXACT`), as NVIDIA's driver may fold a compare's NaN case
away. The frame rate shows on the picture (`VICTORY_PANEL=1`, F3 hides it).

Later the same day: **turquoise water.** The program taken for the water's volumetrics
(DXBC digest `581dab2c...`) is its tile classifier: a wave per 8x8 tile, which lists the
tiles the water's passes then shade. Three of those passes (`50cf2832...`, `24126c14...`,
`7099a57f...`) were refused by NVIDIA's driver as well. The classifier alone in the paired
form listed tiles their passes never shaded: the dark slabs at piers and islands. All four
left out: no tiles, dark navy water everywhere. All four in the paired form
(`VICTORY_PAIRED_ONLY`): the water as on the console.

`VICTORY_PIECE_KB` (4, 8, 16, 32 or 64; 64 by default): the size of the pieces in which what
shaders wrote to host copies on the card is compared, fetched and landed. Smaller pieces fetch
less around a few bytes written. With `VICTORY_FETCH_CHECK=1` a landed range that spanned
several pieces read past its piece in the fetch (`land::ordered` joins ranges within 64 KiB):
the card hung at the first load with any size but 64; the ranges are now cut at pieces.

Also new: **no more melting.** With the title's list calls recorded on threads of their own
(`VICTORY_THIN=1`, the base fork's default), NVIDIA's capped descriptor pool ran full: about
500 descriptor tables a frame refused and 300 indirect draws dropped, so the water went
missing in tiles and the picture smeared and melted the longer one played (the same as
avatarmanz/VIctory-7900XTX issues 35, 62 and 112 on other NVIDIA cards). The launcher now sets
`VICTORY_THIN=0`: 11 refusals a frame on average, a clean picture, about 1.5 fps slower.

What is known to be wrong:

- Now and then a texture flickers or the frame stutters when a new area loads: the descriptor
  pool still runs full for a moment there.
- Lighting is flat and colours are off indoors and in shade: the title's ray-traced light does
  not run yet (`VICTORY_BVH_BUILDER=1` resets the card within seconds on NVIDIA). For this tree's
  work on it, see "Native ray tracing" above.
- The water is the darker blue of the parent forks: its transmission volumetrics are off
  until their paired form draws right.
- A faint grid of darker diamonds on the near water in some views.
- In-world screens (the boat's navigation display) stay dark.
- The picture is softer than the Series X profile: the world is rendered at the Series S size and
  upscaled by the title's own upscaler.
- The first run with a new build or driver stutters while pipelines are made.

## Why it needs a lot of memory

`VICTORY_CARD_ALL=1` gives every title buffer a copy in the graphics card's memory. Without it,
buffers that live only in the PC's memory make the thread that prepares each frame wait for the
card to finish before it may read them: 58 ms of a 104 ms frame on this PC. With it the wait is
gone, at the cost of video memory (this PC has 24 GB). The parent fork measured 40-46 GB of
private memory: **64 GB of RAM and a 16-24 GB card are what this was tuned on.** On smaller cards,
leave `-HighRam` off in the launcher (below) and compare.

## How to run it

You need Windows 10/11 x64, a CPU with AVX2, an NVIDIA card with mesh shaders and shader model 6.6,
Rust (the toolchain is pinned), the MSVC build tools, and your own dump of the title.

```powershell
git clone https://github.com/Corgansm/victory-corgi
cd victory-corgi
copy victory.example.toml victory.toml      # then set your paths, outside this folder
cargo build --locked --profile diag -p victory --features diagnostics
target\diag\victory.exe shaders <your game folder>\shaders.rpf   # every shader translated, about a minute
scripts\play-highram.ps1 -Exe <your game folder>\game_scarlett_master.exe
```

`scripts\play-highram.ps1` options:

- `-HighRam` is on by default; `-HighRam:$false` leaves out `VICTORY_CARD_ALL` and
  `VICTORY_FETCH_CHECK` (for smaller cards: slower, as the parent forks ran).
- `-HalfScale` renders the world at half the Series S size (faster, blurrier). Scales other
  than 0.5 and the title's own make the title stop at startup on this build.
- `-SeriesX` uses the Series X profile (sharper, much slower).
- `-More @{VICTORY_X='1'}` passes other settings.

Run it from a normal window, not as administrator. A firewall rule that blocks
`target\diag\victory.exe` from the network is a good idea: the title tries to send crash reports.

## What is in it

This is a tree of other people's work, combined and tested on one NVIDIA laptop. The history is
kept: every commit names its author, and commits taken from elsewhere say where from.

| From | What |
|---|---|
| [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX) (the base) | Threads for the title's list calls (`VICTORY_THIN`), submissions prepared on a thread of their own, write relay, card fills, textures on a copy queue and shown only after their upload ran (`VICTORY_SHOW_AFTER`), texture read-back for the water (boats float). Its own credits: fxceless/victory-fork, DexNom, Belobug, ZJOU-VICTORY, myokex, Geckomacabre and more ([docs/README-avatarmanz-cobrez.md](docs/README-avatarmanz-cobrez.md)). |
| [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) | NVIDIA's view heap is capped at 1,000,000: more dynamic descriptor slots (`VICTORY_DYNAMIC_SLOTS`), which ended thousands of dropped draws a frame; the Series S interface plane (`VICTORY_COMPOSE`, after fxceless pull request 1189 by Tissma95); render scale, the title's own upscaler and the watchdog patches (after ZigZagBoi1/ScarlettPort); ray walks ended at the root (`VICTORY_BVH_MISS`). |
| [cobrez/Victory-Nvidia-4090-laptop](https://github.com/cobrez/Victory-Nvidia-4090-laptop) | The high-performance adapter on laptops with two GPUs, idle lists released when memory is low, and the `-Exact` pair `VICTORY_FETCH_CHECK` / `VICTORY_CARD_ALL` (relay results and title copies reach the right submission), which is what made this PC fast. |
| fxceless/victory-fork pull request 854 (PiloteCode) | Wave64 programs in a paired form on 32-lane hosts, mesh programs over 32 KiB linked with their pixel program. Here only for programs no other form takes, and only those listed in `VICTORY_PAIRED_ONLY`. |
| fxceless/victory-fork pull request 927 (tippythetaidum) | Halves moved through 16-bit buffer views keep their bits: the network that moves the faces works, so mouths move. Combined here with the base's own form for views picked from the heap. |
| Belobug/VIctory-Belobug-AMD pull request 181 | The BCPack BC7 dictionary's first entry read whole: no more coloured 4x4 blocks on clothes and faces. |
| [Loydben/VIctory-NVIDIA-HighRAM](https://github.com/Loydben/VIctory-NVIDIA-HighRAM) (the parent of this tree) | `VICTORY_F16_NEAREST` in integer arithmetic (16-bit float results rounded to nearest on NVIDIA, which rounds toward zero: no banding), quad reads in pixel programs (no lines along triangle edges), a guard for a script crash in the first boat mission (`VICTORY_SCRIPT_NULL_COPY`), NaN-exact float compares (`VICTORY_NAN_EXACT`), the paired form's group sync for one-wave groups (`VICTORY_PAIRED_SYNC`) and per-program choice (`VICTORY_PAIRED_ONLY`), the merge, the measurements and `scripts\play-highram.ps1`. |
| This tree | Settings while the game runs (`VICTORY_LIVE_FILE`, `scripts\live-menu.ps1`, F8), the frame rate cap (`VICTORY_FRAME_CAP`), the layer's own ray-tracing structure builder (`VICTORY_BVH_HOOK`, `VICTORY_BVH_NATIVE`), and changes to the host ray tracing it feeds (`VICTORY_DXR`, `VICTORY_DXR_GUIDED`): table entries set only once their views are written, structures kept inside the title's own buffers read as well. |

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not covered is named in
[docs/asset-policy.md](docs/asset-policy.md).
