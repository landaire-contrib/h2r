<p align="center">
  <img src="docs/images/readme/header.webp" alt="A panorama of Old Mombasa, Delta Approach and Ascension" width="100%">
</p>

# h2recomp

Halo 2, the original Xbox one, recompiled to native code and running on a Mac, an Apple TV and an iPhone. Bring your own disc.

The retail executable comes off your own disc, all 18,700-odd functions of it get lifted to C, and clang builds the lot into a native library. A Rust runtime runs that library in place of the Xbox, with the kernel, Direct3D and DirectSound swapped out underneath it and a native GPU renderer drawing the frame.

<p align="center">
  <img src="docs/images/readme/oldmombasa-4x.jpg" alt="Old Mombasa at 4x render scale" width="32%">
  <img src="docs/images/readme/deltaapproach-4x.jpg" alt="Delta Approach at 4x render scale" width="32%">
  <img src="docs/images/readme/ascension-4x.jpg" alt="Ascension at 4x render scale" width="32%">
</p>

You'll need your own legally obtained copy of the game. The documentation images are rendered from my disc by scripts and kept in Git LFS. [How the README media is made](CONTRIBUTING.md#readme-media).

h2recomp was created under Microsoft's "[Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft. Halo © Microsoft Corporation. This is a free, non-commercial fan project.

## Play it

