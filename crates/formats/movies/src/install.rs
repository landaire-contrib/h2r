//! Optional installation using an external, user-owned ffmpeg executable.
use crate::{
    Freshness, MANIFEST_NAME, Manifest, MovieEntry, OutputStamp, Sha256, hex, locate_entry,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use xiso::{Entry, Xiso};

/// The installer accepts an app data directory, never a checkout directory.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub data_dir: PathBuf,
    /// Explicit ffmpeg executable. Never a shell command. When `None`, the
    /// `H2_FFMPEG` environment variable, then PATH, then the usual package
    /// manager directories are tried (see [`find_ffmpeg`]).
    pub ffmpeg: Option<PathBuf>,
}
impl InstallOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ffmpeg: None,
        }
    }
}

/// Where Homebrew (Apple silicon, Intel) and MacPorts put ffmpeg. A
/// Finder-launched app has PATH=/usr/bin:/bin and would never find these.
const FFMPEG_FALLBACK_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];

/// Pick an ffmpeg executable. An explicit path or a non-empty `H2_FFMPEG`
/// is used exclusively, so a wrong override is reported rather than hidden.
/// Otherwise the first existing `ffmpeg` on `path_var`, then in `fallbacks`.
pub fn find_ffmpeg(
    explicit: Option<&Path>,
    env_override: Option<&std::ffi::OsStr>,
    path_var: Option<&std::ffi::OsStr>,
    fallbacks: &[&Path],
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Some(p) = env_override.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let on_path = path_var.into_iter().flat_map(std::env::split_paths);
    on_path
        .chain(fallbacks.iter().map(|p| p.to_path_buf()))
        .filter(|d| d.is_absolute())
        .map(|d| d.join("ffmpeg"))
        .find(|c| c.is_file())
}

/// Whose per-user data convention [`app_movies_dir`] follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataHost {
    Apple,
    Windows,
    Xdg,
}

impl DataHost {
    const CURRENT: Self = if cfg!(target_vendor = "apple") {
        Self::Apple
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Xdg
    };
}

/// Default data destination for the native desktop application, beside the
/// game's data root: `~/Library/Application Support/h2recomp/movies` on
/// Apple hosts, `%LOCALAPPDATA%\h2recomp\movies` on Windows and
/// `$XDG_DATA_HOME/h2recomp/movies` (else `~/.local/share/...`) elsewhere.
pub fn app_movies_dir() -> io::Result<PathBuf> {
    movies_dir_for(DataHost::CURRENT, |name| std::env::var_os(name))
}

fn movies_dir_for(host: DataHost, env: impl Fn(&str) -> Option<std::ffi::OsString>) -> io::Result<PathBuf> {
    let var = |name: &str| env(name).filter(|s| !s.is_empty()).map(PathBuf::from);
    let need = |name: &str| {
        var(name).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} is not set")))
    };
    let base = match host {
        DataHost::Apple => need("HOME")?.join("Library/Application Support"),
        DataHost::Windows => need("LOCALAPPDATA")?,
        // The XDG spec has a relative XDG_DATA_HOME ignored.
        DataHost::Xdg => match var("XDG_DATA_HOME").filter(|p| p.is_absolute()) {
            Some(xdg) => xdg,
            None => need("HOME")?.join(".local/share"),
        },
    };
    Ok(base.join("h2recomp").join("movies"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    Complete,
    /// Installation may continue without movies; existing conversions survive.
    FfmpegAbsent,
    /// Installation may continue; show this reason and offer a later retry.
    FfmpegUnusable(String),
}
#[derive(Debug)]
pub struct InstallReport {
    pub status: InstallStatus,
    pub converted: Vec<String>,
    pub reused: Vec<String>,
}

/// Discover every non-directory `.bik`, at any depth, case-insensitively.
pub fn discover<R: Read + Seek>(disc: &mut Xiso<R>) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<_> = disc
        .entries()
        .map_err(io::Error::other)?
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".bik"))
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Open the user's ISO read-only. No source media is ever written back.
pub fn install_iso(iso: &Path, options: &InstallOptions) -> io::Result<InstallReport> {
    let mut disc = Xiso::open(File::open(iso)?).map_err(io::Error::other)?;
    install(&mut disc, options)
}

