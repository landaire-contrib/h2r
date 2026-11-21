//! `h2 coop test`: co-op over Live on one Mac with one command
//! (docs/live/live-coop.md section 12). A Live server in this process, an
//! account per game (friends with the host), and two (or more) `h2 play`
//! games with their own data, Live directory, XNet ports and agent socket.
//! Their co-op lines go into one timestamped log, and the checklist of
//! section 11.3 is printed from it when the games close.
//!
//! `h2 coop verdict <log>` prints the checklist of a log again.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::coop_drive;
use crate::coop_verdict::{self, Roles};
use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{Client, FriendAction, LIVE_DIR_ENV, friend_run, status_of};

const USAGE: &str = "usage:
  h2 coop test [--disc ISO] [--players N] [--auto] [--late] [--drive] [--root DIR] [--out DIR]
               [--ip ADDR] [--port N] [--server-port N] [--pad host|guest]
               [--extras] [--fresh] [--dry-run] [-- <more h2 play arguments>]
  h2 coop verdict <coop.log> [--auto]

Co-op over Live on this Mac (docs/live/live-coop.md section 12). Starts a
Live server in this process on 127.0.0.1:--server-port (8075), keeping its
data under --root; makes an account per game (Alpha hosts; Bravo, then
Charlie, Delta and on to Papa, join) that are friends with the host; and opens one
`h2 play` window per game (--players, 2 by default, at most 16; past 2 the host gets
H2_COOP_MAX_PLAYERS), each with
its own data and Live directory under --root, its own XNet ports (--port,
31000, then 10 up per game, on --ip, this Mac's LAN address by default) and
its own agent socket (`h2 agent --socket <path> status`). The host's window
opens first (its first start may build the game into --out); the others
once it is signed in, so they never build at the same time. Each window's
title names its game and role (Halo 2 — Alpha (host)), and the windows
sit side by side across the main screen, the host leftmost.

Input: each window takes the keyboard while it has focus. The pads go to
the host (--pad guest gives them to the first guest; the others have
none). The guests play without sound.

Once the host is at its main menu (asked over its agent socket), one line
says what to press next. The menus are yours: on every window, Xbox Live
(that signs the game in, so the host sees the guests online); on the host,
Co-op Campaign, a level and difficulty, then Start Game.

--auto: the host invites the guests to its lobby by itself, and each guest
accepts by itself, as if pressed through the friends list. Without it,
invite from the host's lobby (Y, the friends list) and accept on the guest.

--late (with --auto, 3 players or more): the last guest accepts 120 s after
its invitation (H2_COOP_AUTO_ACCEPT_DELAY), so it arrives while the host is
in the level and joins in progress (docs/live/live-coop.md section 14); the
checklist then says whether and at which tick it joined.

--drive (with --auto): nobody presses anything. Each game prints the menu
screen its first player is on (H2_UI_WATCH=1), and this process presses
through the menus over each game's agent socket, one press at a time, each
on a screen or a line it waited for: every game signs in to Xbox Live; the
host picks Co-op Campaign and, once every lobby guest has joined, Start
Game; each guest goes back to the main menu for the invitation and, if the
game hasn't taken it 20 s after accepting, joins through the Custom Game
Browser (the late guest no sooner than 120 s after its invitation). Every
press is a `coop-drive:` line in coop.log (docs/live/live-coop.md 12.6).

Every game's whole stderr goes to <root>/logs/<time>/<gamertag>.log, and
their co-op lines (coop-live, coop-probe, coop-auto, skulls, session) to
coop.log there, with the time since the start. When the last window
closes (or after Ctrl-C), the checklist prints: each step of section 11.3
as seen or not seen, where it stopped, and lines that look like trouble.
`h2 coop verdict` prints it again from a coop.log.

--root defaults to coop-test under the application data directory, so
profiles and accounts last between runs; --fresh forgets the server and the
accounts (not the game data). --extras installs the map pack and Halo 1
into each game's data as `h2 play` does (off: co-op needs neither).
--dry-run prints what would run and starts nothing.";

/// Gamertags in game order: the host first.
pub const TAGS: [&str; 16] = [
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliett",
    "Kilo", "Lima", "Mike", "November", "Oscar", "Papa",
];
const DEFAULT_PORT: u16 = 31000;
const PORT_STEP: u16 = 10;
const DEFAULT_SERVER_PORT: u16 = 8075;
/// How long `--late`'s last guest waits after its invitation: time to
/// pick a level, start it and load in.
pub const LATE_SECS: u32 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Test(Opts),
    Verdict { log: PathBuf, auto: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opts {
    pub disc: PathBuf,
    pub players: usize,
    pub auto: bool,
    /// The last guest accepts [`LATE_SECS`] after its invitation.
    pub late: bool,
    /// Press through the menus ([`crate::coop_drive`]).
    pub drive: bool,
    pub root: PathBuf,
    pub out: Option<PathBuf>,
    pub ip: Option<Ipv4Addr>,
    pub port: u16,
    pub server_port: u16,
    pub pad_on_guest: bool,
    pub extras: bool,
    pub fresh: bool,
    pub dry_run: bool,
    pub play_args: Vec<String>,
}

/// Parses `h2 coop ...` using the disc and application data environment.
pub fn parse(args: Vec<String>, env: impl Fn(&str) -> Option<String>) -> Result<Cmd> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("test") => {}
        Some("verdict") => {
            let mut log = None;
            let mut auto = false;
            for a in it {
                match a.as_str() {
                    "--auto" => auto = true,
                    _ if log.is_none() && !a.starts_with("--") => log = Some(PathBuf::from(a)),
                    _ => bail!("unexpected {a:?}\n\n{USAGE}"),
                }
            }
            let log = log.with_context(|| format!("verdict takes a coop.log\n\n{USAGE}"))?;
            return Ok(Cmd::Verdict { log, auto });
        }
        Some("-h" | "--help") | None => bail!("{USAGE}"),
        Some(other) => bail!("unknown `h2 coop {other}`\n\n{USAGE}"),
    }
    let mut disc = env("H2_DISC").map(PathBuf::from);
    let mut players = 2;
    let mut auto = false;
    let mut late = false;
    let mut drive = false;
    let mut root = None;
    let mut out = None;
    let mut ip = None;
    let mut port = DEFAULT_PORT;
    let mut server_port = DEFAULT_SERVER_PORT;
    let mut pad_on_guest = false;
    let mut extras = false;
    let mut fresh = false;
    let mut dry_run = false;
    let mut play_args = Vec::new();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--disc" => disc = Some(PathBuf::from(value("--disc")?)),
            "--players" => {
                let v = value("--players")?;
                players = v
                    .parse()
                    .ok()
                    .filter(|n| (2..=TAGS.len()).contains(n))
                    .with_context(|| format!("--players {v}: 2 to {}", TAGS.len()))?;
            }
            "--auto" => auto = true,
            "--late" => late = true,
            "--drive" => drive = true,
            "--root" => root = Some(PathBuf::from(value("--root")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--ip" => {
                let v = value("--ip")?;
                ip = Some(
                    v.parse()
                        .with_context(|| format!("--ip {v}: an IPv4 address"))?,
                );
            }
            "--port" => {
                let v = value("--port")?;
                port = v.parse().with_context(|| format!("--port {v}: a port"))?;
            }
            "--server-port" => {
                let v = value("--server-port")?;
                server_port = v
                    .parse()
                    .with_context(|| format!("--server-port {v}: a port"))?;
            }
            "--pad" => match value("--pad")?.as_str() {
                "host" => pad_on_guest = false,
                "guest" => pad_on_guest = true,
                v => bail!("--pad {v}: host or guest"),
            },
            "--extras" => extras = true,
            "--fresh" => fresh = true,
            "--dry-run" => dry_run = true,
            "--" => {
                play_args.extend(it.by_ref());
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ => bail!("unexpected {a:?}\n\n{USAGE}"),
        }
    }
    let disc = disc.with_context(|| format!("no disc: --disc ISO or H2_DISC\n\n{USAGE}"))?;
    if late && (!auto || players < 3) {
        bail!("--late needs --auto and --players 3 or more (the last guest joins late)");
    }
    if drive && !auto {
        bail!("--drive needs --auto (the invitations are what it waits on)");
    }
    let last = u32::from(port) + u32::from(PORT_STEP) * (players as u32 - 1) + 6;
    if port == 0 || last > 65535 {
        bail!("--port {port}: the games need {port} to {last}");
    }
    let root = match root {
        Some(r) => r,
        None => content::store::app_data_dir(content::store::DataHost::CURRENT, |name| env(name).map(Into::into))
            .context("application data directory is unavailable; pass --root")?
            .join("coop-test"),
    };
    Ok(Cmd::Test(Opts {
        disc,
        players,
        auto,
        late,
        drive,
        root,
        out,
        ip,
        port,
        server_port,
        pad_on_guest,
        extras,
        fresh,
        dry_run,
        play_args,
    }))
}

