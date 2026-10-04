//! Choosing a shell and starting it with ShellView's integration hooks.
//!
//! zsh  — started with `ZDOTDIR` pointing at our forwarder files, which source
//!        the user's own dotfiles and then add `precmd`/`preexec` hooks.
//! bash — started as `bash --rcfile shellview.bash -i`; the rcfile emulates
//!        login startup, then adds `PROMPT_COMMAND` + a DEBUG trap.
//! other shells run untouched (no hooks).

use std::io;
use std::path::{Path, PathBuf};

const ZSH_FILES: [(&str, &str); 4] = [
    (".zshenv", include_str!("../../../shell/zsh/zshenv")),
    (".zprofile", include_str!("../../../shell/zsh/zprofile")),
    (".zshrc", include_str!("../../../shell/zsh/zshrc")),
    (".zlogin", include_str!("../../../shell/zsh/zlogin")),
];
const BASH_RC: &str = include_str!("../../../shell/bash/shellview.bash");

/// Everything needed to start the shell process.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub name: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub integrated: bool,
}

/// Pick a shell: explicit choice (name or path) > `$SHELL` > zsh > bash > sh.
pub fn resolve(choice: Option<&str>) -> io::Result<PathBuf> {
    let shell_env = std::env::var("SHELL").ok();
    let candidates = choice
        .into_iter()
        .chain(shell_env.as_deref())
        .chain(["zsh", "bash", "sh"]);
    for c in candidates {
        if c.contains('/') {
            if is_executable(Path::new(c)) {
                return Ok(PathBuf::from(c));
            }
            if choice == Some(c) {
                return Err(not_found(c));
            }
        } else if let Some(p) = find_in_path(c) {
            return Ok(p);
        } else if choice == Some(c) {
            return Err(not_found(c));
        }
    }
    Err(not_found("any shell"))
}

fn not_found(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("shell not found: {what}"))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

/// Write `files` into `dir`, only touching files whose content changed.
fn install(dir: &Path, files: &[(&str, &str)]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, content) in files {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(*content) {
            std::fs::write(&path, content)?;
        }
    }
    Ok(())
}

/// Build the launch plan. Integration files go under `cache/integration/<version>/`.
pub fn build(choice: Option<&str>, integrate: bool, cache: &Path) -> io::Result<Launch> {
    let program = resolve(choice)?;
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().trim_start_matches('-').to_string())
        .unwrap_or_default();
    let base = cache.join("integration").join(env!("CARGO_PKG_VERSION"));

    let launch = match (integrate, name.as_str()) {
        (true, "zsh") => {
            let dir = base.join("zsh");
            install(&dir, &ZSH_FILES)?;
            Launch {
                args: vec!["-l".into()],
                env: vec![
                    ("ZDOTDIR".into(), dir.to_string_lossy().into_owned()),
                    (
                        "SHELLVIEW_ORIG_ZDOTDIR".into(),
                        std::env::var("ZDOTDIR").unwrap_or_default(),
                    ),
                ],
                integrated: true,
                program,
                name,
            }
        }
        (true, "bash") => {
            let dir = base.join("bash");
            install(&dir, &[("shellview.bash", BASH_RC)])?;
            Launch {
                // --rcfile is ignored by login shells, so the rcfile emulates login startup.
                args: vec![
                    "--rcfile".into(),
                    dir.join("shellview.bash").to_string_lossy().into_owned(),
                    "-i".into(),
                ],
                env: vec![("SHELLVIEW_LOGIN".into(), "1".into())],
                integrated: true,
                program,
                name,
            }
        }
        _ => Launch {
            args: vec!["-l".into()],
            env: vec![],
            integrated: false,
            program,
            name,
        },
    };
    Ok(launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sv-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn bash_launch_uses_rcfile() {
        if find_in_path("bash").is_none() {
            return;
        }
        let cache = tmp("bash");
        let l = build(Some("bash"), true, &cache).unwrap();
        assert_eq!(l.name, "bash");
        assert_eq!(l.args[0], "--rcfile");
        assert!(Path::new(&l.args[1]).exists());
        assert!(l.integrated);
    }

    #[test]
    fn zsh_launch_sets_zdotdir() {
        if find_in_path("zsh").is_none() {
            return;
        }
        let cache = tmp("zsh");
        let l = build(Some("zsh"), true, &cache).unwrap();
        let zdot = &l.env.iter().find(|(k, _)| k == "ZDOTDIR").unwrap().1;
        for (f, _) in ZSH_FILES {
            assert!(Path::new(zdot).join(f).exists(), "{f} missing");
        }
    }

    #[test]
    fn integration_can_be_disabled() {
        let cache = tmp("plain");
        let l = build(Some("sh"), true, &cache).unwrap();
        assert!(!l.integrated);
        let l = build(Some("bash"), false, &cache);
        if let Ok(l) = l {
            assert!(!l.integrated);
        }
    }

    #[test]
    fn unknown_shell_is_an_error() {
        assert!(resolve(Some("no-such-shell-xyz")).is_err());
        assert!(resolve(Some("/no/such/shell")).is_err());
    }
}
