//! End-to-end tests: run the real `shellview` binary inside a pseudo-terminal
//! with a real bash / zsh, type into it, and read the screen back.

use portable_pty::{CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(15);

struct Session {
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    rx: Receiver<Vec<u8>>,
    screen: vt100::Parser,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    dir: PathBuf,
}

fn have(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sv-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("home")).unwrap();
    std::fs::create_dir_all(d.join("work")).unwrap();
    // Predictable prompts, while still going through the user-dotfile path.
    std::fs::write(d.join("home/.bash_profile"), "PS1='PROMPT> '\n").unwrap();
    std::fs::write(d.join("home/.zshrc"), "PROMPT='PROMPT> '\nunsetopt BEEP\n").unwrap();
    d
}

impl Session {
    fn start(shell: &str, tag: &str) -> Session {
        let dir = scratch(tag);
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_shellview"));
        cmd.args(["--shell", shell]);
        cmd.cwd(dir.join("work"));
        for k in [
            "SHELLVIEW",
            "SHELLVIEW_PID",
            "ZDOTDIR",
            "PROMPT_COMMAND",
            "BASH_ENV",
            "ENV",
        ] {
            cmd.env_remove(k);
        }
        cmd.env("HOME", dir.join("home"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("SHELLVIEW_STATE_DIR", dir.join("state"));
        cmd.env("SHELLVIEW_CACHE_DIR", dir.join("cache"));
        cmd.env("SHELLVIEW_CONFIG", dir.join("none.yaml"));
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Session {
            writer,
            master: pair.master,
            rx,
            screen: vt100::Parser::new(24, 100, 0),
            child,
            dir,
        }
    }

    fn pump(&mut self, for_: Duration) {
        let end = Instant::now() + for_;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(b) => self.screen.process(&b),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
    }

    fn text(&self) -> String {
        self.screen.screen().contents()
    }

    fn wait_for(&mut self, needle: &str) {
        let end = Instant::now() + WAIT;
        while Instant::now() < end {
            if self.text().contains(needle) {
                return;
            }
            self.pump(Duration::from_millis(100));
        }
        panic!("timed out waiting for {needle:?}; screen:\n{}", self.text());
    }

    fn count(&self, needle: &str) -> usize {
        self.text().matches(needle).count()
    }

    fn send(&mut self, s: &str) {
        self.writer.write_all(s.as_bytes()).unwrap();
        self.writer.flush().unwrap();
    }

    /// Type a command and wait for the next prompt.
    fn run(&mut self, cmd: &str) {
        let prompts = self.count("PROMPT>");
        self.send(&format!("{cmd}\r"));
        let end = Instant::now() + WAIT;
        while self.count("PROMPT>") <= prompts {
            assert!(
                Instant::now() < end,
                "no prompt after {cmd:?}; screen:\n{}",
                self.text()
            );
            self.pump(Duration::from_millis(100));
        }
    }

    fn wait_exit(&mut self) -> u32 {
        let end = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.exit_code();
            }
            assert!(
                Instant::now() < end,
                "shellview did not exit; screen:\n{}",
                self.text()
            );
            self.pump(Duration::from_millis(100));
        }
    }

