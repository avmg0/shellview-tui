//! ShellView — a multi-pane workspace around your real zsh or bash.
//!
//! One binary, two names:
//!   `shellview [--shell zsh|bash|PATH]`  start ShellView
//!   `sv <command>`                       control it from inside your shell

mod app;
mod cli;
mod config;
mod history;
mod input;
mod paths;
mod shell;
mod term;

use std::path::{Path, PathBuf};
use sv_proto::{ENV_INSIDE, ENV_PID, ENV_SHELL, ENV_VERSION};

const USAGE: &str = "\
shellview — your shell, plus a workspace around it

usage:
  shellview [options]
  shellview sv <command>      (same as running `sv <command>`)

options:
  -s, --shell <zsh|bash|PATH>  shell to run (default: config, then $SHELL)
      --no-integration         don't add ShellView's shell hooks
  -h, --help                   this help
  -V, --version                print the version

inside ShellView:
  ctrl+q                quit
  shift+PgUp/PgDn       scroll back (or use the mouse wheel)
  sv off | sv exit      basic view | leave
";

struct Args {
    shell: Option<String>,
    integration: bool,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        shell: None,
        integration: true,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-s" | "--shell" => {
                out.shell = Some(it.next().ok_or("--shell needs a value")?.clone());
            }
            s if s.starts_with("--shell=") => out.shell = Some(s["--shell=".len()..].to_string()),
            "--no-integration" => out.integration = false,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(out)
}

/// Make `sv` callable inside the shell: `<cache>/bin/sv` → this executable.
fn install_sv_link(cache: &Path) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin = cache.join("bin");
    std::fs::create_dir_all(&bin).ok()?;
    let link = bin.join("sv");
    if std::fs::read_link(&link).ok().as_deref() != Some(exe.as_path()) {
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&exe, &link).ok()?;
    }
    Some(bin)
}

fn main() {
    let argv0 = std::env::args_os()
        .next()
        .map(PathBuf::from)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if argv0 == "sv" {
        std::process::exit(cli::run(&args));
    }
    match args.first().map(String::as_str) {
        Some("sv") => std::process::exit(cli::run(&args[1..])),
        Some("-h" | "--help") => {
            print!("{USAGE}");
            return;
        }
        Some("-V" | "--version") => {
            println!("shellview {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => {}
    }

    let opts = match parse_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("shellview: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if cli::inside() {
        eprintln!(
            "shellview: already running in this terminal — use `sv off` / `sv on` to switch views"
        );
        std::process::exit(1);
    }

    let (cfg, warning) = config::Config::load(&paths::config_file());
    if let Some(w) = warning {
        eprintln!("shellview: {w}");
    }

    let cache = paths::cache_dir();
    let choice = opts.shell.as_deref().or(cfg.shell.as_deref());
    let launch = match shell::build(choice, opts.integration && cfg.shell_integration, &cache) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("shellview: {e}");
            std::process::exit(1);
        }
    };

    let mut env = vec![
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "ShellView".into()),
        (
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        ),
        (ENV_INSIDE.into(), "1".into()),
        (ENV_PID.into(), std::process::id().to_string()),
        (ENV_VERSION.into(), env!("CARGO_PKG_VERSION").into()),
        (ENV_SHELL.into(), launch.name.clone()),
        (
            "SHELLVIEW_HOOKS".into(),
            if launch.integrated { "on" } else { "off" }.into(),
        ),
    ];
    if let Some(bin) = install_sv_link(&cache) {
        let path = std::env::var("PATH").unwrap_or_default();
        env.push(("PATH".into(), format!("{}:{path}", bin.display())));
    }

    let result = app::run(app::Options {
        launch,
        extra_env: env,
        scrollback: cfg.scrollback_lines,
        mouse: cfg.mouse,
        history: history::History::new(&paths::state_dir()),
    });
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("shellview: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args() {
        let a = parse_args(&s(&["--shell", "bash"])).unwrap();
        assert_eq!(a.shell.as_deref(), Some("bash"));
        assert!(a.integration);
        let a = parse_args(&s(&["--shell=zsh", "--no-integration"])).unwrap();
        assert_eq!(a.shell.as_deref(), Some("zsh"));
        assert!(!a.integration);
        assert!(parse_args(&s(&["--shell"])).is_err());
        assert!(parse_args(&s(&["--bogus"])).is_err());
    }
}
