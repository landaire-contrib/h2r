//! `h2 build`, `h2 run` and `h2 serve`: the boot tracer (crates/apps/boot,
//! docs/runtime/BOOT.md); `serve` is `run` with the agent API on and no
//! building (docs/runtime/agent-api.md).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

#[cfg(not(feature = "desktop"))]
const USAGE: &str = "usage:
  h2 serve <disc.iso> --system-link --host-light [--host-bots] [--socket PATH]
           [--out DIR] [--data DIR] [--fresh-data] [--settings DIR] [--film FILE]

--out defaults to the temporary directory's h2boot and must contain the
prebuilt game module. Use the same H2_OPT the module was built with.
--data sets the runtime data directory.
--socket sets the agent API socket (default H2_AGENT or the agent's default).
--host-light skips steady-state world rendering, uses a silent sound clock,
disables kernel tracing and paces to real time. Requires --system-link.
--host-bots enables bot support on a light host.";

#[cfg(feature = "desktop")]
const USAGE: &str = "usage:
  h2 build <disc.iso|default.xbe> [--out DIR] [--jobs N] [--check] [--tier fast|clang] [--arch TARGET]
           [--profile host --coverage FILE]
  h2 run   <disc.iso> [--out DIR] [--tier auto|fast|clang] [--data DIR] [--fresh-data] [--slices N] [--transfers N] [--no-stopgaps] [--no-d3d] [--no-bink] [--no-dsound] [--no-xapi] [--no-mappack] [--no-halo1]
           [--movies DIR | --no-movies] [--settings DIR] [--film FILE]
  h2 serve <disc.iso> [--socket PATH] [--system-link [--host-light [--host-bots]]] [run's options]

--out defaults to $TMPDIR/h2boot (generated C, objects, and the game module).
--arch selects a single build target and overrides H2_EMIT_TARGET. Use
  x86_64-linux to cross-build libh2game.so for a Linux dedicated host.
--profile host uses the light host audit's address list (--coverage) to
  select clang's native owners. Use a separate --out directory.
--tier (or H2_TIER): `build` builds clang's optimised module by default, or
  with `fast` the Cranelift fast tier in --out/fast/ in seconds
  (docs/lifter/cranelift-tier.md). `run` defaults to `auto`: clang's module
  when it is current, else the fast tier (built first if need be) with
  clang's build started in the background at low priority (log
  --out/clang-build.log), used from the next start. `fast` runs the fast
  tier only; `clang` builds clang's in the foreground, as before tiers.
  `build --background` is that background build: low priority, and gone at
  once when another holds --out.
--data defaults to ~/Library/Application Support/h2recomp/data (~/.local/share/h2recomp/data on Unix other than Apple's), or $H2_DATA (host root of the hard-disk partitions).
--fresh-data empties it first: the cache partitions, profiles and saves a
  run leaves behind change what the next run does, so runs compared frame
  for frame need it (and must not share a data directory while running).
  It refuses to when the disc, image or --out lies inside what it would empty.
`run` installs a mappack.iso found next to the disc into --data (once; a warning
never stops the run). --no-mappack or H2_MAPPACK=0 skips it (docs/game/map-pack.md).
`run` and `serve` convert a halo1.iso found next to the disc into --data when
it is new or the converter changed (a warning never stops the run);
--no-halo1 or H2_HALO1=0 skips it (docs/halo1/campaign.md).
The intro, attract and credits movies are skipped: `run` and `serve` are
headless, and a level boot (H2_LEVEL) has no intro. --movies DIR plays those
`h2 movies` installed there for this disc. The run logs which
(`movies: playback (...)` or `movies: skip (...)`; docs/audio/bink-hle.md).
`run` builds first when the dylib is missing or was built from another
image or generator (its .stamp). `build --check` builds nothing: it exits 0
when --out's dylib is current for this h2 and the image, 3 otherwise.
--settings DIR gives `run` and `serve` our settings store, read from
  DIR/settings.toml (written when a menu changes a setting), the H2_*
  variables and the defaults, as `h2 play` builds it from --data. Without it a
  headless run has no store, and the menus behind one (the campaign's Game
  Options page, the Video screen's rows, the Halo 1 options) stay retail.
  `--settings` with the --data directory uses the same file `h2 play` does.
`serve` runs headless with the agent API listening on --socket (default
$H2_AGENT, else the cache directory's h2recomp/agent.sock) until the game
exits, for `h2 agent` and other clients (docs/runtime/agent-api.md). It never
builds: when --out's dylib isn't current it stops and says so. Use the same
H2_OPT the image was built with.
--system-link enables the XNet sockets for a headless LAN game. Choose System
  Link in the normal menus. No Live account or service URL is required.
--host-light skips steady-state world rendering, uses a silent sound clock,
  disables kernel tracing and paces to real time. Requires --system-link.
--host-bots enables bot support on a light host.";

pub fn run(cmd: &str, args: Vec<String>) -> Result<()> {
    let tmp = std::env::temp_dir();
    let mut out = tmp.join("h2boot");
    let mut data = content::store::default_data_root();
    #[cfg(feature = "desktop")]
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let serve = cmd == "serve";
    // A server runs until the game exits.
    let mut slices = if serve { u64::MAX } else { 100_000u64 };
    let mut socket = None;
    let mut system_link = false;
    let mut host_light = false;
    let mut host_bots = false;
    let mut film = None;
    let mut transfers = 50_000_000u64;
    let mut disc = None;
    let mut stopgaps = true;
    let mut d3d = true;
    let mut bink = true;
    let mut movies = boot::movies_glue::Movies::Auto;
    let mut dsound = true;
    let mut xapi = true;
    let mut fresh = false;
    #[cfg(feature = "desktop")]
    let mut check = false;
    #[cfg(feature = "desktop")]
    let mut build_arch = None;
    #[cfg(feature = "desktop")]
    let mut slim = false;
    #[cfg(feature = "desktop")]
    let mut coverage = None;
    #[cfg(feature = "desktop")]
    let mut no_mappack = false;
    #[cfg(feature = "desktop")]
    let mut no_halo1 = false;
    let mut tier = None;
    #[cfg(feature = "desktop")]
    let mut background = false;
    let mut settings_dir = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = PathBuf::from(val()?),
            "--data" => data = PathBuf::from(val()?),
            #[cfg(feature = "desktop")]
            "--jobs" => jobs = val()?.parse()?,
            "--fresh-data" => fresh = true,
            #[cfg(feature = "desktop")]
            "--check" if cmd == "build" => check = true,
            #[cfg(feature = "desktop")]
            "--arch" if cmd == "build" => {
                let arches = emit::compile::architectures(Some(&val()?))?;
                let [arch] = arches.as_slice() else { bail!("h2 build --arch requires one target") };
                build_arch = Some(*arch);
            }
            #[cfg(feature = "desktop")]
            "--profile" if cmd == "build" => {
                anyhow::ensure!(val()? == "host", "expected --profile host");
                slim = true;
            }
            #[cfg(feature = "desktop")]
            "--coverage" if cmd == "build" => coverage = Some(PathBuf::from(val()?)),
            #[cfg(feature = "desktop")]
            "--background" if cmd == "build" => background = true,
            "--tier" => tier = Some(boot::tier::Choice::parse(&val()?)?),
            "--system-link" => system_link = true,
            "--host-light" => host_light = true,
            "--film" if cmd != "build" => film = Some(PathBuf::from(val()?)),
            "--host-bots" => host_bots = true,
            "--socket" if serve => socket = Some(PathBuf::from(val()?)),
            "--settings" if cmd != "build" => settings_dir = Some(PathBuf::from(val()?)),
            "--no-stopgaps" => stopgaps = false,
            "--no-d3d" => d3d = false,
            "--no-bink" => bink = false,
            #[cfg(feature = "desktop")]
            "--movies" => movies = boot::movies_glue::Movies::Dir(PathBuf::from(val()?)),
            #[cfg(not(feature = "desktop"))]
            "--movies" => bail!("movie playback needs the desktop feature"),
            "--no-movies" => movies = boot::movies_glue::Movies::Off,
            "--no-dsound" => dsound = false,
            "--no-xapi" => xapi = false,
            #[cfg(feature = "desktop")]
            "--no-mappack" => no_mappack = true,
            #[cfg(feature = "desktop")]
            "--no-halo1" => no_halo1 = true,
            #[cfg(not(feature = "desktop"))]
            "--no-mappack" | "--no-halo1" => {}
            "--slices" => slices = val()?.parse()?,
            "--transfers" => transfers = val()?.parse()?,
            "-h" | "--help" => bail!("{USAGE}"),
            _ if disc.is_none() => disc = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let mut live = headless_live_config(serve, system_link)?;
    check_host_options(serve, system_link, host_light, host_bots)?;
    let publisher = if host_light {
        leased_publisher(&mut live, |key| std::env::var_os(key))?
    } else {
        None
    };
    if host_light && (!d3d || !bink || !dsound || !xapi) {
        bail!("--host-light needs the guest D3D, Bink, DirectSound and XAPI shims");
    }
    if host_light && matches!(movies, boot::movies_glue::Movies::Dir(_)) {
        bail!("--host-light skips movies; remove --movies");
    }
    let disc = disc
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .context(USAGE)?;
    let xbe = boot::read_xbe(&disc)?;
    use boot::tier::{Choice, Tier};
    let choice = match tier {
        Some(t) => t,
        None => Choice::from_env()?,
    };
    #[cfg(feature = "desktop")]
    if cmd == "build" {
        // `build` builds one tier: clang's unless asked for the fast one.
        let fast = choice == Choice::Fast;
        let arch = match build_arch {
            Some(arch) => arch,
            None => emit::compile::Arch::target()?,
        };
        anyhow::ensure!(slim == coverage.is_some(), "--profile host and --coverage FILE must be used together");
        anyhow::ensure!(!slim || (!fast && !background), "--profile host requires the foreground clang tier");
        anyhow::ensure!(
            !slim || std::env::var_os(boot::host_audit::MODULE_ENV).is_none_or(|v| v.is_empty()),
            "--profile host conflicts with H2_HOST_MODULE"
        );
        let coverage = coverage.as_deref().map(boot::host_coverage::Coverage::read).transpose()?;
        let lib = if fast {
            boot::tier::check_fast_target(arch)?;
            boot::tier::fast_dylib(&out)
        } else {
            out.join(arch.module_file_name())
        };
        if check {
            let current = if let Some(coverage) = &coverage {
                boot::slim_current_for(&xbe, &lib, arch, coverage)
            } else if fast {
                boot::tier::fast_current(&xbe, &lib)
            } else {
                boot::dylib_current_for(&xbe, &lib, arch)
            };
            eprintln!(
                "h2 build --check: {} is {}",
                lib.display(),
                if current { "current" } else { "out of date" }
            );
            std::process::exit(if current { 0 } else { 3 });
        }
        if let Some(coverage) = &coverage {
            boot::build_slim(&xbe, &out, jobs, arch, coverage)?;
        } else if fast {
            boot::tier::build_fast_for(&xbe, &out, jobs, arch)?;
        } else {
            let _lock = if background {
                match background_lock(&out)? {
                    Some(l) => Some(l),
                    None => {
                        eprintln!("h2 build --background: another build holds {}; leaving it to that one", out.display());
                        return Ok(());
                    }
                }
            } else {
                None
            };
            boot::build_for(&xbe, &out, jobs, arch)?;
        }
        return Ok(());
    }
    // `serve` never builds, and runs clang's module unless told otherwise.
    let choice = if serve && tier.is_none() && std::env::var_os("H2_TIER").is_none() { Choice::Clang } else { choice };
    #[cfg(unix)]
    let mut between_slices = None;
    let lib = if serve {
        let plan = if host_light {
            boot::tier::plan_light(&xbe, &out, choice)
        } else {
            boot::tier::plan(&xbe, &out, choice)
        };
        if plan.build {
            bail!(
                "h2 serve: {} isn't built for this image and this h2; build it with `h2 build{}` first (serve never builds)",
                plan.dylib.display(),
                if plan.tier == Tier::Fast { " --tier fast" } else { "" }
            );
        }
        let path = socket.unwrap_or_else(agent_proto::default_socket);
        // SAFETY: set before the run starts any thread; boot reads it once.
        unsafe { std::env::set_var(agent_proto::ENV, &path) };
        #[cfg(unix)]
        {
            between_slices = stop_on_signal(path, host_light)?;
        }
        plan.dylib
    } else {
        #[cfg(feature = "desktop")]
        {
            boot::tier::prepare(&xbe, &disc, &out, choice, jobs, true)?.dylib
        }
        #[cfg(not(feature = "desktop"))]
        bail!("the host CLI requires h2 serve");
    };
    if fresh {
        for d in fresh_data_dirs(&data, &[&disc, &out])? {
            std::fs::remove_dir_all(&d).with_context(|| format!("emptying {}", d.display()))?;
        }
    }
    #[cfg(feature = "desktop")]
    if crate::mappack_cmd::auto_enabled(no_mappack) {
        crate::mappack_cmd::auto_install(&disc, &data);
    }
    #[cfg(feature = "desktop")]
    if crate::h1_import::auto_enabled(no_halo1) {
        crate::h1_import::auto_import(&disc, &data);
    }
    let host = boot::Host {
        film_out: film,
        #[cfg(unix)]
        between_slices,
        host_light,
        host_bots,
        settings: settings_dir.map(|d| settings_store(cmd, &d)),
        ..boot::Host::default()
    };
    let r = boot::run_hosted_with_publisher(
        &xbe,
        &lib,
        &boot::RunOpts {
            disc,
            data_root: data,
            max_slices: slices,
            transfers_per_slice: transfers,
            stopgaps,
            trace: Some(out.join("kernel-trace.tsv")),
            d3d,
            bink,
            dsound,
            xapi,
            movies,
        },
        host,
        &live,
        publisher,
    );
    // The kernel's storage may outlive the run (or never drop): store its
    // pending write metadata now. Signals and other abnormal exits fall
    // under the bounded loss in docs/runtime/storage.md "Write metadata".
    vfs::flush_pending_metadata();
    let r = r?;
    for l in &r.kernel_log {
        println!("DbgPrint: {}", l.trim_end());
    }
    println!(
        "outcome: {}",
        r.outcome.chars().take(400).collect::<String>()
    );
    println!("stop: {}", r.stop);
    println!("last eip: {:#010x}", r.eip);
    Ok(())
}

fn check_host_options(serve: bool, system_link: bool, light: bool, bots: bool) -> Result<()> {
    if light && !(serve && system_link) {
        bail!("--host-light requires h2 serve --system-link");
    }
    if bots && !light {
        bail!("--host-bots requires --host-light");
    }
    Ok(())
}

fn leased_publisher(
    config: &mut boot::LiveConfig,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Option<boot::HostPublisher>> {
    let values = ["H2_HOST_TOKEN", "H2_HOST_LIVE_URL", "H2_HOST_LEASE_JSON"].map(env);
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let [Some(token), Some(url), Some(json)] = values.map(|v| v.and_then(|s| s.into_string().ok()))
    else {
        bail!("host publication requires H2_HOST_TOKEN, H2_HOST_LIVE_URL and H2_HOST_LEASE_JSON");
    };
    anyhow::ensure!(
        live_proto::bearer_allowed(&url),
        "host publication requires HTTPS or loopback HTTP"
    );
    let lease = serde_json::from_str(&json).context("invalid H2_HOST_LEASE_JSON")?;
    let publisher = boot::HostPublisher::new(lease, token)?;
    config.server_url = Some(url);
    Ok(Some(publisher))
}

/// System Link only needs the XNet boundary. Leave accounts and HTTP unset.
fn headless_live_config(serve: bool, system_link: bool) -> Result<boot::LiveConfig> {
    if system_link && !serve {
        bail!("--system-link requires h2 serve");
    }
    Ok(boot::LiveConfig {
        enabled: system_link,
        ..Default::default()
    })
}

/// `--settings DIR`: our settings store, built as `h2 play` builds its own
/// (`boot::setting_defs::load`): `DIR/settings.toml`, then the environment,
/// then each default. A bad value is logged and the next source used.
fn settings_store(cmd: &str, dir: &Path) -> Arc<settings::Store> {
    let (store, problems) = boot::setting_defs::load(dir, &[]);
    for p in problems {
        eprintln!("h2 {cmd}: {p}");
    }
    eprintln!("h2 {cmd}: settings from {}", settings::path_in(dir).display());
    Arc::new(store)
}

/// `build --background`: lower this process's priority (clang's processes
/// inherit it) and take `out`'s background-build lock, held until the
/// returned file drops; `None` when another build holds it.
#[cfg(feature = "desktop")]
fn background_lock(out: &Path) -> Result<Option<std::fs::File>> {
    std::fs::create_dir_all(out)?;
    let path = out.join("clang-build.lock");
    let f = std::fs::File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match f.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(error).with_context(|| format!("locking {}", path.display()));
        }
    }
    #[cfg(unix)]
    // SAFETY: plain priority change of this process.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) } != 0 {
        eprintln!("warning: build --background: lowering priority: {}", std::io::Error::last_os_error());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
        };
        // Child processes inherit a below-normal priority class by default.
        if unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) } == 0 {
            eprintln!("warning: build --background: lowering priority: {}", std::io::Error::last_os_error());
        }
    }
    Ok(Some(f))
}