/// One game: what it is called, where it keeps things, and how it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub tag: String,
    pub host: bool,
    pub data: PathBuf,
    pub live: PathBuf,
    pub xnet: SocketAddrV4,
    pub agent: PathBuf,
    /// After `h2`.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The games of `opts` against the server at `server`, the XNet address
/// `ip`, with agent sockets in `sockets`.
pub fn plan(opts: &Opts, server: &str, ip: Ipv4Addr, sockets: &Path) -> Vec<Game> {
    let tags = &TAGS[..opts.players];
    let xnet: Vec<SocketAddrV4> = (0..opts.players)
        .map(|i| SocketAddrV4::new(ip, opts.port + PORT_STEP * i as u16))
        .collect();
    tags.iter()
        .enumerate()
        .map(|(i, tag)| {
            let host = i == 0;
            let dir = opts.root.join(tag.to_ascii_lowercase());
            let data = dir.join("data");
            let live = dir.join("live");
            let agent = sockets.join(format!("coop-{}.sock", tag.to_ascii_lowercase()));
            let mut args = vec![
                "play".to_string(),
                opts.disc.display().to_string(),
                "--data".into(),
                data.display().to_string(),
                "--live-server".into(),
                server.into(),
                "--gamertag".into(),
                tag.to_string(),
            ];
            args.extend([
                "--title".into(),
                window_title(tag, host),
                "--tile".into(),
                format!("{i}/{}", opts.players),
            ]);
            if let Some(out) = &opts.out {
                args.extend(["--out".into(), out.display().to_string()]);
            }
            if !host {
                args.push("--no-audio".into());
            }
            let pads = if opts.pad_on_guest { i == 1 } else { host };
            if !pads {
                args.push("--no-gamepads".into());
            }
            if !opts.extras {
                args.extend(["--no-mappack".into(), "--no-halo1".into()]);
            }
            args.extend(opts.play_args.iter().cloned());
            let peers: Vec<String> = xnet
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, a)| a.to_string())
                .collect();
            let mut env = vec![
                (LIVE_DIR_ENV.to_string(), live.display().to_string()),
                ("H2_XNET_ADDR".into(), xnet[i].to_string()),
                ("H2_XNET_LOOPBACK_PEERS".into(), peers.join(",")),
                (agent_proto::ENV.into(), agent.display().to_string()),
            ];
            if opts.drive {
                env.push(("H2_UI_WATCH".into(), "1".into()));
            }
            if host && tags.len() > 2 {
                // Past two players the host's co-op limit must let them in
                // (coop.max_players); guests take the host's limit.
                env.push(("H2_COOP_MAX_PLAYERS".into(), tags.len().to_string()));
            }
            if opts.auto {
                if host {
                    env.push(("H2_COOP_AUTO_INVITE".into(), tags[1..].join(",")));
                } else {
                    env.push(("H2_COOP_AUTO_ACCEPT".into(), tags[0].into()));
                    if opts.late && i == tags.len() - 1 {
                        env.push(("H2_COOP_AUTO_ACCEPT_DELAY".into(), LATE_SECS.to_string()));
                    }
                }
            }
            Game {
                tag: tag.to_string(),
                host,
                data,
                live,
                xnet: xnet[i],
                agent,
                args,
                env,
            }
        })
        .collect()
}

