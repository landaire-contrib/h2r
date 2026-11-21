//! Writing packages into `<data>/maps/`: a package directory, and the
//! `active.toml` choice. The editor saves through this; the resolver reads
//! the same layout (docs/game/maps.md §1, "The package directory").
//!
//! Every write is atomic for a reader: a new package is assembled in a
//! hidden sibling directory (a `.` name, which [`Library`](crate::Library)
//! skips) and renamed into place; a package being replaced gets its new
//! files first, then its manifest by rename, which is the commit point,
//! then loses the files the old manifest named and the new one does not.
//! `active.toml` is rewritten by rename, keeping its other entries.
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{
    ACTIVE_FILE, CACHE_DIR, Manifest, ManifestError,
    manifest::{MANIFEST_FILE, MAX_IMAGE_BYTES, is_map_name, relative},
};

/// The packages directory under a data root.
pub const MAPS_DIR: &str = "maps";

/// Which per-user application data convention the host follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataHost {
    /// `~/Library/Application Support/h2recomp`.
    Library,
    /// `%LOCALAPPDATA%\h2recomp`.
    Windows,
    /// `$XDG_DATA_HOME/h2recomp`, else `~/.local/share/h2recomp`.
    Xdg,
}

impl DataHost {
    /// The convention of the host this binary runs on: Apple, Windows, or
    /// XDG on every other Unix (as `d3d::pipeline_store`'s cache host).
    pub const CURRENT: Self = if cfg!(target_vendor = "apple") {
        Self::Library
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Xdg
    };
}

/// The per-user `h2recomp` application data directory under `host`'s
/// convention, reading variables through `env`. `None` when the variable it
/// needs is unset or empty. A relative `XDG_DATA_HOME` is ignored, as the
/// XDG spec requires. A UWP host passes its package's LocalFolder to
/// `vfs::Config::new` instead.
pub fn app_data_dir(host: DataHost, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = match host {
        DataHost::Library => var("HOME")?.join("Library/Application Support"),
        DataHost::Windows => var("LOCALAPPDATA")?,
        DataHost::Xdg => var("XDG_DATA_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| var("HOME").map(|h| h.join(".local/share")))?,
    };
    Some(base.join("h2recomp"))
}

/// The runtime's default data root, as `h2 play`, `h2 run` and `h2
/// settings` use when `--data` is not given: `H2_DATA`, else
/// `<app data>/data` ([`app_data_dir`]), a persistent directory. Saves are
/// never put under `$TMPDIR`, which the system purges and which differs per
/// shell session; it is the root only when no app data directory resolves.
pub fn default_data_root() -> PathBuf {
    if let Some(d) = std::env::var_os("H2_DATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    app_data_dir(DataHost::CURRENT, |name| std::env::var_os(name))
        .map_or_else(|| std::env::temp_dir().join("h2boot-data"), |b| b.join("data"))
}

/// `<data>/maps`.
pub fn maps_dir(data_root: &Path) -> PathBuf {
    data_root.join(MAPS_DIR)
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(
        "package directory {0:?}: use letters, digits, '_' and '-' (not \"cache\", at most 64)"
    )]
    DirName(String),
    #[error("{0} already exists and is not the package being replaced")]
    Exists(PathBuf),
    #[error("{0}")]
    Manifest(#[from] ManifestError),
    #[error("image: {0}")]
    Image(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Whether `name` may name a package directory: a plain name (as for maps)
/// other than the cache's.
pub fn is_dir_name(name: &str) -> bool {
    is_map_name(name) && !name.eq_ignore_ascii_case(CACHE_DIR)
}

/// The image kind of `bytes` (`"png"` or `"jpg"`), checking the size limit
/// and the signature, as the resolver does.
pub fn image_kind(bytes: &[u8]) -> Result<&'static str, StoreError> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(StoreError::Image(format!(
            "{} bytes, over {MAX_IMAGE_BYTES}",
            bytes.len()
        )));
    }
    crate::manifest::image_signature(bytes)
        .ok_or_else(|| StoreError::Image("not a PNG or JPEG file".into()))
}

