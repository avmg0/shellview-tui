//! Where ShellView keeps its files.
//!
//! Every location can be overridden with an environment variable, which the
//! tests use to stay out of the real home directory.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if Path::new(&v).is_absolute() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

fn overridable(var: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(default)
}

/// `~/.config/shellview/config.yaml`
pub fn config_file() -> PathBuf {
    overridable("SHELLVIEW_CONFIG", || {
        xdg("XDG_CONFIG_HOME", ".config").join("shellview/config.yaml")
    })
}

/// `~/.local/state/shellview` — history, sessions, agent logs.
pub fn state_dir() -> PathBuf {
    overridable("SHELLVIEW_STATE_DIR", || {
        xdg("XDG_STATE_HOME", ".local/state").join("shellview")
    })
}

/// `~/.cache/shellview` — shell integration files and the `sv` link.
pub fn cache_dir() -> PathBuf {
    overridable("SHELLVIEW_CACHE_DIR", || {
        xdg("XDG_CACHE_HOME", ".cache").join("shellview")
    })
}
