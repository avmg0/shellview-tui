//! Performance budget check.
//!
//!   cargo build --release && cargo run --release --example bench [-- --check]
//!
//! Starts the release `shellview` in a pseudo-terminal with bash, waits for
//! the prompt, idles, and reports binary size, startup time, memory and idle
//! CPU of the ShellView process itself (the shell is not counted).
//! With `--check`, exits 1 if any P0 budget is exceeded.

use portable_pty::{CommandBuilder, PtySize};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

// P0 budgets (docs/ARCHITECTURE.md §8).
const MAX_BINARY_MB: f64 = 5.0;
const MAX_RSS_MB: f64 = 8.0;
const MAX_IDLE_CPU_PCT: f64 = 0.5;
const MAX_FIRST_FRAME_MS: u128 = 100;

fn release_binary() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/shellview");
    root.canonicalize().unwrap_or(root)
}

fn rss_kb(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Total CPU time (user + system) used by `pid`, in seconds.
fn cpu_secs(pid: u32) -> Option<f64> {
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        let after = stat.rsplit_once(')')?.1;
        let f: Vec<&str> = after.split_whitespace().collect();
        let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        return Some(ticks / hz);
    }
    // macOS: "M:SS.ss"
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut secs = 0.0;
    for part in t.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let idle = Duration::from_secs(
        std::env::var("BENCH_IDLE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10),
    );
    let bin = release_binary();
    let size_mb = std::fs::metadata(&bin)
        .unwrap_or_else(|_| panic!("build first: cargo build --release ({})", bin.display()))
        .len() as f64
        / 1_048_576.0;

    let tmp = std::env::temp_dir().join(format!("sv-bench-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("home")).unwrap();
    std::fs::write(tmp.join("home/.bash_profile"), "PS1='BENCH> '\n").unwrap();

    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(&bin);
    cmd.args(["--shell", "bash"]);
    cmd.env_remove("SHELLVIEW");
    cmd.env("HOME", tmp.join("home"));
    cmd.env("TERM", "xterm-256color");
    cmd.env("SHELLVIEW_STATE_DIR", tmp.join("state"));
    cmd.env("SHELLVIEW_CACHE_DIR", tmp.join("cache"));
    cmd.env("SHELLVIEW_CONFIG", tmp.join("none.yaml"));

    let started = Instant::now();
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let pid = child.process_id().unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // First frame = ShellView switched to the alternate screen and drew.
    let mut seen = Vec::new();
    let mut first_frame = None;
    let mut prompt = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while prompt.is_none() && Instant::now() < deadline {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(20)) {
            if first_frame.is_none() {
                first_frame = Some(started.elapsed());
            }
            seen.extend_from_slice(&b);
            if seen.windows(6).any(|w| w == b"BENCH>") {
                prompt = Some(started.elapsed());
            }
        }
    }
    let first_frame = first_frame.expect("no output from shellview");
    let prompt = prompt.expect("no prompt within 10s");

    std::thread::sleep(Duration::from_secs(2)); // settle
    while rx.try_recv().is_ok() {}
    let cpu0 = cpu_secs(pid).unwrap_or(0.0);
    std::thread::sleep(idle);
    let cpu1 = cpu_secs(pid).unwrap_or(0.0);
    let rss_mb = rss_kb(pid).unwrap_or(0) as f64 / 1024.0;
    let idle_cpu = (cpu1 - cpu0) / idle.as_secs_f64() * 100.0;

    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&tmp);

    let row = |name: &str, value: String, budget: String, ok: bool| {
        println!(
            "{:<22} {:>12}   budget {:<10} {}",
            name,
            value,
            budget,
            if ok { "ok" } else { "OVER" }
        );
        ok
    };
    println!("ShellView P0 budget ({}s idle)\n", idle.as_secs());
    let mut ok = true;
    ok &= row(
        "binary size",
        format!("{size_mb:.2} MB"),
        format!("≤ {MAX_BINARY_MB} MB"),
        size_mb <= MAX_BINARY_MB,
    );
    ok &= row(
        "first frame",
        format!("{} ms", first_frame.as_millis()),
        format!("≤ {MAX_FIRST_FRAME_MS} ms"),
        first_frame.as_millis() <= MAX_FIRST_FRAME_MS,
    );
    println!(
        "{:<22} {:>12}   (includes bash startup)",
        "prompt visible",
        format!("{} ms", prompt.as_millis())
    );
    ok &= row(
        "memory (RSS) idle",
        format!("{rss_mb:.1} MB"),
        format!("≤ {MAX_RSS_MB} MB"),
        rss_mb <= MAX_RSS_MB,
    );
    ok &= row(
        "CPU idle",
        format!("{idle_cpu:.2} %"),
        format!("≤ {MAX_IDLE_CPU_PCT} %"),
        idle_cpu <= MAX_IDLE_CPU_PCT,
    );

    if check && !ok {
        std::process::exit(1);
    }
}
