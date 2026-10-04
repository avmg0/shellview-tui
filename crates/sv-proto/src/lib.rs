//! Wire formats shared by the ShellView app and the `sv` command.
//!
//! Shell hooks and `sv` talk to ShellView by printing OSC escape sequences to
//! the terminal. ShellView sits between the shell and the real terminal, so it
//! sees them, acts on them, and never forwards them.
//!
//! | Sequence                        | Meaning                         |
//! |---------------------------------|---------------------------------|
//! | `OSC 7 ; file://host/path`      | current working directory       |
//! | `OSC 133 ; A`                   | prompt shown                    |
//! | `OSC 133 ; C`                   | command started                 |
//! | `OSC 133 ; D [; exit]`          | command finished                |
//! | `OSC 7770 ; cmd ; <base64>`     | text of the command being run   |
//! | `OSC 7770 ; mode ; on/off/exit` | switch ShellView view / quit    |

use base64::Engine as _;

/// ShellView's private OSC number.
pub const OSC_SHELLVIEW: &[u8] = b"7770";

/// Environment variable set inside every shell ShellView starts.
pub const ENV_INSIDE: &str = "SHELLVIEW";
/// Environment variable holding the ShellView process id.
pub const ENV_PID: &str = "SHELLVIEW_PID";
/// Environment variable holding the ShellView version.
pub const ENV_VERSION: &str = "SHELLVIEW_VERSION";
/// Environment variable holding the shell name ShellView started (zsh, bash…).
pub const ENV_SHELL: &str = "SHELLVIEW_SHELL";

/// A view-mode request sent by `sv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Show the full multi-pane view.
    On,
    /// Hide panels and show only the shell (basic view).
    Off,
    /// Leave ShellView completely.
    Exit,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::On => "on",
            Mode::Off => "off",
            Mode::Exit => "exit",
        }
    }

    pub fn parse(s: &[u8]) -> Option<Self> {
        match s {
            b"on" => Some(Mode::On),
            b"off" => Some(Mode::Off),
            b"exit" => Some(Mode::Exit),
            _ => None,
        }
    }

    /// The escape sequence `sv` prints to request this mode.
    pub fn sequence(self) -> String {
        format!("\x1b]7770;mode;{}\x07", self.as_str())
    }
}

/// Something the shell (or `sv`) told ShellView.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    Cwd(String),
    PromptShown,
    CommandStarted,
    CommandFinished(Option<i32>),
    CommandText(String),
    Mode(Mode),
}

/// Parse the parameters of an OSC sequence (already split on `;`).
///
/// Returns `None` for sequences that are not ShellView's.
pub fn parse_osc(params: &[&[u8]]) -> Option<ShellEvent> {
    match params {
        [b"7", rest @ ..] if !rest.is_empty() => {
            let url = rest.join(&b';');
            Some(ShellEvent::Cwd(path_from_file_url(&url)?))
        }
        [b"133", kind, rest @ ..] => match *kind {
            b"A" => Some(ShellEvent::PromptShown),
            b"C" => Some(ShellEvent::CommandStarted),
            b"D" => {
                let code = rest
                    .first()
                    .and_then(|c| std::str::from_utf8(c).ok())
                    .and_then(|c| c.parse::<i32>().ok());
                Some(ShellEvent::CommandFinished(code))
            }
            _ => None,
        },
        [id, b"cmd", data] if *id == OSC_SHELLVIEW => {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(data)
                .ok()?;
            Some(ShellEvent::CommandText(
                String::from_utf8_lossy(&raw).into_owned(),
            ))
        }
        [id, b"cmd"] if *id == OSC_SHELLVIEW => Some(ShellEvent::CommandText(String::new())),
        [id, b"mode", m] if *id == OSC_SHELLVIEW => Mode::parse(m).map(ShellEvent::Mode),
        _ => None,
    }
}

/// `file://host/some%20path` → `/some path`
fn path_from_file_url(url: &[u8]) -> Option<String> {
    let rest = url.strip_prefix(b"file://")?;
    let slash = rest.iter().position(|&b| b == b'/')?;
    let path = percent_decode(&rest[slash..]);
    Some(String::from_utf8_lossy(&path).into_owned())
}

fn percent_decode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Vec<&[u8]> {
        s.as_bytes().split(|&b| b == b';').collect()
    }

    #[test]
    fn cwd() {
        assert_eq!(
            parse_osc(&split("7;file://mbp/Users/a/My%20Dir")),
            Some(ShellEvent::Cwd("/Users/a/My Dir".into()))
        );
        assert_eq!(
            parse_osc(&split("7;file:///tmp/a;b")),
            Some(ShellEvent::Cwd("/tmp/a;b".into()))
        );
    }

    #[test]
    fn command_lifecycle() {
        assert_eq!(parse_osc(&split("133;A")), Some(ShellEvent::PromptShown));
        assert_eq!(parse_osc(&split("133;C")), Some(ShellEvent::CommandStarted));
        assert_eq!(
            parse_osc(&split("133;D;1")),
            Some(ShellEvent::CommandFinished(Some(1)))
        );
        assert_eq!(
            parse_osc(&split("133;D")),
            Some(ShellEvent::CommandFinished(None))
        );
        // "ls | wc -l" in base64
        assert_eq!(
            parse_osc(&split("7770;cmd;bHMgfCB3YyAtbA==")),
            Some(ShellEvent::CommandText("ls | wc -l".into()))
        );
    }

    #[test]
    fn modes() {
        for m in [Mode::On, Mode::Off, Mode::Exit] {
            let seq = m.sequence();
            let body = seq.trim_start_matches("\x1b]").trim_end_matches('\x07');
            assert_eq!(parse_osc(&split(body)), Some(ShellEvent::Mode(m)));
        }
    }

    #[test]
    fn foreign_sequences_ignored() {
        assert_eq!(parse_osc(&split("8;;https://example.com")), None);
        assert_eq!(parse_osc(&split("52;c;aGk=")), None);
        assert_eq!(parse_osc(&split("7770;mode;bogus")), None);
    }

    #[test]
    fn bad_percent_escapes_are_kept() {
        assert_eq!(percent_decode(b"/a%2"), b"/a%2".to_vec());
        assert_eq!(percent_decode(b"/a%zz"), b"/a%zz".to_vec());
        assert_eq!(percent_decode(b"/a%41"), b"/aA".to_vec());
    }
}
