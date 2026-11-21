use super::*;
use crate::coop_verdict::Seen;

fn args(s: &[&str]) -> Vec<String> {
    s.iter().map(|a| a.to_string()).collect()
}

fn env(k: &str) -> Option<String> {
    match k {
        "HOME" => Some("/home/player".into()),
        "LOCALAPPDATA" => Some(r"C:\Local".into()),
        "H2_DISC" => Some("/discs/halo2.iso".into()),
        _ => None,
    }
}

fn opts(s: &[&str]) -> Opts {
    match parse(args(s), env).unwrap() {
        Cmd::Test(o) => o,
        c => panic!("{c:?}"),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("h2-coop-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn defaults_and_flags() {
    let o = opts(&["test"]);
    assert_eq!(o.disc, PathBuf::from("/discs/halo2.iso"));
    assert_eq!(o.players, 2);
    assert!(!o.auto && !o.fresh && !o.dry_run && !o.extras && !o.pad_on_guest);
    assert_eq!(
        o.root,
        if cfg!(target_vendor = "apple") {
            PathBuf::from("/home/player/Library/Application Support/h2recomp/coop-test")
        } else if cfg!(windows) {
            PathBuf::from(r"C:\Local\h2recomp\coop-test")
        } else {
            PathBuf::from("/home/player/.local/share/h2recomp/coop-test")
        }
    );
    assert_eq!((o.port, o.server_port), (31000, 8075));
    let o = opts(&[
        "test",
        "--disc",
        "/d.iso",
        "--players",
        "3",
        "--auto",
        "--root",
        "/r",
        "--out",
        "/o",
        "--ip",
        "10.0.0.5",
        "--port",
        "40000",
        "--server-port",
        "9000",
        "--pad",
        "guest",
        "--extras",
        "--fresh",
        "--dry-run",
        "--",
        "--fps",
        "60",
        "--auto",
    ]);
    assert_eq!(o.disc, PathBuf::from("/d.iso"));
    assert_eq!(o.players, 3);
    assert!(o.auto && o.fresh && o.dry_run && o.extras && o.pad_on_guest);
    assert_eq!(o.root, PathBuf::from("/r"));
    assert_eq!(o.out, Some(PathBuf::from("/o")));
    assert_eq!(o.ip, Some(Ipv4Addr::new(10, 0, 0, 5)));
    assert_eq!((o.port, o.server_port), (40000, 9000));
    assert_eq!(
        o.play_args,
        args(&["--fps", "60", "--auto"]),
        "after -- is h2 play's"
    );
}

#[test]
fn bad_arguments_say_why() {
    let err = |s: &[&str]| format!("{:#}", parse(args(s), env).unwrap_err());
    assert!(err(&["test", "--players", "1"]).contains("2 to 16"));
    assert!(err(&["test", "--players", "17"]).contains("2 to 16"));
    assert!(err(&["test", "--pad", "both"]).contains("host or guest"));
    assert!(err(&["test", "--ip", "nope"]).contains("IPv4"));
    assert!(err(&["test", "--port", "65530"]).contains("65530 to 65546"));
    assert!(err(&["test", "--bogus"]).contains("unexpected"));
    assert!(err(&["test", "--root"]).contains("needs a value"));
    assert!(err(&["frobnicate"]).contains("unknown `h2 coop frobnicate`"));
    assert!(err(&[]).contains("usage"));
    let no_disc = parse(args(&["test"]), |k| (k == "HOME").then(|| "/h".into())).unwrap_err();
    assert!(format!("{no_disc:#}").contains("--disc"));
}

#[test]
fn verdict_takes_a_log() {
    assert_eq!(
        parse(args(&["verdict", "/l/coop.log", "--auto"]), env).unwrap(),
        Cmd::Verdict {
            log: "/l/coop.log".into(),
            auto: true
        }
    );
    assert!(parse(args(&["verdict"]), env).is_err());
}

#[test]
fn each_game_has_its_own_ports_data_live_directory_and_socket() {
    let o = opts(&["test", "--root", "/r", "--players", "3", "--auto"]);
    let ip = Ipv4Addr::new(192, 168, 1, 20);
    let games = plan(
        &o,
        "http://127.0.0.1:8075",
        ip,
        Path::new("/cache/h2recomp"),
    );
    assert_eq!(games.len(), 3);
    let tags: Vec<&str> = games.iter().map(|g| g.tag.as_str()).collect();
    assert_eq!(tags, ["Alpha", "Bravo", "Charlie"]);
    assert!(games[0].host && !games[1].host);
    let get = |g: &Game, k: &str| g.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(
        get(&games[0], "H2_XNET_ADDR").unwrap(),
        "192.168.1.20:31000"
    );
    assert_eq!(
        get(&games[1], "H2_XNET_ADDR").unwrap(),
        "192.168.1.20:31010"
    );
    assert_eq!(
        get(&games[1], "H2_XNET_LOOPBACK_PEERS").unwrap(),
        "192.168.1.20:31000,192.168.1.20:31020"
    );
    assert_eq!(get(&games[2], "H2_LIVE_DIR").unwrap(), "/r/charlie/live");
    assert_eq!(
        get(&games[0], "H2_AGENT").unwrap(),
        "/cache/h2recomp/coop-alpha.sock"
    );
    assert_eq!(
        get(&games[0], "H2_COOP_AUTO_INVITE").unwrap(),
        "Bravo,Charlie"
    );
    assert_eq!(get(&games[2], "H2_COOP_AUTO_ACCEPT").unwrap(), "Alpha");
    assert_eq!(get(&games[0], "H2_COOP_AUTO_ACCEPT"), None);
    // The ports each game binds (base..base+6) never overlap, as the HLE
    // requires (`NetworkConfig::validate`: 7 apart).
    for w in games.windows(2) {
        assert!(w[1].xnet.port() - w[0].xnet.port() >= 7);
    }
    assert_eq!(
        games[0].args,
        args(&[
            "play",
            "/discs/halo2.iso",
            "--data",
            "/r/alpha/data",
            "--live-server",
            "http://127.0.0.1:8075",
            "--gamertag",
            "Alpha",
            "--title",
            "Halo 2 — Alpha (host)",
            "--tile",
            "0/3",
            "--no-mappack",
            "--no-halo1",
        ])
    );
    assert!(games[1].args.contains(&"--no-audio".into()));
    // Told apart by their titles, side by side, the host leftmost.
    assert!(
        games[2]
            .args
            .windows(2)
            .any(|w| w == ["--title", "Halo 2 — Charlie (guest)"])
    );
    assert!(games[2].args.windows(2).any(|w| w == ["--tile", "2/3"]));
    assert!(games[1].args.contains(&"--no-gamepads".into()));
    assert!(!games[0].args.contains(&"--no-gamepads".into()));
    // Without --auto, nothing automatic.
    let o = opts(&[
        "test", "--root", "/r", "--pad", "guest", "--extras", "--out", "/o",
    ]);
    let games = plan(&o, "http://x", ip, Path::new("/s"));
    assert!(
        games
            .iter()
            .all(|g| g.env.iter().all(|(k, _)| !k.starts_with("H2_COOP_AUTO")))
    );
    assert!(
        games[0].args.contains(&"--no-gamepads".into()),
        "the pads go to the guest"
    );
    assert!(!games[1].args.contains(&"--no-gamepads".into()));
    assert!(!games[0].args.contains(&"--no-mappack".into()));
    assert!(games[0].args.windows(2).any(|w| w == ["--out", "/o"]));
}

#[test]
fn shell_lines_quote_what_needs_it() {
    let o = opts(&["test", "--root", "/My Games/coop"]);
    let games = plan(
        &o,
        "http://127.0.0.1:8075",
        Ipv4Addr::LOCALHOST,
        Path::new("/s"),
    );
    let line = shell_line("/bin/h2", &games[0]);
    assert!(
        line.starts_with("H2_LIVE_DIR='/My Games/coop/alpha/live' H2_XNET_ADDR=127.0.0.1:31000"),
        "{line}"
    );
    assert!(
        line.contains(" /bin/h2 play /discs/halo2.iso --data '/My Games/coop/alpha/data' "),
        "{line}"
    );
}

#[test]
fn accounts_are_made_once_and_the_guests_befriend_the_host() {
    let dir = scratch("accounts");
    let server = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &dir.join("server"),
        Default::default(),
    )
    .unwrap();
    let client = Client::at(&server.url).unwrap();
    let mut o = opts(&["test", "--players", "3"]);
    o.root = dir.clone();
    let games = plan(&o, &server.url, Ipv4Addr::LOCALHOST, &dir);
    let all = dir.join("accounts.json");
    let made = accounts(&client, &games, &all).unwrap();
    assert_eq!(made.len(), 3);
    for (g, a) in games.iter().zip(&made) {
        // Each game's Live directory holds its own account, and only it.
        let mine = live_accounts::load(&g.live.join("accounts.json")).unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].xuid, a.xuid);
        assert_eq!(mine[0].gamertag, g.tag);
    }
    let friends: live_proto::Friends = client.get("/accounts/Alpha/friends", &[]).unwrap();
    let mut tags: Vec<String> = friends
        .friends
        .iter()
        .map(|f| f.gamertag.to_string())
        .collect();
    tags.sort();
    assert_eq!(tags, ["Bravo", "Charlie"]);
    // Again: the same accounts, nothing new, no error.
    let again = accounts(&client, &games, &all).unwrap();
    assert_eq!(
        again.iter().map(|a| a.xuid).collect::<Vec<_>>(),
        made.iter().map(|a| a.xuid).collect::<Vec<_>>()
    );
    // A game that lost its key: the server keeps the name; say --fresh.
    fs::remove_dir_all(&games[1].live).unwrap();
    let err = accounts(&client, &games, &all).unwrap_err();
    assert!(format!("{err:#}").contains("--fresh"), "{err:#}");
    drop(server);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_directories_are_named_by_utc_time() {
    assert_eq!(stamp(0), "19700101-000000");
    assert_eq!(stamp(951_782_400), "20000229-000000");
    assert_eq!(stamp(1_795_398_245), "20261123-014405");
}

