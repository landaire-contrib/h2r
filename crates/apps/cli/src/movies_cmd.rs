//! `h2 movies`: convert the disc's Bink movies with the user's own ffmpeg.
//!
//! ffmpeg is only ever run as an external process; nothing here decodes Bink
//! or links ffmpeg. See docs/audio/movies.md for the rationale and the settings.
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use movies::{MANIFEST_NAME, Manifest, MovieEntry, Sha256};

use crate::safe_out::OutDir;

pub const EXIT_OK: i32 = 0;
/// A movie failed, or the disc/output could not be used.
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
/// No ffmpeg found. Movies are optional; the game skips them.
pub const EXIT_NO_FFMPEG: i32 = 3;
/// ffmpeg exists but cannot decode Bink or lacks the H.264/AAC encoders.
pub const EXIT_FFMPEG_UNUSABLE: i32 = 4;
pub const EXIT_INTERRUPTED: i32 = 130;

const USAGE: &str = "usage: h2 movies [<iso>] [--out DIR] [--ffmpeg PATH] [--force] [--only NAME]

Converts the Bink (.bik) movies on the disc to H.264/AAC MP4 using your own
ffmpeg, which must include the Bink decoder. The movies are optional: without
them the game simply skips them. --out defaults to the app data directory
(~/Library/Application Support/h2recomp/movies, %LOCALAPPDATA%\\h2recomp\\movies
on Windows, ~/.local/share/h2recomp/movies elsewhere), where `h2 play` looks for
them; --movies DIR names another, and makes headless `h2 run` play them.

exit codes: 0 all done, 1 a movie failed, 2 bad arguments,
            3 ffmpeg not found (movies are optional), 4 ffmpeg cannot be used,
            130 interrupted";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub iso: Option<PathBuf>,
    pub out: PathBuf,
    pub ffmpeg: Option<PathBuf>,
    pub force: bool,
    /// Convert only movies whose file name contains this (case-insensitive).
    pub only: Option<String>,
}

pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut iso, mut out, mut ffmpeg, mut force, mut only) = (None, None, None, false, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a directory")?)),
            "--ffmpeg" => ffmpeg = Some(PathBuf::from(it.next().ok_or("--ffmpeg needs a path")?)),
            "--force" => force = true,
            "--only" => only = Some(it.next().ok_or("--only needs a name")?.to_ascii_lowercase()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if iso.is_none() => iso = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    let out = match out {
        Some(o) => o,
        None => movies::install::app_movies_dir().map_err(|e| format!("--out DIR is required ({e})"))?,
    };
    Ok(Args { iso, out, ffmpeg, force, only })
}

// ---------------------------------------------------------------- ffmpeg

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub path: PathBuf,
    /// e.g. `ffmpeg version 7.1.1`
    pub version: String,
    pub video: VideoEncoder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    X264,
    /// Hardware encoder on macOS builds without libx264.
    VideoToolbox,
}

#[derive(Debug)]
pub enum DiscoverError {
    Missing(String),
    Unusable(String),
}

fn run_text(path: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", path.display()))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Does an `ffmpeg -decoders`/`-encoders` listing contain `name`?
fn lists(listing: &str, name: &str) -> bool {
    listing.lines().any(|l| l.split_whitespace().nth(1) == Some(name))
}

fn is_executable(p: &Path) -> bool {
    let Ok(m) = fs::metadata(p) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.is_file() && m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        m.is_file()
    }
}