/// Write package `dir` under `maps`: `files` (name, bytes) and the
/// manifest. The manifest must name exactly the files given. With
/// `replace`, an existing package directory of that name is updated in
/// place (its manifest swapped last); without, an existing name is
/// refused. Returns the package directory.
pub fn write_package(
    maps: &Path,
    dir: &str,
    manifest: &Manifest,
    files: &[(&str, &[u8])],
    replace: bool,
) -> Result<PathBuf, StoreError> {
    if !is_dir_name(dir) {
        return Err(StoreError::DirName(dir.to_owned()));
    }
    manifest.validate()?;
    for (name, _) in files {
        relative(name)?;
        if *name == MANIFEST_FILE {
            return Err(StoreError::Io(io::Error::other(
                "a layer cannot be named manifest.toml",
            )));
        }
    }
    let named = manifest
        .patches
        .iter()
        .chain(&manifest.packs)
        .map(|l| &l.file);
    for f in named.chain(&manifest.package.image) {
        if !files.iter().any(|(n, _)| n == f) {
            return Err(StoreError::Io(io::Error::other(format!(
                "the manifest names {f}, which is not being written"
            ))));
        }
    }
    std::fs::create_dir_all(maps)?;
    let target = maps.join(dir);
    let text = manifest.to_toml();
    if target.exists() {
        if !replace {
            return Err(StoreError::Exists(target));
        }
        return replace_in(&target, &text, files);
    }
    let tmp = maps.join(format!(".{dir}.{}.new", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let result = (|| {
        std::fs::create_dir(&tmp)?;
        for (name, bytes) in files {
            write_synced(&tmp.join(name), bytes)?;
        }
        write_synced(&tmp.join(MANIFEST_FILE), text.as_bytes())?;
        manifest.validate_in(&tmp)?;
        std::fs::rename(&tmp, &target)?;
        Ok(target.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    result
}

fn replace_in(target: &Path, text: &str, files: &[(&str, &[u8])]) -> Result<PathBuf, StoreError> {
    let old: Option<Manifest> = std::fs::read_to_string(target.join(MANIFEST_FILE))
        .ok()
        .and_then(|t| Manifest::from_toml(&t).ok());
    for (name, bytes) in files {
        write_atomic(&target.join(name), bytes)?;
    }
    let new = Manifest::from_toml(text)?;
    new.validate_in(target)?;
    write_atomic(&target.join(MANIFEST_FILE), text.as_bytes())?;
    // Files only the old manifest named.
    if let Some(old) = old {
        let keep: Vec<&str> = files.iter().map(|f| f.0).collect();
        let named = old.patches.iter().chain(&old.packs).map(|l| &l.file);
        for f in named.chain(&old.package.image) {
            if !keep.contains(&f.as_str())
                && let Ok(rel) = relative(f)
            {
                let _ = std::fs::remove_file(target.join(rel));
            }
        }
    }
    Ok(target.to_path_buf())
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Write `path` by a temporary sibling and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("no file name"))?
        .to_string_lossy()
        .into_owned();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = write_synced(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn read_active(maps: &Path) -> io::Result<toml::Table> {
    match std::fs::read_to_string(maps.join(ACTIVE_FILE)) {
        Ok(t) => toml::from_str(&t).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(e),
    }
}

/// What `active.toml` says for `map`: `Some(Some(dir))` a package,
/// `Some(None)` the disc's map, `None` nothing (the choice rule decides).
pub fn active_choice(maps: &Path, map: &str) -> io::Result<Option<Option<String>>> {
    let table = read_active(maps)?;
    Ok(table
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(map))
        .and_then(|(_, v)| match v {
            toml::Value::String(s) => Some(Some(s.clone())),
            toml::Value::Boolean(false) => Some(None),
            _ => None,
        }))
}

/// Set `map`'s entry in `active.toml`: `Some(dir)` picks that package,
/// `None` removes the entry. Other entries are kept; entries for the same
/// map in another case are replaced. An unreadable `active.toml` is an
/// error, never overwritten.
pub fn set_active(maps: &Path, map: &str, dir: Option<&str>) -> Result<(), StoreError> {
    if !is_map_name(map) {
        return Err(ManifestError::MapName(map.to_owned()).into());
    }
    if let Some(d) = dir
        && !is_dir_name(d)
    {
        return Err(StoreError::DirName(d.to_owned()));
    }
    let mut table = read_active(maps)?;
    table.retain(|k, _| !k.eq_ignore_ascii_case(map));
    if let Some(d) = dir {
        table.insert(map.to_ascii_lowercase(), toml::Value::String(d.to_owned()));
    }
    std::fs::create_dir_all(maps)?;
    let text = toml::to_string(&table).map_err(io::Error::other)?;
    write_atomic(&maps.join(ACTIVE_FILE), text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Library, Sha256, manifest::Layer, resolve::tests::Temp};

    fn manifest(files: &[(&str, &[u8])]) -> Manifest {
        let mut m = Manifest::retail("lockout", Sha256([1; 32]));
        m.package.name = "Test".into();
        m.patches = files
            .iter()
            .map(|(f, b)| Layer {
                file: (*f).into(),
                sha256: Sha256::of(b),
            })
            .collect();
        m
    }

    #[test]
    fn the_app_data_dir_follows_the_host() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
        };
        // Absolute on the host running the test, as `XDG_DATA_HOME` must be.
        const XDG: &str = if cfg!(windows) { r"C:\x" } else { "/x" };
        let all = vars(&[("HOME", "/h"), ("LOCALAPPDATA", "/l"), ("XDG_DATA_HOME", XDG)]);
        assert_eq!(app_data_dir(DataHost::Library, &all), Some(PathBuf::from("/h/Library/Application Support/h2recomp")));
        assert_eq!(app_data_dir(DataHost::Windows, &all), Some(PathBuf::from("/l").join("h2recomp")));
        assert_eq!(app_data_dir(DataHost::Xdg, &all), Some(PathBuf::from(XDG).join("h2recomp")));
        let home = vars(&[("HOME", "/h"), ("XDG_DATA_HOME", "")]);
        assert_eq!(app_data_dir(DataHost::Xdg, &home), Some(PathBuf::from("/h/.local/share/h2recomp")));
        let relative = vars(&[("HOME", "/h"), ("XDG_DATA_HOME", "rel/data")]);
        assert_eq!(
            app_data_dir(DataHost::Xdg, &relative),
            Some(PathBuf::from("/h/.local/share/h2recomp")),
            "a relative XDG_DATA_HOME is ignored"
        );
        assert_eq!(app_data_dir(DataHost::Library, vars(&[("HOME", ""), ("LOCALAPPDATA", "/l")])), None);
        assert_eq!(app_data_dir(DataHost::Windows, vars(&[("HOME", "/h")])), None, "Windows never falls back to HOME");
        assert_eq!(app_data_dir(DataHost::Xdg, vars(&[])), None);
    }

    #[test]
    fn new_and_replaced_packages() {
        let t = Temp::new();
        let maps = t.0.join("maps");
        let files: &[(&str, &[u8])] = &[("a.ppf", b"PPF30one")];
        let m = manifest(files);
        let dir = write_package(&maps, "lockout-test", &m, files, false).unwrap();
        assert_eq!(std::fs::read(dir.join("a.ppf")).unwrap(), b"PPF30one");
        let lib = Library::open(&maps);
        assert_eq!(lib.packages().len(), 1);
        assert_eq!(lib.packages()[0].manifest, m);
        // No leftovers from the assembly.
        let names: Vec<_> = std::fs::read_dir(&maps)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["lockout-test"]);
        assert!(matches!(
            write_package(&maps, "lockout-test", &m, files, false),
            Err(StoreError::Exists(_))
        ));
        let files2: &[(&str, &[u8])] = &[("b.bps", b"BPS1two")];
        let m2 = manifest(files2);
        write_package(&maps, "lockout-test", &m2, files2, true).unwrap();
        assert!(!dir.join("a.ppf").exists(), "the old layer is removed");
        assert_eq!(std::fs::read(dir.join("b.bps")).unwrap(), b"BPS1two");
        assert_eq!(Library::open(&maps).packages()[0].manifest, m2);
        for bad in ["", "cache", "CACHE", ".hidden", "a/b", "a.b"] {
            assert!(
                matches!(
                    write_package(&maps, bad, &m, files, false),
                    Err(StoreError::DirName(_))
                ),
                "{bad}"
            );
        }
        // A manifest naming a file not written is refused, nothing left.
        assert!(write_package(&maps, "other", &m2, files, false).is_err());
        assert!(!maps.join("other").exists());
        let mut img = m.clone();
        img.package.image = Some("p.png".into());
        assert!(write_package(&maps, "broken", &img, files, false).is_err());
        assert!(!maps.join("broken").exists());
    }

    /// A written patch, saved as a package, resolves to the edited bytes,
    /// through the choice rule and directly; random edits, both formats.
    #[test]
    fn written_packages_resolve_to_the_edited_map() {
        use crate::{patch::diff, repair_checksum, resolve::tests::synthetic_map, testutil::Rng};
        for seed in 0..40 {
            let t = Temp::new();
            let maps = t.0.join("maps");
            let mut rng = Rng::new(seed);
            let n = 0x400 + rng.below(0x2000) as usize;
            let base = synthetic_map(&rng.bytes(n));
            let mut e = base.clone();
            for _ in 0..1 + rng.below(4) {
                let at = match rng.below(3) {
                    0 => e.len() - 1 - rng.below(4) as usize,
                    1 => 0x2d0 + rng.below(4) as usize,
                    _ => rng.below(e.len() as u64) as usize,
                };
                e[at] ^= 1 + rng.below(255) as u8;
            }
            if seed % 2 == 1 {
                let k = rng.below(0x1000) as usize;
                let tail = rng.bytes(k);
                e.extend(tail);
            }
            // The editor's map writer always leaves a valid checksum; half
            // the cases leave it as edited (wrong) to show the repair.
            let valid = seed % 4 < 2;
            if valid {
                repair_checksum(&mut e);
            }
            let w = diff(&base, &e, "test");
            let file = format!("lockout.{}", w.format.extension());
            let mut m = Manifest::retail("lockout", Sha256::of(&base));
            m.package.name = "Edited".into();
            m.patches = vec![Layer {
                file: file.clone(),
                sha256: Sha256::of(&w.bytes),
            }];
            write_package(&maps, "lockout-edit", &m, &[(&file, &w.bytes)], false).unwrap();
            let mut want = e.clone();
            repair_checksum(&mut want);
            if valid {
                assert_eq!(want, e);
            }
            let mut lib = Library::open(&maps);
            let r = lib.resolve("lockout", &mut &base[..]).unwrap().unwrap();
            assert_eq!(r.id, m.map_id());
            assert_eq!(r.read_all(&mut &base[..]).unwrap(), want, "seed {seed}");
            let p = lib.packages()[0].clone();
            let r = lib.resolve_package(&p, &mut &base[..]).unwrap();
            assert_eq!(r.read_all(&mut &base[..]).unwrap(), want, "seed {seed}");
        }
    }

    #[test]
    fn image_kinds() {
        assert_eq!(image_kind(b"\x89PNG\r\n\x1a\nxx").unwrap(), "png");
        assert_eq!(image_kind(&[0xff, 0xd8, 0xff, 0xe0]).unwrap(), "jpg");
        assert!(image_kind(b"GIF89a").is_err());
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        assert!(image_kind(&big).is_err());
        big.truncate(MAX_IMAGE_BYTES as usize);
        assert!(image_kind(&big).is_ok());
    }

    #[test]
    fn active_entries() {
        let t = Temp::new();
        let maps = t.0.join("maps");
        assert_eq!(active_choice(&maps, "lockout").unwrap(), None);
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::write(
            maps.join(ACTIVE_FILE),
            "Lockout = \"old\"\nmidship = false\n",
        )
        .unwrap();
        set_active(&maps, "lockout", Some("new-one")).unwrap();
        assert_eq!(
            active_choice(&maps, "LOCKOUT").unwrap(),
            Some(Some("new-one".into()))
        );
        assert_eq!(active_choice(&maps, "midship").unwrap(), Some(None));
        let text = std::fs::read_to_string(maps.join(ACTIVE_FILE)).unwrap();
        assert!(!text.contains("old"), "{text}");
        set_active(&maps, "lockout", None).unwrap();
        assert_eq!(active_choice(&maps, "lockout").unwrap(), None);
        assert_eq!(active_choice(&maps, "midship").unwrap(), Some(None));
        assert!(set_active(&maps, "lock.out", Some("x")).is_err());
        assert!(set_active(&maps, "lockout", Some("cache")).is_err());
        std::fs::write(maps.join(ACTIVE_FILE), "not toml [").unwrap();
        assert!(set_active(&maps, "lockout", Some("x")).is_err());
        assert_eq!(
            std::fs::read_to_string(maps.join(ACTIVE_FILE)).unwrap(),
            "not toml ["
        );
    }
}
