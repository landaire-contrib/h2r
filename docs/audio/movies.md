# Movies

Status: Built. Code: `crates/formats/movies/src` (installer), `crates/platform/bink-hle/src`, `crates/extensions/game/src/movies_glue.rs`, `crates/apps/cli/src/movies_cmd.rs` (`h2 movies`). Tests: see "Tests".
Switch: `--movies`, `--no-movies` (command-line flags).

The retail disc contains three Bink 1 movies: `/bink/intro_60.bik`,
`/bink/attract_60.bik`, and `/bink/credits_60.bik`. They are 640x480,
30000/1001 fps, with 44.1 kHz stereo audio. Discovery walks the entire
XDVDFS tree, including nested directories, rather than hard-coding these
names. Campaign scenes rendered by the engine are separate from these files.

## Installation contract

Movie conversion is optional. Run the user's own ffmpeg as a separate
process at install time. Do not bundle ffmpeg, link its libraries, copy a
Bink decoder, or commit disc media, converted output, or XBE bytes.
The installer opens the ISO read-only using `xiso::Xiso`.

`movies::install` is the reusable app installer API:

```rust,no_run
use movies::install::{app_movies_dir, install_iso, InstallOptions, InstallStatus};
use std::path::Path;

let mut options = InstallOptions::new(app_movies_dir()?);
options.ffmpeg = Some("/opt/homebrew/bin/ffmpeg".into()); // optional; else H2_FFMPEG, PATH, then /opt/homebrew/bin, /usr/local/bin, /opt/local/bin
let report = install_iso(Path::new("/Volumes/OwnDisc/halo2.iso"), &options)?;
match report.status {
    InstallStatus::Complete => { /* report.converted and report.reused */ }
    InstallStatus::FfmpegAbsent => { /* continue without movies; offer retry */ }
    InstallStatus::FfmpegUnusable(reason) => { /* show reason; offer retry */ }
}
# Ok::<(), std::io::Error>(())
```

The default destination is
`~/Library/Application Support/h2recomp/movies` on Apple hosts,
`%LOCALAPPDATA%\h2recomp\movies` on Windows and
`$XDG_DATA_HOME/h2recomp/movies` (`~/.local/share/h2recomp/movies`) on other Unix (`movies::install::app_movies_dir`). A sandboxed app may provide
its own container data directory through `InstallOptions::new`. Destinations
inside a Git checkout or worktree, including symlink aliases, are rejected.
Nothing writes into the source ISO or repository. The synchronous installer
belongs on the installer's worker thread, before launching the game.