#[test]
fn signed_in_is_the_games_own_line() {
    assert!(signed_in(
        "Alpha",
        "live: playing as Alpha (0009000000000001) on http://x"
    ));
    assert!(!signed_in(
        "Alpha",
        "live: playing as Bravo (2) on http://x"
    ));
    assert!(!signed_in("Alpha", "coop test: live: playing as Alpha"));
}

/// Two stand-in games (shell scripts) through the supervisor: lines land in
/// their own logs and the wanted ones in the merged log, the host's
/// sign-in line is reported, and the verdict reads the result.
#[cfg(unix)]
#[test]
fn the_supervisor_merges_two_games_and_the_verdict_reads_the_log() {
    let dir = scratch("supervise");
    let roles = Roles {
        host: "Alpha".into(),
        guests: vec!["Bravo".into()],
        late: vec![],
    };
    let merged = dir.join("coop.log");
    let mut sup = Supervisor::new(&merged, &dir, &roles).unwrap();
    let script = |lines: &[&str]| {
        let mut c = Command::new("/bin/sh");
        let body: Vec<String> = lines.iter().map(|l| format!("echo \"{l}\" >&2")).collect();
        c.arg("-c")
            .arg(format!("echo on-stdout; {}", body.join("; ")))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    };
    sup.spawn_command(
        script(&[
            "play: slice 1000",
            "live: playing as Alpha (1) on http://127.0.0.1:8075",
            "coop-live: 15 campaign levels (15 named)",
            "coop-auto: invited Bravo to session 8000000000000001",
        ]),
        "Alpha",
    )
    .unwrap();
    let mut ready = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.contains(&0) {
        ready.extend(sup.pump(Duration::from_millis(50)).unwrap());
        assert!(Instant::now() < deadline, "the host never signed in");
    }
    sup.spawn_command(
        script(&[
            "live: playing as Bravo (2) on http://127.0.0.1:8075",
            "coop-auto: accepting Alpha's invitation to session 8000000000000001",
            "d3d: not wanted",
        ]),
        "Bravo",
    )
    .unwrap();
    while !sup.all_exited() {
        sup.pump(Duration::from_millis(50)).unwrap();
        assert!(Instant::now() < deadline, "the games never closed");
    }
    sup.pump(Duration::from_millis(50)).unwrap();
    let alpha = fs::read_to_string(dir.join("Alpha.log")).unwrap();
    assert!(
        alpha.contains("on-stdout") && alpha.contains("play: slice 1000"),
        "{alpha}"
    );
    let text = fs::read_to_string(&merged).unwrap();
    assert!(text.starts_with("# host Alpha\n# guest Bravo\n"), "{text}");
    assert!(
        !text.contains("play: slice") && !text.contains("d3d"),
        "{text}"
    );
    assert!(text.contains("] Bravo | coop-auto: accepting"), "{text}");
    let v = coop_verdict::verdict(&text, true).unwrap();
    assert_eq!(v.steps[0].seen, Seen::Yes);
    assert_eq!(v.steps[6].seen, Seen::Yes, "invited");
    assert_eq!(v.steps[7].seen, Seen::Yes, "accepted");
    assert_eq!(v.stopped_at().unwrap().n, 2, "Bravo read no levels");
    fs::remove_dir_all(&dir).unwrap();
}

