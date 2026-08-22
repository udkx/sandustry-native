<!-- Русская версия: README.ru.md -->

# sandustry-native

A native simulation core for **Sandustry 0.5.2**, written in Rust. It replaces
the per-cell physics loop with the same physics compiled to native code, working
directly on the game's own shared memory.

The goal is narrow: **play without frame drops**, without rewriting the game.

> **Experimental.** This patches a game you own. It keeps a backup and undoes
> itself with one command, but treat it as a testing build, not a mod you install
> and forget.

## Measured

Same save, same camera, same session — the core switches itself on and off in
30-second phases, so both branches are measured on one scene:

| | with core | stock |
|---|---:|---:|
| tick, median | **9.9 ms** | 21.0 ms |
| tick, p95 | 15.7 ms | 27.4 ms |
| simulation updates per second | 58.9 | 46.3 |
| worker CPU load | 42 % | 82 % |

The budget for one tick is 16.67 ms. On a built-up save the stock game spends
21 ms and cannot hold 60 ups; with the core the tick fits the budget including
its tail, and workers idle half the time instead of saturating.

These come from the A/B mode shipped here, not from a synthetic benchmark — see
[Measuring it yourself](#measuring-it-yourself).

## Requirements

- **Sandustry 0.5.2** on Steam. The installer refuses other versions unless you
  pass `--force`: the patch anchors to specific places in the game's code, and if
  they moved it will not apply.
- **Node.js 18+** to run the installer.
- Windows x64, macOS (arm64 / x86_64), or Linux x64.

## Install

1. Download the core for your platform from [Releases](../../releases) and put it
   in `prebuilt/`.
2. Then:

```bash
npm install
node installer/install.js
```

The installer finds the game in your Steam library, backs up `app.asar` next to
itself, patches two files inside it, and drops the native core beside the game's
resources. If autodetection fails, point it at the game folder:

```bash
node installer/install.js --game "C:\SteamLibrary\steamapps\common\Sandustry"
```

Launch the game as usual — the core turns itself on.

### Undo

```bash
node installer/install.js --revert
```

This restores the saved copy of `app.asar` byte for byte rather than un-patching
anything. If something goes wrong before you get that far, Steam's *Properties →
Installed Files → Verify integrity* restores the game as well.

## Mods are off while the core is on

The simulation worker can only load a native module when Electron's
`nodeIntegrationInWorker` is enabled — and that flag hands Node access to
**everything running in those workers, Workshop mods included**. A mod would get
your file system.

So the two are mutually exclusive by design:

| launch | core | Workshop mods |
|---|---|---|
| normal | on | **off** |
| `--with-mods` | off | on |

To play with mods, add this to Steam → *Properties* → *Launch Options*:

```
%command% --with-mods
```

The game prints which mode it started in as the first line of its log.

## Reporting results

The bridge appends a line to `sandustry-native.log` in your system temp directory
(`%TEMP%` on Windows, `/tmp` on macOS and Linux):

```
tick 991: chunks taken 14565, deferred material/hook/structure/timer 1017/0/3656/0,
cells 9075864, moved 10305, asleep 172762 | ms: core 1047, raster 29, ...
```

If you hit a problem — odd physics, a crash, a slowdown — that file plus your
save is what makes it diagnosable. Please attach both to an issue.

## Measuring it yourself

Set a phase length in seconds, stand still, and don't move the camera:

```bash
SANDUSTRY_NATIVE_AB=30 <launch the game>
```

All simulation workers derive the phase from the wall clock, so they flip
together without exchanging a single message. The log then carries a per-phase
breakdown of where time went: core, raster, deferred cells, chunks handed back to
JS.

## Build from source

```bash
cargo build --release -p sand-node
cp target/release/libsand_node.dylib js/sand.node   # .so on Linux, .dll on Windows
codesign -s - --force js/sand.node                  # macOS only, or the game gets killed
node js/smoke.js                                    # checks the module without launching the game
cargo test -p sand-sim
```

The benchmark also wants `fixtures/busy-320.json`, a snapshot of a real world.
That file is not in the repository — it is 41 MB of the game's own world data.
Tests need nothing extra.

## How it works

- `crates/sand-sim` — the core: movement, density swaps, chunk selection, event
  queue. Knows nothing about Node or the game; pure logic over slices, so it runs
  in tests without launching anything.
- `crates/sand-node` — N-API wrapper. The game's arrays arrive as views over its
  own memory; nothing is copied in either direction.
- `crates/sand-bench` — benchmark on a snapshot of a real world.
- `installer/` — finds the game, patches `main.js` and the simulation worker,
  installs the core, and undoes all of it.

Two rules the core lives by. **It only takes a chunk it owns completely** — a
cell with a mod hook, a structure nearby, an unknown material, and the chunk goes
back to the original code. And **side effects never fire from native code**:
raster, sound, economy and mod events are queued and applied in one pass, because
a callback into JS per mutation would eat the entire win. Measured, not assumed:
one over-broad "this pair reacts" test used to send 57 % of all visited cells
back into JS and made the game with the core **26 % slower** than stock.

The unwritten rules of the game that any code moving its cells must respect are
collected in [`docs/game-contracts.md`](docs/game-contracts.md) (Russian). Each
one cost a debugging cycle in a live world.

## Known limits

- Sandustry 0.5.2, Steam edition, only.
- Workshop mods are off while the core is on (see above).
- A chunk with a machine next to it still goes back to the original code — 28 %
  of awake chunks, and 73 % of the remaining time. That is the next thing to fix,
  not a permanent property.
- The traversal margin the game shifts between threads is not reproduced, so
  behaviour at the seams between thread bands differs slightly from stock.

## Legal

This repository contains **no code or assets from Sandustry**. It ships a
separate native library and a patcher that modifies a copy of the game you
already own, on your own machine — the same thing any mod loader does.

Sandustry is © Hooded Horse / Steamforged. This project is not affiliated with or
endorsed by them. The core reimplements physics behaviour observed in the game;
if the rights holder objects to its distribution, it comes down.

Licensed under the MIT License — see [LICENSE](LICENSE). The license covers this
project's own code and nothing else.