/// Locate and vet ffmpeg: `explicit` if given, else the first `ffmpeg` on
/// `path_env`. Checks `-version`, the Bink decoders and the H.264/AAC encoders.
pub fn discover(explicit: Option<&Path>, path_env: Option<OsString>) -> Result<Ffmpeg, DiscoverError> {
    let path = match explicit {
        Some(p) if is_executable(p) => p.to_path_buf(),
        Some(p) => {
            return Err(DiscoverError::Missing(format!("{} is not an executable file", p.display())));
        }
        None => {
            let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
            path_env
                .iter()
                .flat_map(std::env::split_paths)
                .map(|d| d.join(name))
                .find(|p| is_executable(p))
                .ok_or_else(|| DiscoverError::Missing("ffmpeg was not found on PATH".into()))?
        }
    };
    let unusable = |m: String| DiscoverError::Unusable(format!("{}: {m}", path.display()));
    let ver = run_text(&path, &["-version"]).map_err(|e| unusable(format!("{e:#}")))?;
    let first = ver.lines().next().unwrap_or("");
    let version = first.split(" Copyright").next().unwrap_or(first).trim().to_string();
    if !version.starts_with("ffmpeg version") {
        return Err(unusable("does not look like ffmpeg (`-version` printed something else)".into()));
    }
    let dec = run_text(&path, &["-hide_banner", "-decoders"]).map_err(|e| unusable(format!("{e:#}")))?;
    if !lists(&dec, "binkvideo") || !(lists(&dec, "binkaudio_dct") || lists(&dec, "binkaudio_rdft")) {
        return Err(unusable("this ffmpeg build has no Bink video/audio decoder".into()));
    }
    let enc = run_text(&path, &["-hide_banner", "-encoders"]).map_err(|e| unusable(format!("{e:#}")))?;
    let video = if lists(&enc, "libx264") {
        VideoEncoder::X264
    } else if lists(&enc, "h264_videotoolbox") {
        VideoEncoder::VideoToolbox
    } else {
        return Err(unusable("this ffmpeg build has no H.264 encoder (libx264)".into()));
    };
    if !lists(&enc, "aac") {
        return Err(unusable("this ffmpeg build has no AAC encoder".into()));
    }
    Ok(Ffmpeg { path, version, video })
}

impl Ffmpeg {
    pub fn settings(&self) -> &'static str {
        match self.video {
            VideoEncoder::X264 => "libx264 crf18 slow high yuv420p + aac 192k, mp4 faststart",
            VideoEncoder::VideoToolbox => "h264_videotoolbox 4M high yuv420p + aac 192k, mp4 faststart",
        }
    }

    fn encode_args(&self, out: &Path) -> Vec<OsString> {
        let mut a: Vec<OsString> = ["-hide_banner", "-loglevel", "error", "-nostats", "-f", "bink", "-i", "pipe:0"]
            .iter()
            .map(Into::into)
            .collect();
        let rest: &[&str] = match self.video {
            VideoEncoder::X264 => &["-c:v", "libx264", "-preset", "slow", "-crf", "18", "-profile:v", "high"],
            VideoEncoder::VideoToolbox => &["-c:v", "h264_videotoolbox", "-b:v", "4M", "-profile:v", "high"],
        };
        a.extend(rest.iter().map(OsString::from));
        a.extend(
            ["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn"]
                .iter()
                .map(OsString::from),
        );
        a.extend(["-movflags", "+faststart", "-f", "mp4"].iter().map(OsString::from));
        a.push(out.into());
        a
    }
}

/// Run ffmpeg to encode what `feed` writes into its stdin, producing `out`
/// (which must not exist; mp4 `faststart` needs a seekable file, so ffmpeg
/// writes a scratch file rather than a pipe). `feed` returns bytes written.
pub fn transcode(ff: &Ffmpeg, out: &Path, feed: impl FnOnce(&mut dyn Write) -> io::Result<u64>) -> Result<()> {
    let mut child = ChildGuard(
        Command::new(&ff.path)
            .args(ff.encode_args(out))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {}", ff.path.display()))?,
    );
    let mut stderr = child.0.stderr.take().unwrap();
    let log = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = (&mut stderr).take(64 * 1024).read_to_string(&mut s);
        let _ = io::copy(&mut stderr, &mut io::sink());
        s
    });
    let mut stdin = child.0.stdin.take().unwrap();
    // Ordering matters. Keep `feed`'s result instead of `?`-ing it: when
    // ffmpeg dies early, the write fails with a bare "broken pipe" and the
    // real reason is only in ffmpeg's stderr and exit status, so those are
    // reported first. stdin must be closed before wait() (EOF is what lets
    // ffmpeg finish), and wait() must always run so no zombie is left; if
    // `feed` panics, `ChildGuard` kills and reaps the child while unwinding.
    let fed = feed(&mut stdin);
    drop(stdin);
    let status = child.0.wait()?;
    let log = log.join().unwrap_or_default();
    let tail = || log.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    if !status.success() {
        bail!("ffmpeg failed ({status}):\n{}", tail());
    }
    match fed {
        // ffmpeg may stop reading early (it has enough); only a real read error matters.
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(anyhow!(e).context("streaming source into ffmpeg")),
        _ => Ok(()),
    }
}