/// A game whose guest thread exits (its window stays open) is a stop: the
/// others are closed, and the stopped one is left for the player. A game
/// that ends in failure is one too; one closed normally is not.
#[cfg(unix)]
#[test]
fn a_stopped_game_closes_the_others() {
    let dir = scratch("stop");
    let roles = Roles {
        host: "Alpha".into(),
        guests: vec!["Bravo".into(), "Charlie".into()],
        late: vec![],
    };
    let merged = dir.join("coop.log");
    let mut sup = Supervisor::new(&merged, &dir, &roles).unwrap();
    // `exec` so the signal reaches the process holding the pipes.
    let script = |body: &str| {
        let mut c = Command::new("/bin/sh");
        c.arg("-c")
            .arg(body)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    };
    sup.spawn_command(
        script("echo 'live: playing as Alpha (1)' >&2; exec sleep 30"),
        "Alpha",
    )
    .unwrap();
    sup.spawn_command(
        script("echo 'app: guest thread exited: Fault' >&2; exec sleep 30"),
        "Bravo",
    )
    .unwrap();
    sup.spawn_command(script("exit 0"), "Charlie").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let stop = loop {
        sup.pump(Duration::from_millis(50)).unwrap();
        if let Some(s) = sup.crash() {
            break s;
        }
        assert!(Instant::now() < deadline, "no stop seen");
    };
    assert_eq!(stop, (1, "its guest thread exited".to_string()));
    sup.close_others(1);
    while !sup.exited(0) {
        sup.pump(Duration::from_millis(50)).unwrap();
        assert!(Instant::now() < deadline, "Alpha was not closed");
    }
    assert!(!sup.exited(1), "the stopped game is left open");
    assert_eq!(sup.crash(), None, "said once");
    let text = fs::read_to_string(&merged).unwrap();
    assert!(
        text.contains("] Bravo | app: guest thread exited"),
        "{text}"
    );
    sup.close_others(0);
    while !sup.all_exited() {
        sup.pump(Duration::from_millis(50)).unwrap();
        assert!(Instant::now() < deadline, "Bravo was not closed");
    }

    // A crash (a failed exit) with another still running.
    let mut sup = Supervisor::new(&dir.join("coop2.log"), &dir, &roles).unwrap();
    sup.spawn_command(script("exec sleep 30"), "Alpha").unwrap();
    sup.spawn_command(script("exit 3"), "Bravo").unwrap();
    let stop = loop {
        sup.pump(Duration::from_millis(50)).unwrap();
        if let Some(s) = sup.crash() {
            break s;
        }
        assert!(Instant::now() < deadline, "no stop seen");
    };
    assert_eq!(stop.0, 1);
    assert!(stop.1.contains('3'), "{}", stop.1);
    sup.close_others(1);
    while !sup.all_exited() {
        sup.pump(Duration::from_millis(50)).unwrap();
        assert!(Instant::now() < deadline, "Alpha was not closed");
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn players_go_up_to_sixteen_and_the_host_gets_the_limit() {
    use super::*;
    assert_eq!(TAGS.len(), 16);
    let tags = &TAGS[..3];
    assert_eq!(tags, ["Alpha", "Bravo", "Charlie"]);
    let mut seen = std::collections::HashSet::new();
    assert!(TAGS.iter().all(|t| seen.insert(*t)), "gamertags are unique");
}

#[test]
fn the_next_steps_name_each_window_and_its_presses() {
    let two = Roles {
        host: "Alpha".into(),
        guests: vec!["Bravo".into()],
        late: vec![],
    };
    assert_eq!(
        next_steps(&two, true),
        "coop test: next, on Bravo's window: Xbox Live (to sign in). Then on Alpha's window: \
         Xbox Live > Co-op Campaign > pick a level and difficulty; on Bravo, the invitation is \
         accepted by itself, but join from the friends list: Y, Alpha, Join Game; then Start \
         Game on Alpha."
    );
    let three = Roles {
        host: "Alpha".into(),
        guests: vec!["Bravo".into(), "Charlie".into()],
        late: vec![],
    };
    let line = next_steps(&three, false);
    assert!(
        line.contains("on Bravo and Charlie's windows: Xbox Live"),
        "{line}"
    );
    assert!(
        line.contains("press Y and invite Bravo and Charlie"),
        "{line}"
    );
}

/// A stand-in game's agent socket: the harness asks it to wait for the
/// main menu, and goes on when it answers.
#[cfg(unix)]
#[test]
fn the_harness_waits_on_the_hosts_agent_socket_for_the_main_menu() {
    use std::os::unix::net::UnixListener;
    let dir = scratch("menu");
    let socket = dir.join("coop-alpha.sock");
    // Nothing listening: gives up when told to.
    let err = wait_for_menu(&socket, Duration::from_millis(100)).unwrap_err();
    assert!(format!("{err:#}").contains("connecting to"), "{err:#}");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).unwrap();
        let mut w = &s;
        w.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
            .unwrap();
        line
    });
    wait_for_menu(&socket, Duration::from_secs(10)).unwrap();
    let asked: serde_json::Value = serde_json::from_str(&server.join().unwrap()).unwrap();
    assert_eq!(asked["method"], "session.wait");
    assert_eq!(
        asked["params"]["until"],
        serde_json::json!({"phase": "menu", "map_ticks": 60})
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn late_holds_back_the_last_guests_accept_and_names_it_late() {
    let o = opts(&["test", "--players", "3", "--auto", "--late", "--root", "/r"]);
    assert!(o.late);
    let games = plan(&o, "http://x", Ipv4Addr::new(10, 0, 0, 2), Path::new("/s"));
    let get = |g: &Game, k: &str| g.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(
        get(&games[2], "H2_COOP_AUTO_ACCEPT_DELAY").as_deref(),
        Some("120")
    );
    assert_eq!(get(&games[1], "H2_COOP_AUTO_ACCEPT_DELAY"), None);
    assert_eq!(get(&games[0], "H2_COOP_AUTO_ACCEPT_DELAY"), None);
    // The host still invites everyone (Charlie accepts later).
    assert_eq!(
        get(&games[0], "H2_COOP_AUTO_INVITE").unwrap(),
        "Bravo,Charlie"
    );
    let r = roles(&games, true);
    assert_eq!(r.guests, ["Bravo"]);
    assert_eq!(r.late, ["Charlie"]);
    assert_eq!(roles(&games, false).late, Vec::<String>::new());
    let line = next_steps(&r, true);
    assert!(
        line.contains("Charlie: Xbox Live too; it accepts 120 s after the invitation"),
        "{line}"
    );
    // --late needs a guest to spare and the automation.
    for bad in [
        &["test", "--late", "--auto"][..],
        &["test", "--players", "3", "--late"],
    ] {
        let e = parse(args(bad), env).unwrap_err().to_string();
        assert!(e.contains("--late needs --auto"), "{e}");
    }
}