**1. Get your ISO.** Put your Halo 2 disc image in `disc/` (it's git-ignored). XISO and full Redump images both work, and `cp -c` clones it on macOS without using any extra space. Got the Multiplayer Map Pack disc, or Halo 1? Drop them in beside it as `mappack.iso` and `halo1.iso` and they're imported on the first run.

```sh
mkdir -p disc
cp -c ~/Downloads/your.iso disc/halo2.iso
```

**2. Get the CLI.** You need Rust (with rustup, `rust-toolchain.toml` picks the right version) and clang on your `PATH`.

```sh
git clone https://github.com/0xdeafcafe/h2r && cd h2r
cargo install --locked --path crates/apps/cli
```

**3. Run it.**

```sh
h2 play disc/halo2.iso                          # window, sound, keyboard, mouse and pads
h2 play disc/halo2.iso --live                   # signed in to live.forbes.red
h2 play disc/halo2.iso --level 03a_oldmombasa   # straight into a level, no menus
```

The first run builds the game library into `$TMPDIR/h2boot`, which is slow. Every run after that reuses it. WASD and the mouse move and look, Enter is A, Escape is Start and Cmd+F goes fullscreen. `--follow` keeps you on the latest master, building each new commit in the background. Every flag is in [docs/runtime/app.md](docs/runtime/app.md).

On macOS, run `just mac-install iso=disc/halo2.iso` after installing the CLI
to open **Join in Halo 2** links from the Live website. It installs
`~/Applications/Halo 2.app` and remembers where your disc is. Run the
recipe again after updating the CLI or moving the disc.

## How fast

I'm targeting uncapped frame rates, and it currently runs a steady 30/60/120 fps (the measured numbers are in [docs/features.md](docs/features.md#speed)).

## What I've added

All the things the retail game never had. The full list, with what's only partly done, is in [docs/features.md](docs/features.md).

- **Native GPU renderer**: the whole frame drawn from data kept on the GPU, with a render scale up to 6x. [Renderer](docs/graphics/native-renderer.md) · [render scale](docs/graphics/high-resolution.md)
- **60 and 120 fps** over the game's 30 Hz tick. [High frame rate](docs/game/high-framerate.md)
- **Halo 1 in the Halo 2 engine**: all 13 multiplayer maps, and the campaign levels converted from your own Halo 1 disc, with Halo 1's health model (the campaign converts and boots, nobody has played one through yet). [Campaign](docs/halo1/campaign.md) · [multiplayer](docs/halo1/multiplayer.md)
- **Multiplayer Map Pack** installed from your own map pack disc. [Map pack](docs/game/map-pack.md)
- **13 community game variants** built in, Zombies and SWAT included. [Variants](docs/game/community-gametypes.md)
- **Skulls** per profile, with a Skulls page and a few of our own. [Skulls](docs/game/skulls.md)
- **Clankers**, bots that join hosted matches as phantom players, so every controller port stays free for a person. [Bots](docs/game/bots.md)
- **Theater**: films recorded by dedicated hosts, played back with the game's own renderer and director cameras (first slice). [Theater](docs/game/theater.md)
- **Settings screens** for video, controls, FOV and voice, built as real game screens. [Settings](docs/game/settings-menu.md)
- **Apple TV and iPhone/iPad** apps, with touch controls on iOS (the Apple TV one is still slow). [iOS](docs/runtime/ios.md) · [tvOS](docs/runtime/tvos.md)
- **Co-op campaign over Live**, where retail only did split screen on one console. [Co-op](docs/live/live-coop.md)
- **Cloud saves**: sign in somewhere else and your profiles, campaign progress and variants are already there. [Cloud saves](docs/live/live-cloud-saves.md)

## Live, and the stats site

Xbox Live for the original Xbox shut down in 2010, so I wrote a new one. It runs at **[live.forbes.red](https://live.forbes.red)**, and `h2 play --live` signs you in to it.

It does accounts with a gamertag, friends, Teams, invites, the message of the day, matchmaking off served playlists (Ranked included), the custom games browser, co-op and cloud saves. Dedicated hosts for matches are in progress.

There's a web app beside it: a dashboard in the style of the original Xbox one, and a stats site in the style of Bungie.net's Halo 2 and 3 stats pages. It shows who's online, games as they happen on a live top-down map with the scoreboard and kill feed, replays of finished games and the ranks.

You can run your own too: `h2 live serve` on a Mac, Docker behind a Cloudflare Tunnel, or Cloudflare Workers. [Live](docs/live/live.md) · [hosting](docs/live/live-hosting.md) · [Workers](docs/live/live-worker.md) · [web app](web/README.md)

## The editor and the agent API

`h2-editor` is a native map and variant editor, in the spirit of the original tools' Sapien. Open a map from your disc, move things around it in 3D, edit any field of any tag, make game variants and edit the 16 matchmaking playlists. It reads Halo 1 maps too. Start with the [editor guide](docs/editor/editor-guide.md).

```sh
cargo run --release -p studio -- --disc disc/halo2.iso
```

The agent API is how a script, a test or a coding agent reads and drives the running game without squinting at screenshots. `h2 serve` runs the game with a JSON-RPC socket, and `h2 agent` asks it what's in the level, where the frame time goes and what the menus show, presses buttons, waits for things to happen, and saves and restores state. [Agent API](docs/runtime/agent-api.md) · [the editor's](docs/editor/agent-api.md)

```sh
h2 agent status
h2 agent query '{"kind":["actor"],"near":{"player":0,"radius":30},"fields":["pos","team"]}'
```

## How it works

The XBE is analysed, every function is lifted from x86 to an IR and emitted as C11, and clang builds that into a library. The generated code is never edited by hand: it's rebuilt from your disc. The Rust runtime underneath replaces the Xbox kernel, XAPI, Direct3D 8 and DirectSound, on a deterministic logical clock, and renders through wgpu on Metal. The native renderer builds its scene lists in Rust and skips the game's own draw code, falling back to the D3D path for anything it can't take yet. Bit by bit, the hot parts of the game are being rewritten as Rust that overrides the lifted function at the same address. [ARCHITECTURE.md](docs/ARCHITECTURE.md) has the long version.

<p align="center">
  <img src="docs/images/readme/render-pipeline.svg" alt="Render pipeline before and after: guest front-end, D3D translate, two copies; then native scene lists and one native render" width="100%">
</p>

## Building from source

The root `justfile` builds every target from your disc (`brew install just`):

```sh
just doctor          # what each target needs and what's missing
just mac-play        # build and play on this Mac
just tvos            # the Apple TV app; then just tvos-install
just ios maps=all    # the iPhone/iPad app with every map and the map pack
just check           # fmt, clippy and tests, as before a push
```

`cargo h2 <command>` runs the CLI straight from the checkout. Every recipe, the full command list and Ghidra are in [docs/runtime/building.md](docs/runtime/building.md), and [CONTRIBUTING.md](CONTRIBUTING.md) has the rules (the main one: no game data, ever).

On Linux, [Buck2](https://buck2.build) builds everything inside a [devenv](https://devenv.sh) shell. Tell it where your disc is in `.buckconfig.local` (an ISO or a `.7z` holding one):

```ini
[h2]
  disc = ~/Downloads/halo2.7z
```

Then `devenv shell` and `buck2 run //platform/linux:play`. A Steam Machine or Steam Deck in Developer Mode gets the game with `buck2 run //platform/steamos:run`. [docs/runtime/steamos.md](docs/runtime/steamos.md) has the details.

## Docs

[docs/README.md](docs/README.md) is the index, every doc grouped by area. Good places to start:

- [ARCHITECTURE.md](docs/ARCHITECTURE.md): the design, end to end
- [features.md](docs/features.md): every feature with its state, the speed numbers and the roadmap
- [LIFTER.md](docs/lifter/LIFTER.md): x86 to IR to C11
- [native-renderer.md](docs/graphics/native-renderer.md): the GPU renderer
- [live.md](docs/live/live.md): the Xbox Live replacement
- [editor-guide.md](docs/editor/editor-guide.md) and [agent-api.md](docs/runtime/agent-api.md): the tools
