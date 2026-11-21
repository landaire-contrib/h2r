//! The local accounts file (`live/accounts.json`, 0600): one entry per
//! account this machine holds a key for, on whichever server. `h2 live
//! account create` and `import` write it, `h2 play` reads it with Live on, and
//! the game-side HLE answers sign-in from `xuid` and `gamertag`.
//!
//! An entry is `{xuid, gamertag, id, server, server_id, kid, key}` where
//! `key` is the Ed25519 private JWK (`{kty, crv, x, d}`). Older entries
//! (and the web app's first downloads) carry `key` as the bare base64url
//! seed instead; both read, and fields this version does not know are kept.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use live_proto::sign::{KeyPair, PrivateJwk};
#[cfg(feature = "desktop")]
use live_proto::{AccountKeys, Discovery, Gamertag};
use live_proto::{Author, Xuid};
#[cfg(feature = "desktop")]
use live_server::identity::write_private;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where Live keeps this machine's accounts (`accounts.json`), the
/// account handed to a game (`play-accounts.json`) and an embedded
/// server's data: `$H2_LIVE_DIR`, else the application support directory.
/// `h2 coop test` gives each game its own, so two games on one Mac never
/// share `play-accounts.json`.
pub(crate) fn live_dir() -> Result<PathBuf> {
    live_dir_from(
        std::env::var_os(LIVE_DIR_ENV),
        content::store::app_data_dir(content::store::DataHost::CURRENT, |name| std::env::var_os(name)),
    )
}

/// The variable [`live_dir`] reads first.
pub(crate) const LIVE_DIR_ENV: &str = "H2_LIVE_DIR";

fn live_dir_from(
    over: Option<std::ffi::OsString>,
    app_data: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(dir) = over.filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let root = app_data.context("application data directory is unavailable; set H2_LIVE_DIR")?;
    Ok(root.join("live"))
}