/// A game's window title: which game it is, and whether it hosts.
pub fn window_title(tag: &str, host: bool) -> String {
    format!("Halo 2 — {tag} ({})", if host { "host" } else { "guest" })
}

/// This Mac's LAN address (the one a packet to the internet would leave
/// from), else loopback.
fn lan_ip() -> Ipv4Addr {
    (|| {
        let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
        match s.local_addr().ok()? {
            std::net::SocketAddr::V4(a) => Some(*a.ip()),
            _ => None,
        }
    })()
    .unwrap_or(Ipv4Addr::LOCALHOST)
}

/// A shell line for `game` (for the plan and the user to rerun one game).
pub fn shell_line(exe: &str, game: &Game) -> String {
    let quote = |s: &str| {
        if !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:,=@".contains(c))
        {
            s.to_string()
        } else {
            format!("'{}'", s.replace('\'', r"'\''"))
        }
    };
    let mut parts: Vec<String> = game
        .env
        .iter()
        .map(|(k, v)| format!("{k}={}", quote(v)))
        .collect();
    parts.push(quote(exe));
    parts.extend(game.args.iter().map(|a| quote(a)));
    parts.join(" ")
}

/// Makes sure each game has an account on the server, kept in its own
/// Live directory, and that every guest is the host's friend. Keeps what a
/// past run made.
pub fn accounts(client: &Client, games: &[Game], all: &Path) -> Result<Vec<LocalAccount>> {
    let mut made = Vec::new();
    for g in games {
        let path = g.live.join("accounts.json");
        let mut mine = live_accounts::load(&path)?;
        let found = mine
            .iter()
            .find(|a| {
                a.gamertag.eq_ignore_ascii_case(&g.tag)
                    && a.is_on(&client.base, &client.discovery.id)
            })
            .cloned();
        let account = match found {
            Some(a) => a,
            None => {
                let (created, key) = client.create_account(&g.tag, None).map_err(|e| {
                    match status_of(&e) {
                        Some(409) => anyhow::anyhow!(
                            "the server has {} but {} has no key for it; run again with --fresh",
                            g.tag,
                            path.display()
                        ),
                        _ => e,
                    }
                })?;
                let a = LocalAccount::new(
                    created.xuid,
                    created.gamertag.as_str(),
                    &client.base,
                    &client.discovery,
                    &key,
                );
                eprintln!("coop test: made {} ({})", a.gamertag, a.xuid);
                live_accounts::upsert(&mut mine, a.clone());
                fs::create_dir_all(&g.live)?;
                live_accounts::save(&path, &mine)?;
                a
            }
        };
        made.push(account);
    }
    live_accounts::save(all, &made)?;
    let host = &games[0].tag;
    for g in &games[1..] {
        let friends: live_proto::Friends = client.get(&format!("/accounts/{host}/friends"), &[])?;
        if friends
            .friends
            .iter()
            .any(|f| f.gamertag.as_str().eq_ignore_ascii_case(&g.tag))
        {
            continue;
        }
        if !friends
            .requests_out
            .iter()
            .any(|r| r.gamertag.as_str().eq_ignore_ascii_case(&g.tag))
        {
            friend_run(client, all, Some(host), FriendAction::Add, Some(&g.tag))?;
        }
        friend_run(client, all, Some(&g.tag), FriendAction::Accept, Some(host))?;
    }
    Ok(made)
}