/// Kills and reaps the child if dropped before a normal `wait` (panic).
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Parse `Duration: 00:01:23.45,` from `ffmpeg -i` output.
pub fn parse_duration(text: &str) -> Option<f64> {
    let rest = text.split("Duration:").nth(1)?.trim_start();
    let t = rest.split(',').next()?.trim();
    let mut p = t.split(':');
    let (h, m, s): (f64, f64, f64) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    Some(h * 3600.0 + m * 60.0 + s)
}

fn probe_duration(ff: &Ffmpeg, file: &Path) -> Option<f64> {
    let out = Command::new(&ff.path).args(["-hide_banner", "-i"]).arg(file).stdin(Stdio::null()).output().ok()?;
    parse_duration(&String::from_utf8_lossy(&out.stderr))
}

// ------------------------------------------------------------------- run

/// Tee: forwards to `inner`, hashes and reports progress.
struct Tee<'a, W: Write> {
    inner: W,
    sha: Sha256,
    done: u64,
    total: u64,
    next_report: u64,
    name: &'a str,
    cancel: &'a AtomicBool,
}

impl<W: Write> Write for Tee<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            // Not ErrorKind::Interrupted: io::copy retries that forever.
            return Err(io::Error::other("interrupted"));
        }
        let n = self.inner.write(buf)?;
        self.sha.update(&buf[..n]);
        self.done += n as u64;
        let pct = self.done * 100 / self.total.max(1);
        while pct >= self.next_report && self.next_report <= 100 {
            eprintln!("  {}: {}% of source read", self.name, self.next_report);
            self.next_report += 10;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> io::Result<Self> {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let p = std::env::temp_dir().join(format!("h2-movies-{}-{t}", std::process::id()));
        // Exclusive (fails if something sits there) and private: the scratch
        // holds a whole converted movie.
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut b = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&p)?;
        Ok(Self(p))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Set by SIGINT/SIGTERM; checked while streaming so the run unwinds and
/// the scratch directory (up to a few hundred MB) is removed.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn install_signal_handlers() {
    extern "C" fn on_signal(_: i32) {
        INTERRUPTED.store(true, Ordering::Relaxed); // async-signal-safe
    }
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    // SIGINT = 2, SIGTERM = 15 on every unix we target. No SA_RESTART needed:
    // the flag is polled between writes.
    for sig in [2, 15] {
        unsafe { signal(sig, on_signal as extern "C" fn(i32) as usize) };
    }
}
#[cfg(not(unix))]
fn install_signal_handlers() {}

pub fn run(args: Vec<String>) -> i32 {
    install_signal_handlers();
    run_with(args, std::env::var_os("PATH"))
}

pub fn run_with(args: Vec<String>, path_env: Option<OsString>) -> i32 {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return EXIT_OK;
    }
    let args = match parse_args(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("h2 movies: {e}\n\n{USAGE}");
            return EXIT_USAGE;
        }
    };
    let ff = match discover(args.ffmpeg.as_deref(), path_env) {
        Ok(f) => f,
        Err(DiscoverError::Missing(m)) => {
            eprintln!(
                "h2 movies: {m}.\nThe movies are optional: without them the game skips the intro, attract and \
                 credits movies. To get them, install ffmpeg (with the Bink decoder) or pass --ffmpeg PATH, then re-run."
            );
            return EXIT_NO_FFMPEG;
        }
        Err(DiscoverError::Unusable(m)) => {
            eprintln!(
                "h2 movies: {m}\nThe movies are optional; the game skips them. Use an ffmpeg build with the Bink decoder and libx264."
            );
            return EXIT_FFMPEG_UNUSABLE;
        }
    };
    match convert_all(&args, &ff, &INTERRUPTED) {
        Ok(Outcome::Done) => EXIT_OK,
        Ok(Outcome::Failed) => EXIT_FAILED,
        Ok(Outcome::Interrupted) => EXIT_INTERRUPTED,
        Err(e) => {
            eprintln!("h2 movies: {e:#}");
            EXIT_FAILED
        }
    }
}