/// The directories `--fresh-data` empties (`<data>/disk`, `<data>/metadata`,
/// those that exist), or an error, deleting nothing, when one of them is or
/// contains a path in `keep` (the disc or image, the build output).
fn fresh_data_dirs(data: &Path, keep: &[&Path]) -> Result<Vec<PathBuf>> {
    // Both spellings: as given (made absolute) and with links resolved.
    let forms = |p: &Path| -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Ok(a) = std::path::absolute(p) {
            v.push(a);
        }
        if let Ok(c) = p.canonicalize() {
            v.push(c);
        }
        v
    };
    let mut out = Vec::new();
    for sub in ["disk", "metadata"] {
        let d = data.join(sub);
        if !d.exists() {
            continue;
        }
        for k in keep {
            if forms(&d)
                .iter()
                .any(|d| forms(k).iter().any(|k| k.starts_with(d)))
            {
                bail!(
                    "--fresh-data would delete {} with {} in it; use another --data",
                    d.display(),
                    k.display()
                );
            }
        }
        out.push(d);
    }
    Ok(out)
}

/// Let a light host finish its recording and upload on the game thread's
/// normal teardown path. Other servers keep their immediate signal exit.
#[cfg(unix)]
fn stop_on_signal(socket: PathBuf, graceful: bool) -> Result<Option<boot::BetweenSlices>> {
    use std::sync::atomic::{AtomicBool, Ordering};

    let stopping = graceful.then(|| Arc::new(AtomicBool::new(false)));
    let signal_stop = stopping.clone();
    // SAFETY: plain sigset operations on a local, and a mask change on
    // this thread before any other thread exists.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        let r = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        if r != 0 {
            bail!(
                "h2 serve: blocking SIGINT/SIGTERM: {}",
                std::io::Error::from_raw_os_error(r)
            );
        }
        set
    };
    std::thread::Builder::new()
        .name("serve-signals".into())
        .spawn(move || {
            let mut sig = 0;
            // SAFETY: waits on the signals blocked above.
            if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                return;
            }
            eprintln!("h2 serve: signal {sig}: stopping");
            if let Some(stop) = signal_stop {
                stop.store(true, Ordering::Release);
            } else {
                let _ = std::fs::remove_file(&socket);
                vfs::flush_pending_metadata();
                std::process::exit(128 + sig);
            }
        })?;
    let Some(stop) = stopping else {
        return Ok(None);
    };
    Ok(Some(Box::new(move |_| !stop.load(Ordering::Acquire))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-boot-cmd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn light_host_and_its_bots_are_explicit_opt_ins() {
        assert!(check_host_options(true, true, true, false).is_ok());
        assert!(check_host_options(true, true, true, true).is_ok());
        assert!(check_host_options(true, true, false, false).is_ok());
        assert!(check_host_options(true, false, true, false).is_err());
        assert!(check_host_options(false, true, true, false).is_err());
        assert!(check_host_options(true, true, false, true).is_err());
    }

    #[test]
    fn system_link_enables_accountless_networking_only_on_request() {
        let lan = headless_live_config(true, true).unwrap();
        assert!(lan.enabled);
        assert!(lan.accounts_path.is_none() && lan.server_url.is_none() && !lan.offer_new_account);
        assert!(!headless_live_config(true, false).unwrap().enabled);
        assert!(!headless_live_config(false, false).unwrap().enabled);
        for cmd in ["run", "build"] {
            let error = run(cmd, vec!["--system-link".into()]).unwrap_err();
            assert_eq!(error.to_string(), "--system-link requires h2 serve");
        }
    }

    #[test]
    fn leased_publication_requires_the_complete_child_environment() {
        let mut config = headless_live_config(true, true).unwrap();
        assert!(leased_publisher(&mut config, |_| None).unwrap().is_none());
        for missing in ["H2_HOST_TOKEN", "H2_HOST_LIVE_URL", "H2_HOST_LEASE_JSON"] {
            assert!(
                leased_publisher(&mut config, |key| (key != missing).then(|| "value".into()))
                    .is_err()
            );
        }
        assert!(config.server_url.is_none() && config.accounts_path.is_none());
    }

    #[test]
    fn fresh_data_does_not_delete_its_input() {
        let root = scratch("input");
        let data = root.join("data");
        let image = data.join("disk/Partition1/synthetic.xbe");
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        std::fs::write(&image, b"XBEH").unwrap();
        std::fs::create_dir_all(data.join("metadata")).unwrap();
        let out = root.join("out");
        assert!(fresh_data_dirs(&data, &[&image, &out]).is_err());
        // The image inside metadata, or the data dir's partitions as --out.
        assert!(
            fresh_data_dirs(&data, &[&data.join("metadata/x.iso")]).is_err(),
            "inside metadata, by its spelling"
        );
        assert!(
            fresh_data_dirs(&data, &[&data.join("disk")]).is_err(),
            "equal"
        );
        // Spelled with `..` it is still inside.
        assert!(
            fresh_data_dirs(
                &data,
                &[&data.join("metadata/../disk/Partition1/synthetic.xbe")]
            )
            .is_err()
        );
        assert!(image.exists(), "nothing was deleted");
        let elsewhere = root.join("halo2.iso");
        std::fs::write(&elsewhere, b"").unwrap();
        let dirs = fresh_data_dirs(&data, &[&elsewhere, &out]).unwrap();
        assert_eq!(dirs, [data.join("disk"), data.join("metadata")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `--settings DIR` reads DIR's settings.toml, and the store has what
    /// the settings-gated menus look for (the skulls' settings, which put
    /// the Game Options page on the plan).
    #[test]
    fn settings_come_from_the_given_directory() {
        let dir = scratch("settings");
        std::fs::write(settings::path_in(&dir), "[graphics]\nfov = 100\n").unwrap();
        let store = settings_store("serve", &dir);
        assert_eq!(store.handle::<u32>(boot::setting_defs::FOV.key).get(), 100);
        assert!(
            store.def(boot::skulls::SKULLS[0].key).is_some(),
            "the skulls' settings are on the store"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_data_skips_what_is_not_there() {
        let root = scratch("empty");
        assert!(
            fresh_data_dirs(&root.join("data"), &[&root.join("halo2.iso")])
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
