//! The `sv` command.
//!
//! `sv` is the same binary as `shellview`, invoked under another name (or as
//! `shellview sv …`). Inside ShellView it talks to its parent by printing an
//! escape sequence to the terminal; outside it can start ShellView.

use std::io::Write;
use sv_proto::{ENV_INSIDE, ENV_PID, ENV_SHELL, ENV_VERSION, Mode};

const HELP: &str = "\
sv — control ShellView from your shell

usage:
  sv on        start ShellView here (or show the full view if already inside)
  sv off       switch to basic view (just your shell; everything keeps running)
  sv exit      leave ShellView
  sv status    is ShellView running in this terminal?
  sv version   print the version
  sv help      this help
";

pub fn inside() -> bool {
    std::env::var_os(ENV_INSIDE).is_some_and(|v| v == "1")
}

fn send(mode: Mode) -> i32 {
    // Write to the controlling terminal so it works even with stdout redirected.
    let seq = mode.sequence();
    let written = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .and_then(|mut tty| tty.write_all(seq.as_bytes()));
    if written.is_err() {
        let mut out = std::io::stdout();
        if out
            .write_all(seq.as_bytes())
            .and_then(|_| out.flush())
            .is_err()
        {
            eprintln!("sv: could not reach the terminal");
            return 1;
        }
    }
    0
}

fn not_running(cmd: &str) -> i32 {
    eprintln!("sv {cmd}: ShellView is not running in this terminal — start it with `sv on`");
    1
}

pub fn run(args: &[String]) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "on" => {
            if inside() {
                send(Mode::On)
            } else {
                start_shellview()
            }
        }
        "off" => {
            if inside() {
                send(Mode::Off)
            } else {
                not_running("off")
            }
        }
        "exit" | "quit" => {
            if inside() {
                send(Mode::Exit)
            } else {
                not_running("exit")
            }
        }
        "status" => {
            if inside() {
                let get = |k| std::env::var(k).unwrap_or_else(|_| "?".into());
                println!(
                    "ShellView {} · running (pid {}) · view: basic · shell: {} · hooks: {}",
                    get(ENV_VERSION),
                    get(ENV_PID),
                    get(ENV_SHELL),
                    get("SHELLVIEW_HOOKS")
                );
            } else {
                println!("ShellView is not running in this terminal (start it with `sv on`)");
            }
            0
        }
        "version" | "--version" | "-V" => {
            println!("sv {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            0
        }
        other => {
            eprintln!("sv: unknown command `{other}`\n\n{HELP}");
            2
        }
    }
}

/// Replace this process with ShellView, in the current folder.
fn start_shellview() -> i32 {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("sv on: cannot find the shellview binary: {e}");
            return 1;
        }
    };
    let err = std::process::Command::new(exe).arg0("shellview").exec();
    eprintln!("sv on: could not start ShellView: {err}");
    1
}