pub(crate) fn accounts_path() -> Result<PathBuf> {
    Ok(live_dir()?.join("accounts.json"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAccount {
    pub xuid: Xuid,
    pub gamertag: String,
    /// The author id, `acct:<xuid>@<server id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The base URL the account was made on.
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    pub key: StoredKey,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoredKey {
    Jwk(PrivateJwk),
    Seed(String),
}

impl std::fmt::Debug for StoredKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredKey(..)")
    }
}

impl LocalAccount {
    #[cfg(feature = "desktop")]
    pub fn new(
        xuid: Xuid,
        gamertag: &str,
        server: &str,
        discovery: &Discovery,
        key: &KeyPair,
    ) -> Self {
        LocalAccount {
            xuid,
            gamertag: gamertag.into(),
            id: Some(author_id(xuid, &discovery.id)),
            server: server.into(),
            server_id: Some(discovery.id.clone()),
            kid: Some(key.kid().into()),
            key: StoredKey::Jwk(key.private_jwk()),
            extra: Map::new(),
        }
    }

    /// The key, checked against the entry's own `x` (and `kid`) if it has
    /// them.
    pub fn key_pair(&self) -> Result<KeyPair> {
        let key = match &self.key {
            StoredKey::Jwk(jwk) => KeyPair::from_private_jwk(jwk),
            StoredKey::Seed(seed) => KeyPair::from_secret_b64(seed),
        }
        .map_err(|e| anyhow::anyhow!("{}'s key does not parse: {e}", self.gamertag))?;
        let x = self.extra.get("x").and_then(Value::as_str);
        if x.is_some_and(|x| x != key.public().jwk().x)
            || self.kid.as_deref().is_some_and(|k| k != key.kid())
        {
            bail!("{}'s key does not match its own x or kid", self.gamertag);
        }
        Ok(key)
    }

    /// Whether this entry is for the server with `id` at `base`.
    pub fn is_on(&self, base: &str, id: &str) -> bool {
        match &self.server_id {
            Some(sid) => sid == id,
            None => self.server.trim_end_matches('/') == base.trim_end_matches('/'),
        }
    }

    #[cfg(feature = "desktop")]
    fn tag_key(&self) -> String {
        self.gamertag.to_ascii_lowercase()
    }

    /// The account as this version writes it: the key as a JWK, the
    /// derivable fields filled in.
    #[cfg(feature = "desktop")]
    fn normalized(mut self, key: &KeyPair, server_id: &str) -> Self {
        self.key = StoredKey::Jwk(key.private_jwk());
        self.kid = Some(key.kid().into());
        self.server_id = Some(server_id.into());
        self.id = Some(author_id(self.xuid, server_id));
        self.extra.remove("x");
        self
    }
}

pub fn author_id(xuid: Xuid, server_id: &str) -> String {
    Author::Account {
        xuid,
        server: server_id.into(),
    }
    .to_string()
}

/// The accounts file; a missing file is empty.
pub fn load(path: &Path) -> Result<Vec<LocalAccount>> {
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes the file mode 0600 (tightening an older, looser one).
#[cfg(feature = "desktop")]
pub fn save(path: &Path, accounts: &[LocalAccount]) -> Result<()> {
    write_private(
        path,
        format!("{}\n", serde_json::to_string_pretty(accounts)?).as_bytes(),
    )
}

/// Replaces any entry with the same xuid.
#[cfg(feature = "desktop")]
pub fn upsert(accounts: &mut Vec<LocalAccount>, account: LocalAccount) {
    accounts.retain(|a| a.xuid != account.xuid);
    accounts.push(account);
}

/// An import file: one entry, or a list of them.
#[cfg(feature = "desktop")]
pub fn parse_import(bytes: &[u8]) -> Result<Vec<LocalAccount>> {
    let value: Value = serde_json::from_slice(bytes).context("the file is not JSON")?;
    let entries = match value {
        Value::Array(items) => items,
        one => vec![one],
    };
    if entries.is_empty() {
        bail!("the file has no accounts");
    }
    entries
        .into_iter()
        .map(|e| {
            // Never echo the input: it holds a private key.
            serde_json::from_value(e).map_err(|_| {
                anyhow::anyhow!("an entry is not {{xuid, gamertag, server, key}} with a valid xuid")
            })
        })
        .collect()
}

/// Checks an imported entry against what the server says: the gamertag is
/// valid, the key parses, and its public half is the account's current key.
/// Returns the entry as it will be stored.
#[cfg(feature = "desktop")]
pub fn check_import(
    entry: LocalAccount,
    discovery: &Discovery,
    keys: &AccountKeys,
) -> Result<LocalAccount> {
    Gamertag::new(&entry.gamertag).map_err(|e| anyhow::anyhow!("{:?}: {e}", entry.gamertag))?;
    if entry
        .server_id
        .as_deref()
        .is_some_and(|id| id != discovery.id)
    {
        bail!(
            "{} is for server {:?}, not {:?}",
            entry.gamertag,
            entry.server_id.as_deref().unwrap_or(""),
            discovery.id
        );
    }
    let key = entry.key_pair()?;
    let x = key.public().jwk().x;
    let current = keys
        .keys
        .iter()
        .any(|k| k.valid_to.is_none() && k.kid == key.kid() && k.jwk.x == x);
    if !current {
        bail!(
            "{}'s key is not a current key of xuid {} on {}",
            entry.gamertag,
            entry.xuid,
            discovery.id
        );
    }
    Ok(entry.normalized(&key, &discovery.id))
}

/// Adds `entry`, refusing one whose xuid is already here or whose gamertag
/// is already held for the same server.
#[cfg(feature = "desktop")]
pub fn merge(accounts: &mut Vec<LocalAccount>, entry: LocalAccount) -> Result<()> {
    if let Some(a) = accounts.iter().find(|a| a.xuid == entry.xuid) {
        bail!(
            "xuid {} is already in the accounts file (as {})",
            entry.xuid,
            a.gamertag
        );
    }
    let server_id = entry.server_id.as_deref().unwrap_or_default();
    if accounts
        .iter()
        .any(|a| a.tag_key() == entry.tag_key() && a.is_on(&entry.server, server_id))
    {
        bail!(
            "{} on {server_id} is already in the accounts file",
            entry.gamertag
        );
    }
    accounts.push(entry);
    Ok(())
}

/// A gamertag from an OS user name: letters and digits kept, anything else
/// a single space between words, cut to 15 and led by a letter. `Player`
/// when nothing usable is left.
#[cfg(feature = "desktop")]
pub fn gamertag_from(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    let mut tag = words.join(" ");
    let start = tag
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(tag.len());
    tag.drain(..start);
    tag.truncate(15);
    let tag = tag.trim_end().to_string();
    match Gamertag::new(&tag) {
        Ok(_) => tag,
        Err(_) => "Player".into(),
    }
}

/// `base` with a number on the end, kept within 15 characters.
#[cfg(feature = "desktop")]
pub fn numbered(base: &str, n: u32) -> String {
    let suffix = n.to_string();
    let keep = base.len().min(15 - suffix.len());
    format!("{}{suffix}", base[..keep].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_directory_is_h2_live_dir_when_set() {
        assert_eq!(
            live_dir_from(Some("/x/live".into()), Some("/app/h2recomp".into())).unwrap(),
            PathBuf::from("/x/live")
        );
        assert_eq!(
            live_dir_from(Some("".into()), Some("/app/h2recomp".into())).unwrap(),
            PathBuf::from("/app/h2recomp/live")
        );
        assert!(live_dir_from(None, None).is_err());
    }
}

#[cfg(all(test, feature = "desktop"))]
mod management_tests {
    use super::*;
    use live_proto::AccountKey;

    fn discovery() -> Discovery {
        Discovery {
            sim_combat: false,
            ingest_frame_versions: Vec::new(),
            id: "live.test".into(),
            api: "/v1".into(),
            keys: Vec::new(),
            software: "test".into(),
            federation: false,
            url: None,
        }
    }

    fn keys_of(key: &KeyPair, valid_to: Option<u64>) -> AccountKeys {
        AccountKeys {
            keys: vec![AccountKey {
                kid: key.kid().into(),
                jwk: key.public().jwk(),
                valid_from: 3,
                valid_to,
            }],
            position: 9,
        }
    }

    fn web_entry(key: &KeyPair, seed_form: bool) -> Vec<u8> {
        let k = if seed_form {
            serde_json::json!(key.secret_b64())
        } else {
            serde_json::to_value(key.private_jwk()).unwrap()
        };
        serde_json::to_vec(&serde_json::json!({
            "xuid": "0009000000000042", "gamertag": "Web Pete", "server": "https://live.test", "key": k, "x": key.public().jwk().x,
        }))
        .unwrap()
    }

    #[test]
    fn imports_check_the_key_against_the_server() {
        let key = KeyPair::generate();
        for seed_form in [false, true] {
            let [entry] = parse_import(&web_entry(&key, seed_form))
                .unwrap()
                .try_into()
                .unwrap();
            let stored = check_import(entry, &discovery(), &keys_of(&key, None)).unwrap();
            assert_eq!(
                stored.id.as_deref(),
                Some("acct:0009000000000042@live.test")
            );
            assert_eq!(stored.kid.as_deref(), Some(key.kid()));
            assert!(matches!(stored.key, StoredKey::Jwk(_)) && !stored.extra.contains_key("x"));
            assert_eq!(stored.key_pair().unwrap().kid(), key.kid());
        }
        let entry = || parse_import(&web_entry(&key, false)).unwrap().remove(0);
        // Another key, a revoked key, another server.
        assert!(check_import(entry(), &discovery(), &keys_of(&KeyPair::generate(), None)).is_err());
        assert!(check_import(entry(), &discovery(), &keys_of(&key, Some(7))).is_err());
        let mut e = entry();
        e.server_id = Some("elsewhere".into());
        assert!(check_import(e, &discovery(), &keys_of(&key, None)).is_err());
        // A key that is not its own x.
        let mut e = entry();
        e.extra
            .insert("x".into(), KeyPair::generate().public().jwk().x.into());
        assert!(check_import(e, &discovery(), &keys_of(&key, None)).is_err());
    }

    #[test]
    fn bad_import_files_do_not_echo_keys() {
        assert!(parse_import(b"[]").is_err());
        assert!(parse_import(b"nope").is_err());
        let err =
            parse_import(br#"{"xuid":"12","gamertag":"A","server":"x","key":"secret-marker"}"#)
                .unwrap_err();
        assert!(!format!("{err:#}").contains("secret-marker"));
        let mut e = parse_import(
            br#"{"xuid":"0009000000000001","gamertag":"A","server":"x","key":"not-a-key"}"#,
        )
        .unwrap();
        assert!(e.remove(0).key_pair().is_err());
    }

    #[test]
    fn merging_refuses_duplicates() {
        let key = KeyPair::generate();
        let d = discovery();
        let a = LocalAccount::new(Xuid(0x9000000000001), "Pete", "https://live.test", &d, &key);
        let mut accounts = vec![a.clone()];
        assert!(merge(&mut accounts, a.clone()).is_err(), "same xuid");
        let mut same_tag =
            LocalAccount::new(Xuid(0x9000000000002), "PETE", "https://live.test", &d, &key);
        assert!(
            merge(&mut accounts, same_tag.clone()).is_err(),
            "same gamertag on the server"
        );
        same_tag.server_id = Some("other".into());
        merge(&mut accounts, same_tag).unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn the_file_round_trips_at_0600() {
        let dir = std::env::temp_dir().join(format!("h2-accounts-{}", std::process::id()));
        let path = dir.join("accounts.json");
        assert!(load(&path).unwrap().is_empty());
        let key = KeyPair::generate();
        let mut accounts = Vec::new();
        upsert(
            &mut accounts,
            LocalAccount::new(
                Xuid(0x9000000000001),
                "Pete",
                "http://x",
                &discovery(),
                &key,
            ),
        );
        save(&path, &accounts).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back[0].key_pair().unwrap().kid(), key.kid());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"kty\": \"OKP\""), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // An old entry (key as the seed) still reads.
        fs::write(&path, format!(r#"[{{"xuid":"0009000000000001","gamertag":"Old","server":"http://x","key":"{}"}}]"#, key.secret_b64())).unwrap();
        assert_eq!(load(&path).unwrap()[0].key_pair().unwrap().kid(), key.kid());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn os_user_names_become_gamertags() {
        assert_eq!(gamertag_from("afr"), "afr");
        assert_eq!(gamertag_from("john.smith"), "john smith");
        assert_eq!(gamertag_from("_42bob__x"), "bob x");
        assert_eq!(gamertag_from("a-very-long-user-name"), "a very long use");
        assert_eq!(gamertag_from("abcdefghijklmn op"), "abcdefghijklmn");
        assert_eq!(gamertag_from("1234"), "Player");
        assert_eq!(gamertag_from(""), "Player");
        assert_eq!(gamertag_from("José"), "Jos");
        assert_eq!(numbered("Pete", 2), "Pete2");
        assert_eq!(numbered("abcdefghijklmno", 12), "abcdefghijklm12");
        assert_eq!(numbered("abcdefghijklm o", 12), "abcdefghijklm12");
    }
}