struct Done {
    name: String,
    duration: f64,
    size: u64,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Done,
    Failed,
    Interrupted,
}

type Iso = xiso::Xiso<std::io::BufReader<File>>;

fn list_sources(iso: &mut Iso, only: Option<&str>) -> Result<Vec<xiso::Entry>> {
    let mut v: Vec<_> = iso
        .entries()?
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".bik"))
        .filter(|e| only.is_none_or(|o| e.path.to_ascii_lowercase().contains(o)))
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}

/// An unreadable existing manifest means we would lose the record of earlier
/// conversions, so that needs `--force`; a missing one is simply new.
fn load_manifest(dir: &Path, force: bool) -> Result<Manifest> {
    match Manifest::load(dir) {
        Ok(m) => Ok(m.unwrap_or_default()),
        Err(e) if force => {
            eprintln!("warning: replacing unreadable {MANIFEST_NAME}: {e}");
            Ok(Manifest::default())
        }
        Err(e) => {
            bail!("existing {MANIFEST_NAME} in {} is unreadable ({e}); fix or remove it, or use --force", dir.display())
        }
    }
}

/// Convert one movie; on success the output and manifest are written.
#[allow(clippy::too_many_arguments)]
fn convert_one(
    iso: &mut Iso,
    src: &xiso::Entry,
    ff: &Ffmpeg,
    outputs: (&OutDir, &OutDir),
    scratch: &Path,
    manifest: &mut Manifest,
    cancel: &AtomicBool,
) -> Result<Done> {
    let (out, manifest_out) = outputs;
    let file = src.path.rsplit('/').next().unwrap_or(&src.path);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let output_name = format!("{stem}.mp4");
    let tmp = scratch.join(&output_name);
    let mut sha = None;
    transcode(ff, &tmp, |w| {
        let mut tee =
            Tee { inner: w, sha: Sha256::new(), done: 0, total: src.size as u64, next_report: 10, name: file, cancel };
        let n = iso.copy_to(src, &mut tee).map_err(|e| io::Error::other(e.to_string()))?;
        sha = Some(tee.sha.finish());
        Ok(n)
    })?;
    let sha = sha.context("source was not fully read")?;
    let duration = probe_duration(ff, &tmp).context("could not read the output's duration")?;
    let mut f = File::open(&tmp)?;
    let size = f.metadata()?.len();
    if size == 0 {
        bail!("ffmpeg produced an empty file");
    }
    let path = out.write_from(std::slice::from_ref(&output_name), &mut f)?;
    let _ = fs::remove_file(&tmp);
    manifest.upsert(MovieEntry {
        source_name: src.path.clone(),
        source_size: src.size as u64,
        source_sha256: movies::hex(&sha),
        output_name: output_name.clone(),
        output_size: size,
        duration_secs: duration,
        ffmpeg_version: ff.version.clone(),
        settings: ff.settings().to_string(),
    });
    // OutDir publishes via a temp file and rename, so this is atomic.
    manifest_out.write(&[MANIFEST_NAME.to_string()], manifest.to_json().as_bytes())?;
    println!("  wrote {}", path.display());
    Ok(Done { name: output_name, duration, size })
}