/// Install or resume. Each successful movie is committed separately. Source
/// hashes are checked even on a resume; outputs need both size and SHA-256.
/// A failed conversion returns an error, preserving earlier manifest entries.
pub fn install<R: Read + Seek>(
    disc: &mut Xiso<R>,
    options: &InstallOptions,
) -> io::Result<InstallReport> {
    let entries = discover(disc)?;
    install_movies(
        &entries,
        options,
        |entry| (entry.path.as_str(), u64::from(entry.size)),
        |entry, mut out| copy_exact(disc, entry, &mut out),
    )
}

/// Install a single movie from a byte range in an ISO or an extracted file.
/// `disc_path` is the exact source path (such as `/bink/attract_60.bik`) used
/// to identify it in the manifest. Each copy is bounded by the extent.
pub fn install_region(
    path: &Path,
    offset: u64,
    length: u64,
    disc_path: &str,
    options: &InstallOptions,
) -> io::Result<InstallReport> {
    let name = disc_path.strip_prefix('/').unwrap_or(disc_path);
    if !name.to_ascii_lowercase().ends_with(".bik")
        || name.contains(['\\', ':', '\0'])
        || name.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a slash-separated .bik path on disc",
        ));
    }
    let end = offset
        .checked_add(length)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "movie byte range overflows"))?;
    let mut file = File::open(path)?;
    if end > file.metadata()?.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated movie region",
        ));
    }
    let source = format!("/{name}");
    install_movies(
        &[source],
        options,
        |source| (source.as_str(), length),
        |_, out| {
            file.seek(SeekFrom::Start(offset))?;
            let copied = io::copy(&mut (&mut file).take(length), out)?;
            if copied != length {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated movie region",
                ));
            }
            Ok(())
        },
    )
}