    fn history(&self) -> String {
        std::fs::read_to_string(self.dir.join("state/history.jsonl")).unwrap_or_default()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn lifecycle(shell: &str) {
    let mut s = Session::start(shell, shell);
    s.wait_for("PROMPT>");

    // Ordinary commands work and render.
    s.run("echo hel''lo-from-${0##*/}");
    s.wait_for(&format!(
        "hello-from-{}",
        if shell == "zsh" { "zsh" } else { "bash" }
    ));

    // Hooks report the command, exit code and duration.
    s.run("cd /tmp && false");
    s.run("sleep 0.3");
    let h = s.history();
    assert!(h.contains(r#""cmd":"cd /tmp && false""#), "history:\n{h}");
    assert!(h.contains(r#""exit":1"#), "history:\n{h}");
    assert!(
        h.contains(r#""cwd":"/tmp""#),
        "cwd after cd should be /tmp:\n{h}"
    );
    let sleep_line = h.lines().find(|l| l.contains("sleep 0.3")).unwrap();
    let ms: u64 = sleep_line
        .split(r#""duration_ms":"#)
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(ms >= 250, "duration {ms}ms");

    // `sv` is on PATH and knows it is inside ShellView.
    s.run("sv status");
    s.wait_for("running (pid");
    s.wait_for(&format!("shell: {shell}"));
    s.wait_for("hooks: on");

    // Our private escape sequences never reach the screen.
    assert!(!s.text().contains("7770"), "{}", s.text());

    // `sv exit` leaves ShellView.
    s.send("sv exit\r");
    assert_eq!(s.wait_exit(), 0);
}

#[test]
fn bash_lifecycle() {
    if have("bash") {
        lifecycle("bash");
    }
}

#[test]
fn zsh_lifecycle() {
    if have("zsh") {
        lifecycle("zsh");
    }
}

#[test]
fn ctrl_q_quits() {
    let mut s = Session::start("bash", "ctrlq");
    s.wait_for("PROMPT>");
    s.send("\x11");
    assert_eq!(s.wait_exit(), 0);
}

#[test]
fn shell_exit_code_is_returned() {
    let mut s = Session::start("bash", "exitcode");
    s.wait_for("PROMPT>");
    s.send("exit 7\r");
    assert_eq!(s.wait_exit(), 7);
}

#[test]
fn resize_reaches_the_shell() {
    let mut s = Session::start("bash", "resize");
    s.wait_for("PROMPT>");
    s.master
        .resize(PtySize {
            rows: 30,
            cols: 90,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    s.screen.screen_mut().set_size(30, 90);
    s.pump(Duration::from_millis(300));
    s.run("stty size");
    s.wait_for("30 90");
}

#[test]
fn cursor_position_queries_are_answered() {
    // Programs like fzf and some prompts ask the terminal where the cursor is.
    let mut s = Session::start("bash", "dsr");
    s.wait_for("PROMPT>");
    // Like real programs (fzf etc.), turn echo off *before* asking.
    s.run(r#"stty -echo; printf '\033[6n'; IFS= read -rd R -t 3 r; stty echo; echo "got-${r#*[}-end""#);
    // Wait for the *output* line (the typed command also contains "got-").
    let end = Instant::now() + WAIT;
    let line = loop {
        if let Some(l) = s.text().lines().find(|l| l.starts_with("got-")) {
            break l.to_string();
        }
        assert!(Instant::now() < end, "no reply; screen:\n{}", s.text());
        s.pump(Duration::from_millis(50));
    };
    assert!(line.contains(';') && line.ends_with("-end"), "{line}");
}

#[test]
fn full_screen_apps_use_alternate_screen_and_come_back() {
    let mut s = Session::start("bash", "altscreen");
    s.wait_for("PROMPT>");
    s.run("echo before-app");
    s.send("printf '\\033[?1049h\\033[HFULLSCREEN-APP'; sleep 1; printf '\\033[?1049l'\r");
    // While the app runs, the shell's screen is hidden…
    s.wait_for("FULLSCREEN-APP");
    assert!(
        s.text()
            .lines()
            .next()
            .unwrap()
            .starts_with("FULLSCREEN-APP")
    );
    assert!(!s.text().contains("before-app"));
    // …and it comes back intact afterwards.
    s.wait_for("before-app");
    assert!(!s.text().lines().any(|l| l.starts_with("FULLSCREEN-APP")));
}

#[test]
fn vim_round_trip() {
    if !have("vim") {
        return;
    }
    let mut s = Session::start("bash", "vim");
    s.wait_for("PROMPT>");
    s.send("vim -u NONE -N note.txt\r");
    s.wait_for("note.txt");
    s.send("iShellView vim test\x1b:wq\r");
    s.wait_for("PROMPT>");
    s.run("cat note.txt");
    s.wait_for("ShellView vim test");
    let saved = std::fs::read_to_string(s.dir.join("work/note.txt")).unwrap();
    assert_eq!(saved.trim(), "ShellView vim test");
}

#[test]
fn already_inside_is_refused() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shellview"))
        .env("SHELLVIEW", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"));
}

#[test]
fn sv_outside_shellview() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shellview"))
        .args(["sv", "status"])
        .env_remove("SHELLVIEW")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("not running"));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shellview"))
        .args(["sv", "exit"])
        .env_remove("SHELLVIEW")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}
