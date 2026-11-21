//! Steam's on-screen keyboard for the game's virtual keyboard screen
//! (docs/input/host-keyboard.md). Under Steam on Linux, when a text field
//! gains focus after pad input, `steam://open/keyboard` goes to the running
//! client over its command pipe; `steam://close/keyboard` follows when focus
//! leaves. Steam types into the focused window as ordinary key input, which
//! takes the committed-text path in `keymap::text_input`. `H2_STEAM_KEYBOARD=0`
//! turns it off.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use input::active::DeviceKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Open,
    Close,
}

impl Command {
    /// One command line, as the `steam` launcher writes it.
    fn line(self) -> &'static [u8] {
        match self {
            Command::Open => b"steam://open/keyboard\n",
            Command::Close => b"steam://close/keyboard\n",
        }
    }
}

/// Opens only after pad input, so keyboard users are left alone; closes only
/// what it opened.
fn decide(shown: bool, focused: bool, last: Option<DeviceKind>) -> Option<Command> {
    match (shown, focused) {
        (false, true) if last.is_some_and(|d| d != DeviceKind::Keyboard) => Some(Command::Open),
        (true, false) => Some(Command::Close),
        _ => None,
    }
}

/// Writes one line without waiting: no reading client fails with ENXIO and a
/// full pipe with EAGAIN. A line is below PIPE_BUF, so it is never split.
fn send(pipe: &Path, command: Command) -> std::io::Result<()> {
    OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(pipe)?
        .write_all(command.line())
}

pub(crate) struct SteamKeyboard {
    pipe: PathBuf,
    shown: bool,
}

impl SteamKeyboard {
    /// `Some` when Steam launched the process (`SteamGameId`) and
    /// `H2_STEAM_KEYBOARD` is not `0`.
    pub(crate) fn from_env() -> Option<Self> {
        let launched = std::env::var_os("SteamGameId").is_some_and(|v| !v.is_empty());
        if !launched || std::env::var("H2_STEAM_KEYBOARD").is_ok_and(|v| v == "0") {
            return None;
        }
        let home = std::env::var_os("HOME")?;
        Some(SteamKeyboard {
            pipe: Path::new(&home).join(".steam/steam.pipe"),
            shown: false,
        })
    }

    /// Follows the game's text focus. `last` is the last-used input device.
    /// A failed Close leaves the keyboard counted as shown, so drop retries it.
    pub(crate) fn update(&mut self, focused: bool, last: Option<DeviceKind>) {
        let Some(command) = decide(self.shown, focused, last) else {
            return;
        };
        match send(&self.pipe, command) {
            Ok(()) => self.shown = command == Command::Open,
            Err(e) => eprintln!(
                "app: Steam keyboard {command:?} via {}: {e}",
                self.pipe.display()
            ),
        }
    }
}

impl Drop for SteamKeyboard {
    fn drop(&mut self) {
        self.update(false, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn opens_after_pad_input_and_closes_what_it_opened() {
        use DeviceKind::*;
        assert_eq!(decide(false, true, Some(Xbox)), Some(Command::Open));
        assert_eq!(decide(false, true, Some(Steam)), Some(Command::Open));
        assert_eq!(decide(false, true, Some(Keyboard)), None);
        assert_eq!(decide(false, true, None), None);
        assert_eq!(decide(true, true, Some(Keyboard)), None);
        assert_eq!(decide(true, false, Some(Keyboard)), Some(Command::Close));
        assert_eq!(decide(false, false, Some(Xbox)), None);
    }

    fn fifo(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("h2-steam-kb-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        path
    }

    #[test]
    fn send_writes_one_line_to_a_reading_client() {
        let path = fifo("reader");
        let mut reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .unwrap();
        send(&path, Command::Open).unwrap();
        send(&path, Command::Close).unwrap();
        let mut got = String::new();
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "steam://open/keyboard\nsteam://close/keyboard\n");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn send_without_a_client_fails_at_once() {
        let path = fifo("none");
        let e = send(&path, Command::Open).unwrap_err();
        assert_eq!(e.raw_os_error(), Some(libc::ENXIO));
        std::fs::remove_file(&path).unwrap();
    }
}