fn install_movies<E>(
    entries: &[E],
    options: &InstallOptions,
    describe: impl Fn(&E) -> (&str, u64),
    mut copy: impl FnMut(&E, &mut dyn Write) -> io::Result<()>,
) -> io::Result<InstallReport> {
    let fallbacks = FFMPEG_FALLBACK_DIRS.map(Path::new);
    let found = find_ffmpeg(
        options.ffmpeg.as_deref(),
        std::env::var_os("H2_FFMPEG").as_deref(),
        std::env::var_os("PATH").as_deref(),
        &fallbacks,
    );
    let probed = match found {
        Some(path) => Ffmpeg::probe(&path)?,
        None => Err(InstallStatus::FfmpegAbsent),
    };
    let ffmpeg = match probed {
        Ok(f) => f,
        Err(status) => {
            // Without a usable ffmpeg an already complete install is still
            // complete. The check is read-only: nothing is created or locked.
            let reused =
                installed_without_ffmpeg(entries, &options.data_dir, &describe, &mut copy)?;
            return Ok(match reused {
                Some(reused) => InstallReport {
                    status: InstallStatus::Complete,
                    converted: vec![],
                    reused,
                },
                None => InstallReport {
                    status,
                    converted: vec![],
                    reused: vec![],
                },
            });
        }
    };
    let dir = prepare_dir(&options.data_dir, true)?;
    // Advisory OS lock survives as a file, but is released on crash/exit. Do not
    // unlink it: that would let concurrent installers lock different inodes.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".install.lock"))?;
    lock.try_lock().map_err(io::Error::other)?;
    let mut manifest = Manifest::load(&dir)?.unwrap_or_default();
    // Holding the lock, nobody else owns a scratch directory in `dir`.
    sweep(&dir, |name| name.starts_with(".install-"), true);
    let scratch_root = std::env::temp_dir().canonicalize()?;
    let mut report = InstallReport {
        status: InstallStatus::Complete,
        converted: vec![],
        reused: vec![],
    };
    for entry in entries {
        let (source_name, source_size) = describe(entry);
        let mut digest = DigestWriter::default();
        copy(entry, &mut digest)?;
        let source_hash = hex(&digest.sha.finish());
        if let Some(old) = find_source(&manifest, source_name)
            && old.freshness(source_size, &source_hash) == Freshness::Fresh
            && old.settings == ffmpeg.settings()
            && manifest.output_sha256.contains_key(&old.output_name)
            && locate_entry(&dir, old).is_some()
            && manifest.output_matches(&dir, old)?
        {
            let stamp = OutputStamp::of(&dir.join(&old.output_name))?;
            if manifest.output_stamp.get(&old.output_name) != Some(&stamp) {
                let name = old.output_name.clone();
                manifest.output_stamp.insert(name, stamp);
                commit_manifest(&dir, &manifest)?;
            }
            report.reused.push(source_name.to_owned());
            continue;
        }
        // Seekable input matters: Bink has indexed frames. Spooling also lets
        // the exact same path exercise this pipeline with generated test media.
        let scratch = Scratch::new(&scratch_root)?;
        let input = scratch.0.join("source");
        let mut source = File::create(&input)?;
        let mut copied = DigestWriter::default();
        copy(entry, &mut Tee(&mut source, &mut copied))?;
        drop(source);
        if hex(&copied.sha.finish()) != source_hash {
            return Err(io::Error::other("disc source changed while copying"));
        }
        let output = scratch.0.join("output.mov");
        let duration_secs = ffmpeg.encode(&input, &output)?;
        let output_hash = hash_file(&output)?;
        let output_size = fs::metadata(&output)?.len();
        if output_size == 0 {
            return Err(io::Error::other("ffmpeg produced an empty movie"));
        }
        // A fresh output name avoids replacing a file referenced by the old
        // manifest before the new manifest commits (including after a crash).
        let output_name = format!("movie-{output_hash}.mov");
        // Publication staging is on the destination filesystem, so rename
        // remains atomic even if TMPDIR is on a different volume.
        let publication = Scratch::new(&dir)?;
        let staged_output = publication.0.join("output.mov");
        fs::copy(&output, &staged_output)?;
        File::open(&staged_output)?.sync_all()?;
        fs::rename(&staged_output, dir.join(&output_name))?;
        let legacy = manifest.output_sha256.is_empty();
        let replaced_output = find_source(&manifest, source_name)
            .map(|old| old.output_name.clone())
            .filter(|old| old != &output_name);
        manifest.upsert(MovieEntry {
            source_name: source_name.to_owned(),
            source_size,
            source_sha256: source_hash,
            output_name: output_name.clone(),
            output_size,
            duration_secs,
            ffmpeg_version: ffmpeg.version.clone(),
            settings: ffmpeg.settings(),
        });
        if let Some(old) = replaced_output
            && !manifest.movies.iter().any(|m| m.output_name == old)
        {
            manifest.output_sha256.remove(&old);
            manifest.output_stamp.remove(&old);
        }
        manifest.output_stamp.insert(
            output_name.clone(),
            OutputStamp::of(&dir.join(&output_name))?,
        );
        manifest.output_sha256.insert(output_name, output_hash);
        // The first output hash enables verification for the entire manifest.
        // Keep already installed legacy movies playable during a partial install.
        if legacy {
            verify_legacy_outputs(&dir, &mut manifest);
        }
        commit_manifest(&dir, &manifest)?;
        report.converted.push(source_name.to_owned());
    }
    // Replaced or abandoned conversions: drop any movie the manifest no
    // longer references. Only after success, so a failed run keeps its files.
    let referenced: Vec<&str> = manifest
        .movies
        .iter()
        .map(|m| m.output_name.as_str())
        .collect();
    sweep(
        &dir,
        |name| name.starts_with("movie-") && name.ends_with(".mov") && !referenced.contains(&name),
        false,
    );
    Ok(report)
}

/// Atomically publish the manifest (staged on the destination filesystem).
fn commit_manifest(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    let staging = Scratch::new(dir)?;
    let manifest_tmp = staging.0.join(MANIFEST_NAME);
    let mut file = File::create(&manifest_tmp)?;
    file.write_all(manifest.to_json().as_bytes())?;
    file.sync_all()?;
    fs::rename(manifest_tmp, dir.join(MANIFEST_NAME))?;
    File::open(dir)?.sync_all()
}

/// Best-effort removal of matching entries directly inside `dir`.
fn sweep(dir: &Path, matches: impl Fn(&str) -> bool, directories: bool) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for item in read.flatten() {
        if !item.file_name().to_str().is_some_and(&matches) {
            continue;
        }
        let Ok(kind) = item.file_type() else { continue };
        if directories && kind.is_dir() {
            let _ = fs::remove_dir_all(item.path());
        } else if !directories && kind.is_file() {
            let _ = fs::remove_file(item.path());
        }
    }
}

