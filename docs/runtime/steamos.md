# Playing on Linux and on a Steam Machine

Status: Built. Buck builds the game module from your disc and runs it here, or copies it to a SteamOS device in Developer Mode and launches it there. It has booted to the main menu on a Steam Machine ([notes/steamos-boot.md](notes/steamos-boot.md)); a Steam Deck has not been tried.
Code: `platform/linux/BUCK`, `platform/linux/extract-disc.sh`, `platform/steamos/BUCK`, `platform/steamos/run.py`, `platform/steamos/launch.sh`, `platform/steamos/fetch-devkit-utils.py`.
Switch: `[h2] disc`, `[h2] opt`, `[steamos] device`, `[build_mode] steamos`, see [buck.md](../dev/buck.md).

Everything here runs inside the devenv shell, which the
[Buck2 build](../dev/buck.md) describes. The device has no compiler, so
the clang module is built on your machine and shipped with `h2`. The disc
is read from wherever `.buckconfig.local` says. It never leaves your
machines, and no rule that reads it is cached remotely.

## Your disc

```ini
[h2]
  disc = ~/Downloads/halo2.7z
```

An ISO, an XISO, or a `.7z` holding one all work. `//platform/linux:disc`
extracts or copies it once into `buck-out`. Buck does not see a change
to the file at the same path. Run `buck2 clean` or change the path if you
replace it.

## Play here

```sh
devenv shell
buck2 run //platform/linux:play
buck2 run //platform/linux:play -- --level 03a_oldmombasa
```

`//platform/linux:module` runs `h2 build <disc> --tier clang` with the
devenv clang, ld.lld and llvm-ar. Objects are kept in
`~/.cache/h2recomp/obj` (`H2_OBJ_CACHE`), so a rebuild after a Rust change
only recompiles the shards whose C changed. `play` runs `h2 play` on the
module in `buck-out`, which it reads without writing. A module that is
stale for the running `h2` (another `H2_DEBUG`, `H2_PASSES` or `H2_OPT` in
your environment) fails to rebuild there; unset them. Arguments after `--`
go to `h2 play`.

## Play on a Steam Machine

Put the device in Developer Mode and turn on the devkit service, so it
advertises itself on the network. Install Steam Linux Runtime 4.0 on it
once, from Desktop Mode if Steam doesn't list it:

```sh
steam steam://install/4183110
```

Then, on your machine:

```sh
buck2 run -c build_mode.mode=release //platform/steamos:run
```

The first run asks you to approve a pairing request on the device's screen.
It uses the key Valve's devkit client uses (`~/.config/steamos-devkit/devkit_rsa`),
so a device you paired with that client is already paired. Then it:

1. builds `h2` and the module against the Steam Runtime 4 sysroot and checks them against the runtime's libraries (`//platform/steamos:package`)
2. copies Valve's devkit-utils scripts, the package and the disc into `~/devkit-game/h2r` on the device, sending only what changed
3. registers the game as a Steam shortcut that runs `launch.sh` in the Steam Linux Runtime 4 container, and starts it

The device is found over mDNS (`avahi-browse`). Set `[steamos] device` to its
host name or address to skip that. After `--`, `--no-launch` copies and
registers without starting, and `--env KEY=VALUE` sets a variable for the
game; every other argument goes to `h2 play`. Steam does not pass a
shortcut's environment into the runtime container, so the variables go in
`h2-env` beside `launch.sh`, which exports them. A running copy of the game
is stopped before the new one is copied. The shortcut stays in the library,
so you can start the game from the device later.

To record a run you can replay tick for tick on the same build:

```sh
buck2 run -c build_mode.mode=release //platform/steamos:run -- \
  --env H2_INPUT_RECORD=/home/deck/h2r-recordings/run.input --level 01b_spacestation
```

The log is written when the game exits. Replay it with `H2_INPUT_REPLAY` and
the same package and `--level`.

## Rules and limits

- `[h2] opt` sets `H2_OPT` for the module build, and `launch.sh` and `play` export the same value. `h2 play` rebuilds a module whose stamp disagrees, and the device has no compiler to do it with.
- Inside devenv every binary links against the Steam Runtime 4 sysroot (`[build_mode] steamos`, on by default there), so the `h2` that plays here is the one the device gets. A binary linked against Nix's glibc cannot load the host's Vulkan driver.
- Intro movies are skipped on Linux; [linux.md](linux.md) has what the desktop host leaves out.

## Not verified

- A Steam Deck. The flow is the same SteamOS devkit service, but it has only run on a Steam Machine.
- A whole level played through on the device.

## Related

- [buck.md](../dev/buck.md): the Buck build and `.buckconfig.local`
- [linux.md](linux.md): the Linux game module and desktop host
- [app.md](app.md): `h2 play` flags
