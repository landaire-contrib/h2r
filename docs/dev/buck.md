# Buck2 build

Status: Built. Linux builds every workspace crate with Buck2; Cargo.toml stays the source of truth.
Code: `scripts/update-buck.py`, `build-support/buck/workspace_rules.bzl`, `toolchains/host_toolchain.bzl`, `devenv.nix`, `third-party/rust/fixups/`.
Switch: `-c build_mode.mode=debug|release`, `-c build_mode.steamos=true|false`, `[h2] disc`, `[h2] opt`, `[steamos] device`.

[Buck2](https://buck2.build) builds the Rust workspace, the game module and
the SteamOS package. Cargo manifests still declare crates, features and
dependencies: `scripts/update-buck.py` turns them into BUCK files, and the
generated files are never edited by hand. Running the game is in
[steamos.md](../runtime/steamos.md).

## Setup

The [devenv](https://devenv.sh) shell (`devenv.nix`) supplies Rust from
`rust-toolchain.toml`, clang, lld, llvm-ar, the ALSA and udev development
files, 7-Zip, Reindeer, and the Steam Runtime 4 SDK and platform sysroots.
Entering it links `.buckconfig.devenv` to a generated config with the
absolute tool paths, which `.buckconfig` includes. Buck resolves no tool
through PATH. You need buck2 itself installed; `.buckconfig` uses the
prelude bundled with the binary.

```sh
devenv shell
buck2 build //crates/apps/cli:h2
buck2 test //crates/runtime/runtime:unit-tests
```

Outside devenv, `python3 scripts/configure-buck-toolchain.py` writes the
same `[host_toolchain]` section into `.buckconfig.local` from the tools on
PATH. That path has no Steam Runtime sysroot.

`.buckconfig.local` is yours and is not committed. It holds the disc path
and any of the keys below.

| Key | Meaning |
| --- | --- |
| `[h2] disc` | The Halo 2 disc: an ISO, an XISO, or a `.7z` holding one. `~` is expanded. |
| `[h2] opt` | `H2_OPT` for the game module. `h2 play` must run with the same value. |
| `[steamos] device` | The Steam Machine's host name or address. Found over mDNS when unset. |
| `[build_mode] mode` | `debug` (default, Cargo's dev profile) or `release`. |
| `[build_mode] steamos` | Compile and link against the Steam Runtime 4 sysroot, so binaries run on SteamOS and on any newer glibc. On by default when the toolchain has the sysroot (devenv). A Nix-glibc `h2` cannot load the host's Vulkan driver. |

## Targets

Each workspace member's directory has a generated `BUCK` file:

- the library, named after the package (`//crates/runtime/kernel:kernel`)
- each binary, named after the binary (`//crates/apps/cli:h2`), with `-bin` added when it would clash with the library
- `unit-tests` for the library's `#[cfg(test)]` code and `<bin>-unit-tests` for each binary
- one test per integration test file, named after the file (`-test` added on a clash)
- `build-script` when the crate has a `build.rs`

Examples and benches have no targets. A binary or test whose
`required-features` the unified feature set lacks is left out.

Do not build `//...`. It includes every third-party crate for every
platform, and most of them cannot build on Linux.

## How the rules are made

`scripts/update-buck.py` reads `cargo metadata` and writes two things.

**Third party.** `third-party/rust/Cargo.toml` depends on every external
package a workspace member uses, at the exact resolved version and with the
features Cargo's resolver unified. Reindeer reads it with a copy of
`Cargo.lock` and writes `third-party/rust/BUCK`. Crates are not vendored:
each is an `http_archive` from crates.io pinned by SHA-256. A package used
at two versions gets a version suffix (`//third-party/rust:sha2-0_10`). The
script fails if Reindeer resolves a version the workspace lock does not
contain.

**First party.** Buck has no feature unification, so each crate gets the
features Cargo enables for it in a whole-workspace build.
`[target.'cfg(..)'.dependencies]` become `select()` branches for Linux,
macOS and Windows, evaluated against `rustc --print cfg`. A renamed
dependency becomes a `named_deps` entry. `[profile.dev.package.<crate>]`
opt-levels carry over to debug builds.

Each target compiles from a crate tree (`crate_tree` in
`workspace_rules.bzl`): a directory of symlinked files at their repository
paths. It holds the crate, plus every package its sources reach through a
`../` string literal that names an existing path: `include_str!`,
`#[path]`, and the source trees that `recomp` and `game` hash in their build
scripts. Directories are real, so `../` resolves the way it does in a
checkout. Files outside `crates/` (test fixtures under `docs/` or `web/`,
`Cargo.lock`) enter a tree one by one, and only if git does not ignore them
(`build-support/buck/root_exports.bzl`). Game data never enters a tree:
`disc/` is excluded, and so are ignored captures such as the README frames.

First-party build scripts run in the same tree
(`build-support/buck/run_buildscript.py`), with `CARGO_MANIFEST_DIR` and the
working directory set to the crate's directory in it and `CARGO_CFG_*` from
`rustc --print cfg`. Only declared values reach a script: the C toolchain,
`PATH` and Cargo's variables. Their `rustc-cfg`, `rustc-env`,
`rustc-link-lib` and `rustc-link-search` lines reach rustc. An unknown
`cargo:` directive is an error, and so is a `rerun-if-changed` path missing
from the tree, which means the tree is stale. `app`'s build script finds no
git checkout from the tree and records the build as `unknown`.

## After changing Cargo.toml or Cargo.lock

```sh
devenv shell
python3 scripts/update-buck.py
```

Commit the regenerated `BUCK` files, `third-party/rust/` and any new fixup
with the Cargo change. `--first-party` skips Reindeer when only first-party
manifests or sources changed (a new `include_str!` of another crate counts).
`--check` regenerates, lists the files that differ, puts the committed ones
back and exits 1 when anything is stale.

A third-party crate with a build script needs
`third-party/rust/fixups/<crate>/fixups.toml`; Reindeer warns about each one
that has none. Prefer `buildscript.run = true`. A script that compiles C
and links it also needs `rustc_link_lib` and `rustc_link_search` under
`[buildscript.run]` (`ring`, `zstd-sys`). Use `buildscript.run = false` when
the script probes the host (pkg-config, test links) and declare its result
instead, as `alsa-sys` and `libudev-sys` do.

## Differences from Cargo

- Tests run from the repository root, not the crate directory. A test that opens a path relative to the working directory fails; one that joins `CARGO_MANIFEST_DIR` works.
- Doctests are not run.
- Third-party crates build at opt-level 3 in both modes. Cargo's dev profile builds most of them at opt-level 0.
- Tools are named by path, not by content. Inside devenv every path is a Nix store path, which changes with the tool, so a shared action cache is sound. With `configure-buck-toolchain.py` (`/usr/bin/clang` and the like) it is not: an upgraded tool keeps its action keys.
- Integration tests see `CARGO_TARGET_TMPDIR` as `/tmp`, shared by every run.

## Not verified

- macOS and Windows hosts. The toolchain rules come from a project that builds on both, but this tree has only been built on x86-64 Linux.
- Every integration test under Buck. Tests that read fixtures through relative paths fail until they use `CARGO_MANIFEST_DIR`.

## Related

- [steamos.md](../runtime/steamos.md): playing on Linux and on a Steam Machine
- [linux.md](../runtime/linux.md): the Linux game module and desktop host
- [build-speed.md](build-speed.md): Cargo build speed
