//! `config.yaml` — user settings. Every field is optional.
//!
//! ```yaml
//! shell: zsh              # or bash, or a full path; default: $SHELL
//! scrollback_lines: 5000  # per shell tab
//! shell_integration: true # report folder / command / exit code to ShellView
//! mouse: true            # mouse wheel scrolls history; hold Option (iTerm2) or
//!                         # turn off View → Allow Mouse Reporting (Terminal.app) to select text
//! ```

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub shell: Option<String>,
    pub scrollback_lines: usize,
    pub shell_integration: bool,
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shell: None,
            scrollback_lines: 5000,
            shell_integration: true,
            mouse: true,
        }
    }
}

impl Config {
    /// Parse YAML text. An empty file means "all defaults".
    pub fn parse(text: &str) -> Result<Config, String> {
        if text.trim().is_empty() {
            return Ok(Config::default());
        }
        let mut cfg: Config = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
        cfg.scrollback_lines = cfg.scrollback_lines.min(1_000_000);
        Ok(cfg)
    }

    /// Load from disk. A missing file is fine; a broken one falls back to
    /// defaults and returns a warning for the user.
    pub fn load(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Config::parse(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => (
                    Config::default(),
                    Some(format!("{}: {e} — using defaults", path.display())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
            Err(e) => (
                Config::default(),
                Some(format!("{}: {e} — using defaults", path.display())),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(Config::parse("  \n").unwrap(), Config::default());
    }

    #[test]
    fn partial_fields() {
        let c = Config::parse("shell: bash\n").unwrap();
        assert_eq!(c.shell.as_deref(), Some("bash"));
        assert_eq!(c.scrollback_lines, 5000);
        assert!(c.shell_integration);
    }

    #[test]
    fn unknown_keys_are_tolerated() {
        // Later releases add keys (layouts, keymap…); older binaries must not choke.
        let c = Config::parse("scrollback_lines: 100\nlayouts: {}\n").unwrap();
        assert_eq!(c.scrollback_lines, 100);
    }

    #[test]
    fn bad_yaml_is_an_error() {
        assert!(Config::parse("scrollback_lines: lots\n").is_err());
    }

    #[test]
    fn missing_file_is_silent() {
        let (c, warn) = Config::load(Path::new("/definitely/not/here.yaml"));
        assert_eq!(c, Config::default());
        assert!(warn.is_none());
    }
}
