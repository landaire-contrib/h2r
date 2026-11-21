//! Source hashes (FNV-1a over file names and contents), so cached results
//! are dropped whenever what produced them can have changed:
//!
//! - `UNREAD_SOURCE_HASH`: the analyze crate and the fallback, for
//!   the cached unread-target set (fallback.rs).
//! - `ANALYSIS_SOURCE_HASH`: xbe, analyze, Cargo.lock and the rustc
//!   version, for the cached analysis (lib.rs, `analysis_cache`).
//! - `GENERATOR_SOURCE_HASH`: everything that turns an XBE into the native
//!   module (xbe, analyze, ir, lift, the IR passes, emit and its C, x87, the
//!   runtime ABI header and the boot bridge C), for the dylib's build stamp
//!   (lib.rs).
//! - `FAST_TIER_SOURCE_HASH`: the same plus crates/recompiler/clif and Cargo.lock, for
//!   the Cranelift fast tier's stamp (tier.rs).

use std::path::{Path, PathBuf};

fn walk(dir: &Path, exts: &[&str], files: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, exts, files);
        } else if p.extension().is_some_and(|x| exts.iter().any(|e| x == *e)) {
            files.push(p);
        }
    }
}

/// Hash every file with one of `exts` under `dirs`, plus `extra` files.
fn hash(dirs: &[&str], exts: &[&str], extra: &[&str]) -> u64 {
    let mut files = Vec::new();
    for d in dirs {
        println!("cargo:rerun-if-changed={d}");
        walk(Path::new(d), exts, &mut files);
    }
    files.extend(extra.iter().map(PathBuf::from));
    files.sort();
    files.dedup();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        for &b in f.to_string_lossy().as_bytes().iter().chain(&std::fs::read(f).unwrap_or_default()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// The compiler's version line: a new rustc may compile the analyzer
/// differently, so it is part of the analysis cache's key.
fn rustc_version() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    std::process::Command::new(rustc)
        .arg("-vV")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn main() {
    // What discovery's result depends on: the image parser and the
    // analyzer, their dependencies' versions (Cargo.lock) and the compiler.
    let mut analysis = hash(&["../../formats/xbe/src", "../../recompiler/analyze/src"], &["rs"], &["../../../Cargo.lock"]);
    for b in rustc_version().bytes() {
        analysis ^= b as u64;
        analysis = analysis.wrapping_mul(0x100_0000_01b3);
    }
    println!("cargo:rustc-env=ANALYSIS_SOURCE_HASH={analysis:016x}");
    let unread = hash(&["../../recompiler/analyze/src"], &["rs"], &["src/fallback.rs"]);
    println!("cargo:rustc-env=UNREAD_SOURCE_HASH={unread:016x}");
    // The hook catalog changes lifted owner boundaries on both tiers.
    // Include the gate and inherited HUD return sites as well as the list.
    let entry_sources = ["src/render_entries.rs", "../../hooking/guest-layout/src/points.rs",
        "../../hooking/guest-layout/src/points/render_aspect.rs",
        "../../hooking/guest-layout/src/points/scene_effects.rs"];
    let generator = hash(
        &["../../formats/xbe/src", "../../recompiler/analyze/src", "../../recompiler/ir/src", "../../recompiler/lift/src", "../../recompiler/opt/src", "../../recompiler/emit/src", "../../recompiler/emit/c", "../../recompiler/x87/src", "../../runtime/runtime/include", "c"],
        &["rs", "c", "h"],
        &entry_sources,
    );
    println!("cargo:rustc-env=GENERATOR_SOURCE_HASH={generator:016x}");
    // The fast tier's module (tier.rs): the generator's sources plus
    // crates/recompiler/clif and Cargo.lock (the Cranelift version).
    let mut fast_sources = entry_sources.to_vec();
    fast_sources.push("../../../Cargo.lock");
    let fast = hash(
        &["../../formats/xbe/src", "../../recompiler/analyze/src", "../../recompiler/ir/src", "../../recompiler/lift/src", "../../recompiler/opt/src", "../../recompiler/emit/src", "../../recompiler/emit/c", "../../recompiler/x87/src", "../../runtime/runtime/include", "c", "../../recompiler/clif/src", "../../recompiler/clif/c"],
        &["rs", "c", "h"],
        &fast_sources,
    );
    println!("cargo:rustc-env=FAST_TIER_SOURCE_HASH={fast:016x}");
}
