//! Command history with exit codes and durations, one JSON object per line.
//! Written to `<state>/history.jsonl`; read later by `sv history`.

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
pub struct Entry<'a> {
    pub ts: u64,
    pub cwd: &'a str,
    pub cmd: &'a str,
    pub exit: Option<i32>,
    pub duration_ms: u64,
    pub shell: &'a str,
}

pub struct History {
    path: PathBuf,
}

impl History {
    pub fn new(state_dir: &Path) -> Self {
        History {
            path: state_dir.join("history.jsonl"),
        }
    }

    pub fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Best effort: history must never break the shell.
    pub fn append(&self, e: &Entry) {
        let Ok(line) = serde_json::to_string(e) else {
            return;
        };
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}