fn convert_all(args: &Args, ff: &Ffmpeg, cancel: &AtomicBool) -> Result<Outcome> {
    let iso_path = match &args.iso {
        Some(p) => p.clone(),
        None => crate::default_disc()?,
    };
    let mut iso = crate::open_iso(&iso_path)?;
    let sources = list_sources(&mut iso, args.only.as_deref())?;
    if sources.is_empty() {
        bail!("no matching .bik movies found on {}", iso_path.display());
    }
    println!("using {} ({}, {})", ff.path.display(), ff.version, ff.settings());

    let out = OutDir::new(&args.out, args.force)?;
    // The manifest is always replaced (atomically); only regular files are.
    let manifest_out = OutDir::new(&args.out, true)?;
    let mut manifest = load_manifest(&args.out, args.force)?;
    let scratch = Scratch::new().context("creating scratch directory")?;
    let (mut done, mut failed) = (Vec::new(), 0);
    for (i, src) in sources.iter().enumerate() {
        let file = src.path.rsplit('/').next().unwrap_or(&src.path);
        let output_name = format!("{}.mp4", file.rsplit_once('.').map_or(file, |(s, _)| s));
        println!("[{}/{}] {} ({} bytes) -> {output_name}", i + 1, sources.len(), src.path, src.size);
        let existing = args.out.join(&output_name);
        if !args.force && fs::symlink_metadata(&existing).is_ok() {
            eprintln!("  failed: {} exists (use --force to replace it)", existing.display());
            failed += 1;
            continue;
        }
        match convert_one(&mut iso, src, ff, (&out, &manifest_out), &scratch.0, &mut manifest, cancel) {
            Ok(d) => done.push(d),
            Err(e) => {
                eprintln!("  failed: {e:#}");
                failed += 1;
            }
        }
        let _ = fs::remove_file(scratch.0.join(&output_name));
        if cancel.load(Ordering::Relaxed) {
            eprintln!("interrupted; scratch files removed");
            return Ok(Outcome::Interrupted);
        }
    }
    println!("\nsummary");
    for d in &done {
        println!("  {:<16} {:>8.2} s  {:>8.1} MiB", d.name, d.duration, d.size as f64 / 1048576.0);
    }
    if failed > 0 {
        println!("{failed} of {} movies failed", sources.len());
    } else {
        println!("all {} movies converted; manifest {}", done.len(), args.out.join(MANIFEST_NAME).display());
    }
    Ok(if failed == 0 { Outcome::Done } else { Outcome::Failed })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2-movies-cmd-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// A shell script posing as ffmpeg. `decoders`/`encoders` are the listing
    /// lines; `body` handles the encode (last argument is the output file).
    fn fake(dir: &Path, name: &str, version: &str, decoders: &str, encoders: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n-version) echo '{version}'; exit 0;;\n\
             -hide_banner) case \"$2\" in -decoders) printf '{decoders}'; exit 0;; -encoders) printf '{encoders}'; exit 0;; esac;;\nesac\n\
             for a; do last=$a; done\n{body}\n"
        );
        fs::write(&p, script).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    const DEC: &str = " V....D binkvideo  Bink video\\n A....D binkaudio_dct  Bink Audio\\n";
    const ENC: &str = " V....D libx264  H.264\\n A....D aac  AAC\\n";
    const COPY: &str = "cat > \"$last\"";

    fn good(d: &Path) -> PathBuf {
        fake(d, "ffmpeg", "ffmpeg version 9.9-fake Copyright (c) someone", DEC, ENC, COPY)
    }

    #[test]
    fn args_parse() {
        let a = parse_args(&s(&["d.iso", "--out", "o", "--ffmpeg", "/x/ff", "--force"])).unwrap();
        assert_eq!(
            a,
            Args { iso: Some("d.iso".into()), out: "o".into(), ffmpeg: Some("/x/ff".into()), force: true, only: None }
        );
        let a = parse_args(&s(&["--out", "o"])).unwrap();
        assert_eq!((a.iso, a.ffmpeg, a.force), (None, None, false));
        // --out defaults to the app data directory play and run read.
        assert_eq!(parse_args(&s(&["d.iso"])).unwrap().out, movies::install::app_movies_dir().unwrap());
        assert!(parse_args(&s(&["--out"])).is_err());
        assert!(parse_args(&s(&["--out", "o", "--bogus"])).is_err());
        assert!(parse_args(&s(&["a.iso", "b.iso", "--out", "o"])).is_err());
        assert_eq!(parse_args(&s(&["--out", "o", "--only", "INTRO"])).unwrap().only.as_deref(), Some("intro"));
        assert!(parse_args(&s(&["--out", "o", "--only"])).is_err());
    }

    #[test]
    fn discovery_via_flag_and_path() {
        let d = dir("disc");
        let ff = good(&d);
        let f = discover(Some(&ff), None).unwrap();
        assert_eq!(f.version, "ffmpeg version 9.9-fake");
        assert_eq!(f.video, VideoEncoder::X264);
        let f = discover(None, Some(std::env::join_paths([&d]).unwrap())).unwrap();
        assert_eq!(f.path, ff);
        // missing
        assert!(matches!(discover(Some(&d.join("nope")), None), Err(DiscoverError::Missing(_))));
        assert!(matches!(discover(None, Some("/nonexistent-h2".into())), Err(DiscoverError::Missing(_))));
        assert!(matches!(discover(None, None), Err(DiscoverError::Missing(_))));
    }

    #[test]
    fn discovery_rejects_unusable_builds() {
        let d = dir("unusable");
        let nobink = fake(&d, "a", "ffmpeg version 1 Copyright", " V....D h264\\n", ENC, COPY);
        let notff = fake(&d, "b", "something else", DEC, ENC, COPY);
        let nox264 = fake(&d, "c", "ffmpeg version 1 Copyright", DEC, " A....D aac\\n", COPY);
        let noaac = fake(&d, "e", "ffmpeg version 1 Copyright", DEC, " V....D libx264\\n", COPY);
        let vt = fake(&d, "f", "ffmpeg version 1 Copyright", DEC, " V....D h264_videotoolbox\\n A....D aac\\n", COPY);
        for p in [&nobink, &notff, &nox264, &noaac] {
            assert!(matches!(discover(Some(p), None), Err(DiscoverError::Unusable(_))), "{p:?}");
        }
        assert_eq!(discover(Some(&vt), None).unwrap().video, VideoEncoder::VideoToolbox);
    }

    #[test]
    fn exit_codes() {
        let d = dir("exit");
        let o = d.join("out");
        let o = o.to_str().unwrap();
        assert_eq!(run_with(s(&["--help"]), None), EXIT_OK);
        assert_eq!(run_with(s(&["x.iso", "--bogus"]), None), EXIT_USAGE);
        // ffmpeg missing: distinct code, before the disc is even opened.
        assert_eq!(run_with(s(&["nope.iso", "--out", o]), Some("/nonexistent-h2".into())), EXIT_NO_FFMPEG);
        let missing = d.join("missing");
        assert_eq!(run_with(s(&["nope.iso", "--out", o, "--ffmpeg", missing.to_str().unwrap()]), None), EXIT_NO_FFMPEG);
        // ffmpeg present but no Bink decoder.
        let bad = fake(&d, "bad", "ffmpeg version 1 Copyright", " V....D h264\\n", ENC, COPY);
        assert_eq!(
            run_with(s(&["nope.iso", "--out", o, "--ffmpeg", bad.to_str().unwrap()]), None),
            EXIT_FFMPEG_UNUSABLE
        );
        // ffmpeg fine but the disc cannot be opened: failure, not "missing".
        let ff = good(&d);
        assert_eq!(run_with(s(&["nope.iso", "--out", o, "--ffmpeg", ff.to_str().unwrap()]), None), EXIT_FAILED);
    }

    #[test]
    fn transcode_streams_stdin_to_output() {
        let d = dir("tc");
        let ff = discover(Some(&good(&d)), None).unwrap();
        let out = d.join("o.mp4");
        let data = vec![7u8; 1 << 20];
        transcode(&ff, &out, |w| w.write_all(&data).map(|()| data.len() as u64)).unwrap();
        assert_eq!(fs::read(&out).unwrap(), data);
    }

    #[test]
    fn transcode_reports_ffmpeg_failure() {
        let d = dir("tcfail");
        let p = fake(&d, "ffmpeg", "ffmpeg version 1 Copyright", DEC, ENC, "echo 'Invalid data found' >&2; exit 1");
        let ff = discover(Some(&p), None).unwrap();
        // ffmpeg exits without reading stdin: the broken pipe must not mask its message.
        let e = transcode(&ff, &d.join("o.mp4"), |w| w.write_all(&vec![0u8; 8 << 20]).map(|()| 0)).unwrap_err();
        assert!(format!("{e:#}").contains("Invalid data found"), "{e:#}");
    }

    #[test]
    fn transcode_propagates_source_read_errors() {
        let d = dir("tcread");
        let ff = discover(Some(&good(&d)), None).unwrap();
        let e = transcode(&ff, &d.join("o.mp4"), |_| Err(io::Error::other("disc unreadable"))).unwrap_err();
        assert!(format!("{e:#}").contains("disc unreadable"));
    }

    #[test]
    fn transcode_reaps_child_when_feed_panics() {
        let d = dir("tcpanic");
        let ff = discover(Some(&good(&d)), None).unwrap();
        let r = std::panic::catch_unwind(|| transcode(&ff, &d.join("o.mp4"), |_| panic!("feed bug")));
        assert!(r.is_err()); // propagated, and did not hang on a live child
    }

    #[test]
    fn interrupt_flag_stops_the_stream() {
        let flag = AtomicBool::new(true);
        let mut sink = Vec::new();
        let mut t =
            Tee { inner: &mut sink, sha: Sha256::new(), done: 0, total: 3, next_report: 10, name: "x", cancel: &flag };
        assert!(t.write(b"abc").is_err());
        // io::copy (used by Xiso::copy_to) must terminate rather than retry.
        assert!(io::copy(&mut &b"abcdef"[..], &mut t).is_err());
        assert!(sink.is_empty());
    }

    #[test]
    fn scratch_is_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let p = {
            let s = Scratch::new().unwrap();
            assert_eq!(fs::metadata(&s.0).unwrap().permissions().mode() & 0o777, 0o700);
            fs::write(s.0.join("x"), b"1").unwrap();
            s.0.clone()
        };
        assert!(!p.exists());
    }

    #[test]
    fn corrupt_manifest_needs_force() {
        let d = dir("badmanifest");
        assert!(load_manifest(&d, false).unwrap().movies.is_empty()); // missing is fine
        fs::write(d.join(MANIFEST_NAME), "{garbage").unwrap();
        assert!(load_manifest(&d, false).is_err());
        assert!(load_manifest(&d, true).unwrap().movies.is_empty());
    }

    /// Real disc and real ffmpeg: converts the intro and checks the manifest.
    /// Run with `H2_DISC=... cargo test -p cli -- --ignored`.
    #[test]
    #[ignore = "needs the disc (H2_DISC or disc/) and a real ffmpeg with the Bink decoder"]
    fn converts_the_real_intro() {
        let out = dir("real-intro");
        let iso = crate::default_disc().expect("disc");
        let code = run_with(
            s(&[iso.to_str().unwrap(), "--out", out.to_str().unwrap(), "--only", "intro"]),
            std::env::var_os("PATH"),
        );
        assert_eq!(code, EXIT_OK);
        let m = Manifest::load(&out).unwrap().unwrap();
        let e = m.find("intro_60").expect("manifest entry");
        assert_eq!(e.source_size, 13312428);
        assert!(e.duration_secs > 10.0 && e.duration_secs < 30.0, "{}", e.duration_secs);
        let path = movies::locate_fresh(&out, "intro_60", e.source_size, &e.source_sha256).expect("fresh output");
        let head = fs::read(&path).unwrap();
        assert_eq!(&head[4..8], b"ftyp");
        // Second run without --force refuses to overwrite.
        let again = run_with(
            s(&[iso.to_str().unwrap(), "--out", out.to_str().unwrap(), "--only", "intro"]),
            std::env::var_os("PATH"),
        );
        assert_eq!(again, EXIT_FAILED);
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration("Input #0\n  Duration: 00:01:23.50, start: 0.0, bitrate: 1 kb/s"), Some(83.5));
        assert_eq!(parse_duration("  Duration: 01:00:00.00,"), Some(3600.0));
        assert_eq!(parse_duration("  Duration: N/A, start"), None);
        assert_eq!(parse_duration("nothing"), None);
    }

    #[test]
    fn tee_hashes_and_counts() {
        let mut sink = Vec::new();
        let mut t = Tee {
            inner: &mut sink,
            sha: Sha256::new(),
            done: 0,
            total: 3,
            next_report: 10,
            name: "x",
            cancel: &AtomicBool::new(false),
        };
        t.write_all(b"abc").unwrap();
        assert_eq!(t.done, 3);
        assert_eq!(movies::hex(&t.sha.finish()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sink, b"abc");
    }
}