pub fn run(args: Vec<String>) -> Result<()> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match parse(args, env)? {
        Cmd::Verdict { log, auto } => {
            let text =
                fs::read_to_string(&log).with_context(|| format!("reading {}", log.display()))?;
            let v = coop_verdict::verdict(&text, auto)
                .with_context(|| format!("{} has no `# host` line", log.display()))?;
            print!("{}", v.render());
            Ok(())
        }
        Cmd::Test(opts) => test(opts),
    }
}

fn test(opts: Opts) -> Result<()> {
    let exe = std::env::current_exe().context("finding this h2")?;
    let ip = opts.ip.unwrap_or_else(lan_ip);
    let server = format!("http://127.0.0.1:{}", opts.server_port);
    let sockets = agent_proto::default_socket()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let games = plan(&opts, &server, ip, &sockets);
    let exe_s = exe.display().to_string();
    if opts.dry_run {
        println!(
            "server: {server} (data in {})",
            opts.root.join("server").display()
        );
        for g in &games {
            println!("{}: {}", g.tag, shell_line(&exe_s, g));
        }
        return Ok(());
    }
    if !opts.disc.is_file() {
        bail!("{} is not a file", opts.disc.display());
    }
    // Before any other thread starts (the server's), so none of them
    // takes a Ctrl-C meant for the games.
    let interrupted = watch_interrupts();
    let server_data = opts.root.join("server");
    if opts.fresh {
        let _ = fs::remove_dir_all(&server_data);
        for g in &games {
            let _ = fs::remove_dir_all(&g.live);
        }
    }
    fs::create_dir_all(&server_data)?;
    fs::create_dir_all(&sockets)?;
    let listen = SocketAddrV4::new(Ipv4Addr::LOCALHOST, opts.server_port);
    let embedded = live_server::start_embedded(
        live_server::FILE_DB,
        listen.into(),
        &server_data,
        live_server::http::Site {
            web: live_server::web_dir(),
            game_art: None,
        },
    )
    .with_context(|| {
        format!("starting the Live server on {listen}; another on that port? --server-port N")
    })?;
    let client = Client::at(&embedded.url)?;
    accounts(&client, &games, &opts.root.join("accounts.json"))?;
    let stamp = stamp(crate::live_cmd::now());
    let logs = opts.root.join("logs").join(stamp);
    fs::create_dir_all(&logs)?;
    let roles = roles(&games, opts.late);
    eprintln!(
        "coop test: Live server {} ({} store), web app at {}/",
        embedded.url, embedded.store, embedded.url
    );
    for g in &games {
        eprintln!(
            "coop test: {}{}: data {}, XNet {}, agent `h2 agent --socket {} status`",
            g.tag,
            if g.host { " (host)" } else { "" },
            g.data.display(),
            g.xnet,
            g.agent.display()
        );
    }
    eprintln!("coop test: logs in {}", logs.display());
    let merged = logs.join("coop.log");
    let mut sup = Supervisor::new(&merged, &logs, &roles)?;
    let mut drive = opts.drive.then(|| Drive::new(&games, &roles));
    sup.spawn(&exe, &games[0])?;
    eprintln!(
        "coop test: {} opening first (it builds the game if --out has none)",
        games[0].tag
    );
    let mut waiting: Vec<&Game> = games[1..].iter().collect();
    let mut said = false;
    loop {
        let ready = sup.pump(Duration::from_millis(200))?;
        if let Some(d) = &mut drive {
            d.step(&mut sup, &games)?;
        }
        // Before the stop check: games that end on Ctrl-C didn't crash.
        if interrupted.load(std::sync::atomic::Ordering::Relaxed) {
            sup.note_interrupt();
        }
        if let Some((i, why)) = sup.crash() {
            let tag = games.get(i).map_or("a game", |g| g.tag.as_str());
            eprintln!("coop test: {tag}'s game stopped ({why}); closing the others");
            waiting.clear();
            sup.close_others(i);
            // The others' last lines, then the checklist now: the stopped
            // game's window stays open until it is closed.
            sup.pump(Duration::from_millis(500))?;
            said = print_verdict(&merged, opts.auto)?;
        }
        if ready.contains(&0) && !waiting.is_empty() {
            for g in waiting.drain(..) {
                sup.spawn(&exe, g)?;
            }
            if opts.drive {
                eprintln!(
                    "coop test: --drive: pressing through every window's menus; coop-drive lines say each press"
                );
            } else {
                say_next_steps_at_the_menu(games[0].agent.clone(), next_steps(&roles, opts.auto));
            }
        }
        if sup.all_exited() {
            break;
        }
        if sup.exited(0) && !waiting.is_empty() {
            eprintln!(
                "coop test: {} closed before it signed in; not opening the others",
                games[0].tag
            );
            waiting.clear();
        }
    }
    drop(embedded);
    if !said {
        print_verdict(&merged, opts.auto)?;
    }
    println!("log: {}", merged.display());
    Ok(())
}