fn find_source<'a>(manifest: &'a Manifest, name: &str) -> Option<&'a MovieEntry> {
    manifest
        .movies
        .iter()
        .find(|entry| entry.source_name.eq_ignore_ascii_case(name))
}

fn verify_legacy_outputs(dir: &Path, manifest: &mut Manifest) {
    for entry in &manifest.movies {
        if manifest.output_sha256.contains_key(&entry.output_name) {
            continue;
        }
        if let Some(path) = locate_entry(dir, entry)
            && let Ok(stamp) = OutputStamp::of(&path)
            && let Ok(hash) = hash_file(&path)
        {
            manifest
                .output_sha256
                .insert(entry.output_name.clone(), hash);
            manifest
                .output_stamp
                .insert(entry.output_name.clone(), stamp);
        }
    }
}

/// `Some(reused paths)` when every requested movie has a fresh, verified
/// conversion already installed; reads only, and never creates `data_dir`.
fn installed_without_ffmpeg<E>(
    entries: &[E],
    data_dir: &Path,
    describe: impl Fn(&E) -> (&str, u64),
    mut copy: impl FnMut(&E, &mut dyn Write) -> io::Result<()>,
) -> io::Result<Option<Vec<String>>> {
    let Ok(dir) = prepare_dir(data_dir, false) else {
        return Ok(None);
    };
    let Some(manifest) = Manifest::load(&dir).ok().flatten() else {
        return Ok(None);
    };
    let mut reused = vec![];
    for entry in entries {
        let (source_name, source_size) = describe(entry);
        let mut digest = DigestWriter::default();
        copy(entry, &mut digest)?;
        let hash = hex(&digest.sha.finish());
        match find_source(&manifest, source_name) {
            Some(old)
                if old.freshness(source_size, &hash) == Freshness::Fresh
                    && manifest.output_sha256.contains_key(&old.output_name)
                    && manifest.output_matches(&dir, old)? =>
            {
                reused.push(source_name.to_owned());
            }
            _ => return Ok(None),
        }
    }
    Ok((!reused.is_empty()).then_some(reused))
}

fn copy_exact<R: Read + Seek>(
    disc: &mut Xiso<R>,
    entry: &Entry,
    out: &mut impl Write,
) -> io::Result<()> {
    let n = disc.copy_to(entry, out).map_err(io::Error::other)?;
    if n != entry.size as u64 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated ISO movie",
        ));
    }
    Ok(())
}

/// Reject repository destinations, including worktrees and symlink aliases.
/// Callers may supply their platform's app data directory for sandboxed apps.
fn prepare_dir(path: &Path, create: bool) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Resolve each existing component; reject before creating anything in a repo.
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        resolved.push(component);
        if resolved.exists() {
            resolved = resolved.canonicalize()?;
        }
        if resolved.ancestors().any(|p| p.join(".git").exists()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "movies belong in the app data directory, outside a repository",
            ));
        }
    }
    if create {
        fs::create_dir_all(&resolved)?;
    }
    resolved.canonicalize()
}