`install_region` takes a named movie's file extent and runs the same
conversion and publication path for that movie alone. The windowed boot
uses it on a background worker for an unprepared attract clip after
resolving the disc path through VFS. Other movies in the installation
remain available. [Attract timer](bink-hle.md#attract-timer) describes how
the menu handles preparation and fallback.

Before creating the destination, ffmpeg is checked with `-version`,
`-decoders`, and `-encoders`. It needs `binkvideo`, both Bink audio decoders,
AAC, and one supported video encoder. An absent or unsuitable ffmpeg returns
an explicit optional status and leaves existing movies alone. Conversion or
I/O failure returns `io::Error`, including ffmpeg's bounded diagnostic log.
It is appropriate to continue app installation with a message explaining
that movies are unavailable and can be installed later. Missing movies are
skippable only once the host opts into the existing skip/HLE path; this crate
does not register overrides or change boot behavior.

## Output and staging

The output is **H.264 or HEVC video plus AAC in `.mov`**, decoded by
AVFoundation in `bink-hle`. Preferred encoding is `libx264` with CRF 18,
slow preset, High profile, and `yuv420p`. Fallbacks are
`h264_videotoolbox`, then `hevc_videotoolbox`, at 4 Mbit/s; HEVC uses the
`hvc1` sample tag. Audio uses AAC at 192 kbit/s. The first video track and
optional first audio track are selected; subtitles and data are excluded.
Dimensions and frame rate are preserved. `+faststart` puts the MOV index
first. See the [ffmpeg container documentation](https://ffmpeg.org/ffmpeg-formats.html#mov_002c-mp4_002c-ismv).

Source size and SHA-256 are calculated through `Xiso::copy_to`, with short
reads rejected. A movie needing conversion is staged to a seekable input
file in the system temporary directory (`TMPDIR`): Bink has indexed frames,
and ffmpeg may seek. The staged bytes are
hashed again to detect source changes between the two reads. ffmpeg's
output is also seekable. Only completed output and the manifest are staged
in the app data directory for atomic publication across volumes. Staging
directories are private (0700 on Unix),
and Rust cleanup removes them on normal return, error, or unwinding.
The ffmpeg child is killed and reaped on unwinding; stdout and stderr are
drained independently to avoid a full pipe deadlocking the installer.

A hard kill or power loss can leave a staging directory or an unreferenced
completed output. These are never considered installed. Restarting the
installer retries that movie; it does not resume an individual encode at a
frame boundary. Interrupted staging directories can be removed when no
installer is running. Old unreferenced outputs are retained rather than
risking deleting a movie another process is playing.

## Manifest and resume

`movies.json` retains the version-1 `MovieEntry` fields for compatibility
with the existing CLI: source path, size and SHA-256; output name and size;
duration; ffmpeg version; exact encoding settings. An additive
`output_sha256` map records output hashes by file name. Legacy manifests
without that map still load. When the installer publishes the first hashed
output, it also hashes each remaining readable legacy output with the recorded
size. These hashes record the files as they exist at migration, so installing
only the attract clip keeps other valid legacy movies available. An
`output_stamp` map records each output's size and mtime at the moment its
hash was verified, so `locate` does not re-hash a multi-megabyte movie on
every call; a changed stamp falls back to a full hash. Once a manifest has
any output hashes, an entry without one is not trusted. After a successful
run the installer deletes `movie-*.mov` files the manifest no longer
references, and stale `.install-*` scratch directories, under its lock.

For each movie, reuse requires matching source size/hash, matching encoder
settings, a regular output file with the recorded size, and matching output
SHA-256. A changed ffmpeg version alone does not force conversion; the
version used remains recorded. Same-size corruption is detected. Other
entries in an existing manifest survive, and an unreadable or incompatible
manifest is an error rather than permission to silently replace it.

Outputs are named `movie-<output-sha256>.mov`. Publish the complete file
before atomically replacing the manifest, and sync both. A failed conversion
cannot overwrite a file referenced by the previous manifest. Each successful
movie commits independently, so a later failure does not discard earlier
work. An advisory OS lock serializes installers. Its `.install.lock` file
remains in place, while the lock itself is released on exit or crash; a stale
lock file does not prevent resume.

Lookup remains:

```rust
if let Some(path) = movies::locate(movies_dir, "intro_60") {
    // Open through the platform MovieDecoder.
}
```

`locate` checks file name safety, regular-file status, size, and output hash
when present. Legacy CLI entries retain their size-only lookup behavior.
`locate_fresh` additionally checks a supplied disc source size/hash.
Manifest names cannot escape the movie directory through separators,
absolute paths, or symlink output files. A missing, stale, corrupt, or
unreadable movie resolves to `None`.

## CLI

`h2 movies [<iso>] [--out <dir>] [--ffmpeg PATH] [--force] [--only NAME]` is the
standalone exporter (`crates/apps/cli/src/movies_cmd.rs`). `--out` defaults to the app data
directory (`movies::install::app_movies_dir()`), which is where `h2 play`
looks (`--movies DIR` elsewhere, which headless `h2 run` needs to play them); see
[bink-hle.md](bink-hle.md) for how a run decides to play them. It writes H.264/AAC MP4,
requires `--force` to replace existing output, and does not use the
resumable installer in `crates/formats/movies/src/install.rs`. Its exit codes are 0 success, 1 failure, 2 bad
arguments, 3 ffmpeg missing, 4 unsuitable ffmpeg, and 130 interrupted.
An app installer calls `movies::install::install_iso`. MP4s are playable through
`TranscodedSource` (`Mp4Source` is a compatibility alias).

## Tests

`cargo test -p movies -p bink-hle` builds media with the user's ffmpeg at test
time. The media tests get `#[ignore]` when ffmpeg is absent on PATH at build
time; the manifest, discovery, missing-tool, and synthetic-state tests still
run. Changing PATH causes build scripts to recheck availability. Tests use
only generated NUT input, stored under `.bik` names inside a synthetic
XDVDFS image to exercise the real discovery/copy/transcode path. No Bink
encoder or retail fixture is needed.

Coverage includes per-movie resume, source changes, same-size output
corruption, failed second movie preserving the first, invalid manifest,
case-insensitive discovery, MOV playback through AVFoundation, exact frame
counts and rational timing, BGRA channel order, rewinds, and failed frame
requests retaining the previous picture. The native decoder tests need
access to macOS's media decoder service; a restrictive process sandbox can
block that service. The app's entitlements and real boot remain integration
work.