/// Prints the checklist the merged log makes; whether there was one.
fn print_verdict(merged: &Path, auto: bool) -> Result<bool> {
    let text = fs::read_to_string(merged)?;
    Ok(match coop_verdict::verdict(&text, auto) {
        Some(v) => {
            print!("{}", v.render());
            true
        }
        None => false,
    })
}

/// Who hosts, who joins in the lobby and who (`late`: the last guest)
/// joins in progress.
pub fn roles(games: &[Game], late: bool) -> Roles {
    let mut guests: Vec<String> = games[1..].iter().map(|g| g.tag.clone()).collect();
    let late = if late {
        guests.pop().into_iter().collect()
    } else {
        Vec::new()
    };
    Roles {
        host: games[0].tag.clone(),
        guests,
        late,
    }
}

/// What the user presses next, once the host is at its main menu.
pub fn next_steps(roles: &Roles, auto: bool) -> String {
    let host = &roles.host;
    let guests = roles.guests.join(" and ");
    let joining = if auto {
        // The game takes an accepted invitation only from its main menu's
        // reader (screen 9), which no run has seen fire; the friends
        // list's Join is the path that reaches the host (docs 12.1).
        format!(
            "on {guests}, the invitation is accepted by itself, but join from the friends \
             list: Y, {host}, Join Game"
        )
    } else {
        format!("in the lobby press Y and invite {guests}; on {guests}, accept in the friends list")
    };
    let mut line = format!(
        "coop test: next, on {guests}'s window{s}: Xbox Live (to sign in). Then on {host}'s window: \
         Xbox Live > Co-op Campaign > pick a level and difficulty; {joining}; then Start Game on {host}.",
        s = if roles.guests.len() > 1 { "s" } else { "" },
    );
    if !roles.late.is_empty() {
        let late = roles.late.join(" and ");
        line.push_str(&format!(
            " {late}: Xbox Live too; it accepts {LATE_SECS} s after the invitation, to join in \
             progress, so have {host} in the level by then."
        ));
    }
    line
}