struct Ffmpeg {
    path: PathBuf,
    version: String,
    encoder: &'static str,
}
impl Ffmpeg {
    fn probe(path: &Path) -> io::Result<Result<Self, InstallStatus>> {
        let version = match Command::new(path)
            .arg("-version")
            .stdin(Stdio::null())
            .output()
        {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned(),
            Ok(_) => {
                return Ok(Err(InstallStatus::FfmpegUnusable(
                    "ffmpeg -version failed".into(),
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Err(InstallStatus::FfmpegAbsent));
            }
            Err(e) => return Ok(Err(InstallStatus::FfmpegUnusable(e.to_string()))),
        };
        if !version.starts_with("ffmpeg version") {
            return Ok(Err(InstallStatus::FfmpegUnusable(
                "executable is not ffmpeg".into(),
            )));
        }
        let listing = |flag| -> io::Result<String> {
            let out = Command::new(path)
                .args(["-hide_banner", flag])
                .stdin(Stdio::null())
                .output()?;
            if !out.status.success() {
                return Err(io::Error::other(format!("ffmpeg {flag} failed")));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let capabilities = || -> io::Result<&'static str> {
            let dec = listing("-decoders")?;
            if !lists(&dec, "binkvideo")
                || !lists(&dec, "binkaudio_dct")
                || !lists(&dec, "binkaudio_rdft")
            {
                return Err(io::Error::other(
                    "ffmpeg needs Bink video and audio decoders",
                ));
            }
            let enc = listing("-encoders")?;
            if !lists(&enc, "aac") {
                return Err(io::Error::other("ffmpeg needs an AAC encoder"));
            }
            ["libx264", "h264_videotoolbox", "hevc_videotoolbox"]
                .into_iter()
                .find(|e| lists(&enc, e))
                .ok_or_else(|| io::Error::other("ffmpeg needs an H.264 or HEVC encoder"))
        };
        Ok(match capabilities() {
            Ok(encoder) => Ok(Self {
                path: path.into(),
                version,
                encoder,
            }),
            Err(e) => Err(InstallStatus::FfmpegUnusable(e.to_string())),
        })
    }
    fn settings(&self) -> String {
        format!(
            "{}; {}; yuv420p; aac 192k; mov faststart; v1",
            self.encoder,
            if self.encoder == "libx264" {
                "crf18 slow high"
            } else {
                "4M"
            }
        )
    }
    fn encode(&self, input: &Path, output: &Path) -> io::Result<f64> {
        let mut command = Command::new(&self.path);
        command
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(input)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-sn",
                "-dn",
                "-c:v",
                self.encoder,
            ]);
        if self.encoder == "libx264" {
            command.args(["-preset", "slow", "-crf", "18", "-profile:v", "high"]);
        } else {
            command.args(["-b:v", "4M"]);
        }
        if self.encoder.starts_with("hevc") {
            command.args(["-tag:v", "hvc1"]);
        }
        command
            .args([
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-f",
                "mov",
                "-n",
            ])
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = ChildGuard(command.spawn()?);
        // Drain both pipes concurrently and bound stored diagnostics. A noisy
        // failing process must never deadlock installation or exhaust memory.
        let stderr = child.0.stderr.take().unwrap();
        let log = std::thread::spawn(move || drain(stderr));
        let stdout = child.0.stdout.take().unwrap();
        let progress = drain(stdout);
        let status = child.0.wait()?;
        let log = log
            .join()
            .map_err(|_| io::Error::other("ffmpeg log reader panicked"))??;
        if !status.success() {
            return Err(io::Error::other(format!(
                "ffmpeg failed ({status}): {}",
                String::from_utf8_lossy(&log)
            )));
        }
        let progress = progress?;
        let text = String::from_utf8_lossy(&progress);
        let micros = text
            .lines()
            .filter_map(|l| l.strip_prefix("out_time_us=")?.parse::<u64>().ok())
            .next_back()
            .unwrap_or(0);
        Ok(micros as f64 / 1_000_000.0)
    }
}
fn lists(text: &str, name: &str) -> bool {
    text.lines()
        .any(|l| l.split_whitespace().nth(1) == Some(name))
}
fn drain(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() > 65536 {
            bytes.drain(..bytes.len() - 65536);
        }
    }
    Ok(bytes)
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let p = parent.join(format!(
                ".install-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&p) {
                // Canonical, so ffmpeg is handed a stable absolute path that
                // can never read as an option or traverse a symlink alias.
                Ok(()) => return Ok(Self(p.canonicalize()?)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct DigestWriter {
    sha: Sha256,
}
impl Write for DigestWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sha.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Tee<'a>(&'a mut File, &'a mut DigestWriter);
impl Write for Tee<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.0.write(buf)?;
        self.1.write_all(&buf[..n])?;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub(crate) fn hash_file(path: &Path) -> io::Result<String> {
    let mut hash = DigestWriter::default();
    io::copy(&mut File::open(path)?, &mut hash)?;
    Ok(hex(&hash.sha.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_checkout_and_parent_traversal() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(prepare_dir(&root.join("movie-output"), true).is_err());
        assert!(prepare_dir(&root.join("../movies/movie-output"), true).is_err());
        assert!(!root.join("movie-output").exists());
    }
    #[test]
    fn lists_matches_the_name_column_only() {
        let text = "Decoders:\n V..... = Video\n ------\n V....D binkvideo  Bink Video\n A....D binkaudio_dct Bink Audio (DCT)\n V....D other   mentions binkaudio_rdft here\n";
        assert!(lists(text, "binkvideo"));
        assert!(lists(text, "binkaudio_dct"));
        assert!(
            !lists(text, "binkaudio_rdft"),
            "description text is not a name"
        );
        assert!(!lists(text, "bink"), "no prefix matches");
        assert!(!lists("", "binkvideo"));
        assert!(
            !lists("binkvideo\n", "binkvideo"),
            "flags column is required"
        );
    }
    #[test]
    fn the_movies_dir_follows_the_host() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| std::ffi::OsString::from(v))
        };
        // Absolute on the host running the test, as `XDG_DATA_HOME` must be.
        const XDG: &str = if cfg!(windows) { r"C:\x" } else { "/x" };
        let all = vars(&[("HOME", "/h"), ("LOCALAPPDATA", "/l"), ("XDG_DATA_HOME", XDG)]);
        let dir = |host, env| movies_dir_for(host, env).ok();
        assert_eq!(dir(DataHost::Apple, all), Some(PathBuf::from("/h/Library/Application Support/h2recomp/movies")));
        assert_eq!(dir(DataHost::Windows, all), Some(PathBuf::from("/l").join("h2recomp").join("movies")));
        assert_eq!(dir(DataHost::Xdg, all), Some(PathBuf::from(XDG).join("h2recomp").join("movies")));
        let relative = vars(&[("HOME", "/h"), ("XDG_DATA_HOME", "rel")]);
        assert_eq!(dir(DataHost::Xdg, relative), Some(PathBuf::from("/h/.local/share/h2recomp/movies")));
        assert_eq!(dir(DataHost::Windows, vars(&[("HOME", "/h")])), None, "Windows never falls back to HOME");
        assert_eq!(dir(DataHost::Apple, vars(&[("HOME", "")])), None);
        assert_eq!(dir(DataHost::Xdg, vars(&[])), None);
    }
    #[test]
    fn ffmpeg_discovery_order() {
        let t = std::env::temp_dir().join(format!("h2-find-ffmpeg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        let (on_path, fallback, empty) = (t.join("path"), t.join("fallback"), t.join("empty"));
        for d in [&on_path, &fallback, &empty] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(on_path.join("ffmpeg"), "").unwrap();
        fs::write(fallback.join("ffmpeg"), "").unwrap();
        let path_var = std::env::join_paths([&empty, &on_path]).unwrap();
        let fb = [fallback.as_path()];
        let explicit = Path::new("/x/explicit");
        let env = std::ffi::OsStr::new("/x/env");
        let find = |e, v, p| find_ffmpeg(e, v, p, &fb);
        assert_eq!(
            find(Some(explicit), Some(env), Some(&path_var)),
            Some(explicit.into())
        );
        assert_eq!(
            find(None, Some(env), Some(&path_var)),
            Some("/x/env".into())
        );
        assert_eq!(
            find(None, Some("".as_ref()), Some(&path_var)),
            Some(on_path.join("ffmpeg"))
        );
        // Finder-style PATH without ffmpeg falls back to the package dirs.
        let finder = empty.as_os_str();
        assert_eq!(
            find(None, None, Some(finder)),
            Some(fallback.join("ffmpeg"))
        );
        assert_eq!(find(None, None, None), Some(fallback.join("ffmpeg")));
        assert_eq!(
            find_ffmpeg(None, None, Some(finder), &[empty.as_path()]),
            None
        );
        // A relative PATH entry is never searched.
        assert_eq!(find_ffmpeg(None, None, Some("rel".as_ref()), &[]), None);
        let _ = fs::remove_dir_all(&t);
    }
    #[test]
    fn missing_ffmpeg_is_optional() {
        assert!(matches!(
            Ffmpeg::probe(Path::new("/nonexistent-h2-ffmpeg")).unwrap(),
            Err(InstallStatus::FfmpegAbsent)
        ));
    }

    #[test]
    fn invalid_movie_regions_are_rejected_before_installation() {
        let scratch = Scratch::new(&std::env::temp_dir()).unwrap();
        let source = scratch.0.join("disc.img");
        fs::write(&source, b"synthetic movie data").unwrap();
        let options = InstallOptions::new(scratch.0.join("movies"));
        assert_eq!(
            install_region(&source, 0, 5, "/bink/../intro.bik", &options)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            install_region(&source, u64::MAX, 1, "/bink/intro.bik", &options)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            install_region(&source, 5, 100, "/bink/intro.bik", &options)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(!options.data_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn installs_and_resumes_only_the_named_region() {
        use std::os::unix::fs::PermissionsExt;

        const TARGET: &str = "/bink/attract_60.bik";
        const LEGACY: &str = "/bink/intro_60.bik";
        const MOVIE: &[u8] = b"synthetic encoding";

        let scratch = Scratch::new(&std::env::temp_dir()).unwrap();
        let source = scratch.0.join("disc.img");
        let mut data = b"prefix".to_vec();
        data.extend_from_slice(MOVIE);
        data.extend_from_slice(b"suffix");
        fs::write(&source, &data).unwrap();

        let mut options = InstallOptions::new(scratch.0.join("movies"));
        fs::create_dir(&options.data_dir).unwrap();
        fs::write(options.data_dir.join("old.mov"), b"old").unwrap();
        let mut manifest = Manifest::default();
        for source_name in [LEGACY, TARGET] {
            manifest.upsert(MovieEntry {
                source_name: source_name.to_owned(),
                source_sha256: "legacy".to_owned(),
                source_size: 3,
                output_name: "old.mov".to_owned(),
                output_size: 3,
                duration_secs: 1.0,
                ffmpeg_version: "legacy ffmpeg".to_owned(),
                settings: "legacy".to_owned(),
            });
        }
        fs::write(options.data_dir.join(MANIFEST_NAME), manifest.to_json()).unwrap();

        // A synthetic encoder copies its seekable input to the output,
        // making any extra bytes from the enclosing file visible here.
        let ffmpeg = scratch.0.join("ffmpeg");
        fs::write(&ffmpeg, r#"#!/bin/sh
case "$1:$2" in
-version:) printf 'ffmpeg version synthetic\n' ;;
-hide_banner:-decoders) printf ' V..... binkvideo\n A..... binkaudio_dct\n A..... binkaudio_rdft\n' ;;
-hide_banner:-encoders) printf ' V..... libx264\n A..... aac\n' ;;
-nostdin:-hide_banner)
    [ "$5" = -i ] || exit 2
    for output in "$@"; do :; done
    /bin/cat "$6" > "$output" || exit 3
    printf 'out_time_us=1000000\n'
    ;;
*) exit 1 ;;
esac
"#).unwrap();
        fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o700)).unwrap();
        options.ffmpeg = Some(ffmpeg);
        let offset = b"prefix".len() as u64;
        let length = MOVIE.len() as u64;
        let report = install_region(&source, offset, length, TARGET, &options).unwrap();
        assert_eq!(report.status, InstallStatus::Complete);
        assert_eq!(report.converted, [TARGET]);
        assert!(report.reused.is_empty());

        let installed = Manifest::load(&options.data_dir).unwrap().unwrap();
        let entry = installed.find(TARGET).unwrap();
        let mut digest = Sha256::default();
        digest.update(MOVIE);
        assert_eq!(entry.source_sha256, hex(&digest.finish()));
        assert_eq!(entry.source_size, length);
        let new_output = crate::locate(&options.data_dir, TARGET).unwrap();
        assert_eq!(fs::read(new_output).unwrap().as_slice(), MOVIE);
        assert_eq!(installed.find(LEGACY), manifest.find(LEGACY));
        assert!(crate::locate(&options.data_dir, LEGACY).is_some());
        assert_eq!(fs::read(&source).unwrap(), data);

        let no_ffmpeg = scratch.0.join("missing-ffmpeg");
        options.ffmpeg = Some(no_ffmpeg);
        let report = install_region(&source, offset, length, TARGET, &options).unwrap();
        assert_eq!(report.status, InstallStatus::Complete);
        assert_eq!(report.reused, [TARGET]);
        assert!(report.converted.is_empty());
    }
}