/// How long the host may take from its sign-in to its main menu (the
/// intro, the profile screen) before the line is said anyway.
const MENU_WAIT: Duration = Duration::from_secs(600);

/// Says `line` once the game at `socket` is at its main menu (phase
/// `menu` for 2 s, `session.wait` over the agent API), or, if that can't
/// be asked, says it anyway, as what to do once it is.
fn say_next_steps_at_the_menu(socket: PathBuf, line: String) {
    let _ =
        std::thread::Builder::new()
            .name("coop-menu".into())
            .spawn(move || match wait_for_menu(&socket, MENU_WAIT) {
                Ok(()) => eprintln!("{line}"),
                Err(e) => {
                    eprintln!("{line} (once at the main menu; asking the game failed: {e:#})")
                }
            });
}

/// Waits until the game at `socket` has been in its menu for 60 ticks:
/// connects (retrying while the socket is not up yet), then
/// `session.wait {"phase":"menu","map_ticks":60}`.
pub fn wait_for_menu(socket: &Path, within: Duration) -> Result<()> {
    let deadline = Instant::now() + within;
    let mut client = loop {
        match agent_proto::Client::connect(socket) {
            Ok(c) => break c,
            Err(e) if Instant::now() >= deadline => {
                return Err(e).with_context(|| format!("connecting to {}", socket.display()));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    };
    let left = deadline.saturating_duration_since(Instant::now());
    client.set_timeout(Some(left + Duration::from_secs(5)))?;
    let ms = left.as_millis().clamp(1, 600_000) as u64;
    client
        .call(
            agent_proto::method::WAIT,
            serde_json::json!({"until": {"phase": "menu", "map_ticks": 60}, "timeout_ms": ms}),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

/// A UTC `YYYYMMDD-HHMMSS` for unix time `secs`, naming a run's logs.
pub fn stamp(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Blocks SIGINT and SIGTERM here and notes them on a thread, so Ctrl-C
/// stops the games (they get it too, from the terminal) and this process
/// still prints the checklist. The games start with the mask reset.
#[cfg(unix)]
pub(crate) fn watch_interrupts() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let flag = std::sync::Arc::new(AtomicBool::new(false));
    // SAFETY: plain libc signal-mask calls on a local, initialised set.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    let f = flag.clone();
    let _ = std::thread::Builder::new()
        .name("coop-signals".into())
        .spawn(move || {
            loop {
                let mut sig = 0;
                // SAFETY: `set` is a valid set; sigwait only writes `sig`.
                if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                    return;
                }
                if f.swap(true, Ordering::Relaxed) {
                    eprintln!("coop test: interrupted twice; exiting");
                    std::process::exit(130);
                }
            }
        });
    flag
}

#[cfg(not(unix))]
pub(crate) fn watch_interrupts() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    Default::default()
}

/// `--drive`: a [`coop_drive::Driver`] per game and its agent socket.
struct Drive {
    drivers: Vec<coop_drive::Driver>,
    clients: Vec<Option<agent_proto::Client>>,
    started: bool,
    /// Each game's agent socket is up (`agent: listening`): no press
    /// before then, so a game still building isn't pressed into "stuck".
    ready: Vec<bool>,
}

impl Drive {
    fn new(games: &[Game], roles: &Roles) -> Self {
        let drivers = games
            .iter()
            .map(|g| {
                let role = if g.host {
                    coop_drive::Role::Host {
                        lobby: roles.guests.clone(),
                    }
                } else if roles.late.contains(&g.tag) {
                    coop_drive::Role::Late
                } else {
                    coop_drive::Role::Guest
                };
                coop_drive::Driver::new(&g.tag, role)
            })
            .collect();
        Drive {
            drivers,
            clients: games.iter().map(|_| None).collect(),
            started: false,
            ready: games.iter().map(|_| false).collect(),
        }
    }

    /// Feeds the games' lines to every driver and sends the presses due.
    fn step(&mut self, sup: &mut Supervisor, games: &[Game]) -> Result<()> {
        if !self.started {
            sup.keep_lines();
            self.started = true;
        }
        for (i, at, line) in sup.take_lines() {
            let Some(tag) = games.get(i).map(|g| g.tag.as_str()) else {
                continue;
            };
            if line.starts_with("agent: listening")
                && let Some(r) = self.ready.get_mut(i)
            {
                *r = true;
            }
            for d in &mut self.drivers {
                d.line(at, tag, &line);
            }
        }
        let now = sup.now_ms();
        // Only games that are running (the guests open after the host).
        let running = sup.games.len();
        for (i, game) in games.iter().enumerate().take(running) {
            if !self.ready.get(i).copied().unwrap_or(false) {
                continue;
            }
            if let Some(p) = self.drivers[i].poll(now) {
                let line = self.drivers[i].press_line(&p);
                sup.note(i, &line)?;
                if let Err(e) = self.send(i, game, &p) {
                    sup.note(
                        i,
                        &format!("coop-drive: {}: the press failed: {e:#}", game.tag),
                    )?;
                }
            }
            for said in self.drivers[i].take_said() {
                sup.note(i, &said)?;
            }
        }
        Ok(())
    }

    fn send(&mut self, i: usize, game: &Game, p: &coop_drive::Press) -> Result<()> {
        if self.clients[i].is_none() {
            let c = agent_proto::Client::connect(&game.agent)
                .with_context(|| format!("connecting to {}", game.agent.display()))?;
            c.set_timeout(Some(Duration::from_secs(3)))?;
            self.clients[i] = Some(c);
        }
        let c = self.clients[i].as_mut().expect("connected above");
        let r = c.call(agent_proto::method::INPUT_PAD, coop_drive::pad_params(p));
        if let Err(e) = r {
            // Connect again next time.
            self.clients[i] = None;
            bail!("input.pad: {e}");
        }
        Ok(())
    }
}

enum Event {
    Line(usize, String),
    Closed(usize),
}

/// The games' processes and their logs: every line of game `i` into its
/// own log, the wanted ones into the merged log (and the terminal).
pub struct Supervisor {
    start: Instant,
    merged: File,
    logs: PathBuf,
    games: Vec<(String, Child, File, usize)>,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    ready: HashMap<usize, bool>,
    interrupted: bool,
    /// A game that stopped, and why, until [`Supervisor::crash`] says so.
    crashed: Option<(usize, String)>,
    /// A stop was said: the rest is the closing.
    stopping: bool,
    /// Every game line since [`Supervisor::take_lines`], with its game and
    /// time in ms, when kept (`--drive`).
    lines: Option<Vec<(usize, u64, String)>>,
    /// Which game lines go into the merged log ([`coop_verdict::wanted`]
    /// unless [`Supervisor::with_filter`] says otherwise).
    filter: fn(&str) -> bool,
}

/// The line the game prints when its guest thread stops (a crash or an
/// unimplemented call); its window stays open on the last frame.
const GUEST_EXITED: &str = "app: guest thread exited";

/// Game `tag`'s line saying it is signed in (after any build).
fn signed_in(tag: &str, line: &str) -> bool {
    line.starts_with("live: playing as ") && line["live: playing as ".len()..].starts_with(tag)
}

impl Supervisor {
    pub fn new(merged: &Path, logs: &Path, roles: &Roles) -> Result<Self> {
        let mut file = File::create(merged)?;
        file.write_all(roles.header().as_bytes())?;
        let (tx, rx) = mpsc::channel();
        Ok(Supervisor {
            start: Instant::now(),
            merged: file,
            logs: logs.into(),
            games: Vec::new(),
            tx,
            rx,
            ready: HashMap::new(),
            interrupted: false,
            crashed: None,
            stopping: false,
            lines: None,
            filter: coop_verdict::wanted,
        })
    }

    /// Merges the game lines `filter` keeps instead (`h2 bots test`).
    pub fn with_filter(mut self, filter: fn(&str) -> bool) -> Self {
        self.filter = filter;
        self
    }

    /// Starts `game` with `program` (the `h2` binary; tests pass a shell).
    pub fn spawn(&mut self, program: &Path, game: &Game) -> Result<()> {
        let mut cmd = Command::new(program);
        cmd.args(&game.args)
            .envs(game.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.spawn_command(cmd, &game.tag)
    }

    pub fn spawn_command(&mut self, mut cmd: Command, tag: &str) -> Result<()> {
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {tag}'s game"))?;
        let i = self.games.len();
        let streams = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ];
        let mut open = 0;
        for s in streams.into_iter().flatten() {
            open += 1;
            let tx = self.tx.clone();
            std::thread::Builder::new()
                .name(format!("coop-{tag}"))
                .spawn(move || {
                    for line in BufReader::new(s).lines() {
                        let Ok(line) = line else { break };
                        if tx.send(Event::Line(i, line)).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(Event::Closed(i));
                })?;
        }
        let log = File::create(self.logs.join(format!("{tag}.log")))?;
        self.games.push((tag.to_string(), child, log, open));
        Ok(())
    }

    /// Handles what came in for up to `wait`; the games that signed in
    /// during it.
    pub fn pump(&mut self, wait: Duration) -> Result<Vec<usize>> {
        let mut ready = Vec::new();
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = match self.rx.recv_timeout(left) {
                Ok(ev) => ev,
                Err(_) => break,
            };
            match ev {
                Event::Line(i, line) => {
                    let secs = self.start.elapsed().as_secs_f64();
                    let (tag, _, log, _) = &mut self.games[i];
                    writeln!(log, "{line}")?;
                    if (self.filter)(&line) {
                        let merged = coop_verdict::format_line(secs, tag, &line);
                        writeln!(self.merged, "{merged}")?;
                        eprintln!("{merged}");
                    }
                    if let Some(kept) = &mut self.lines {
                        kept.push((i, (secs * 1000.0) as u64, line.clone()));
                    }
                    if signed_in(tag, &line) && self.ready.insert(i, true).is_none() {
                        ready.push(i);
                    }
                    if line.contains(GUEST_EXITED) && self.crashed.is_none() {
                        self.crashed = Some((i, "its guest thread exited".into()));
                    }
                }
                Event::Closed(i) => {
                    let g = &mut self.games[i];
                    g.3 = g.3.saturating_sub(1);
                }
            }
        }
        self.merged.flush()?;
        Ok(ready)
    }

    /// Keeps every game line for [`Supervisor::take_lines`].
    pub fn keep_lines(&mut self) {
        self.lines.get_or_insert_with(Vec::new);
    }

    /// The kept lines since the last call.
    pub fn take_lines(&mut self) -> Vec<(usize, u64, String)> {
        self.lines.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Milliseconds since the start, as the merged log counts.
    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Writes one of this process's lines (`coop-drive: ...`) into the
    /// merged log and the terminal, under game `i`'s tag.
    pub fn note(&mut self, i: usize, text: &str) -> Result<()> {
        let secs = self.start.elapsed().as_secs_f64();
        let tag = self.games.get(i).map_or("drive", |g| g.0.as_str());
        let merged = coop_verdict::format_line(secs, tag, text);
        writeln!(self.merged, "{merged}")?;
        eprintln!("{merged}");
        Ok(())
    }

    pub fn exited(&mut self, i: usize) -> bool {
        let Some((_, child, _, open)) = self.games.get_mut(i) else {
            return false;
        };
        *open == 0 && !matches!(child.try_wait(), Ok(None))
    }

    pub fn all_exited(&mut self) -> bool {
        (0..self.games.len()).all(|i| self.exited(i))
    }

    /// A game that stopped while others still run, once: its guest thread
    /// exited, or its process ended in failure (a crash, a signal). A game
    /// closed normally (exit 0) is no stop; nor is anything after an
    /// interrupt or a stop already said.
    pub fn crash(&mut self) -> Option<(usize, String)> {
        if self.stopping || self.interrupted {
            return None;
        }
        if self.crashed.is_none() {
            for (i, (_, child, _, _)) in self.games.iter_mut().enumerate() {
                if let Ok(Some(status)) = child.try_wait()
                    && !status.success()
                {
                    self.crashed = Some((i, format!("it ended: {status}")));
                    break;
                }
            }
        }
        let (i, why) = self.crashed.take()?;
        if (0..self.games.len()).all(|j| j == i || self.exited(j)) {
            // Nothing else to close.
            return None;
        }
        self.stopping = true;
        Some((i, why))
    }

    /// Asks every game to close (SIGTERM).
    pub fn close_all(&mut self) {
        self.close_others(usize::MAX);
    }

    /// Kills every game still running (after [`Supervisor::close_all`] was
    /// not enough).
    pub fn kill_all(&mut self) {
        for (_, child, _, _) in &mut self.games {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
        }
    }

    /// Asks every game but `except` to close (SIGTERM), as closing its
    /// window would.
    pub fn close_others(&mut self, except: usize) {
        for (i, (_, child, _, _)) in self.games.iter_mut().enumerate() {
            if i == except || !matches!(child.try_wait(), Ok(None)) {
                continue;
            }
            #[cfg(unix)]
            {
                let Ok(pid) = libc::pid_t::try_from(child.id()) else {
                    continue;
                };
                // SAFETY: a signal to our own running child's pid.
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
            #[cfg(not(unix))]
            let _ = child.kill();
        }
    }

    fn note_interrupt(&mut self) {
        if !self.interrupted {
            self.interrupted = true;
            eprintln!("coop test: interrupted; waiting for the games to close (again to stop now)");
        }
    }
}

#[cfg(test)]
mod tests;
