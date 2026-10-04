//! The event loop.
//!
//! Threads (all tiny, all blocking — no async runtime):
//!   input   stdin bytes            ─┐
//!   pty     shell output            ├─▶ channel ─▶ app (this thread): state + drawing
//!   wait    shell exit status       │
//!   signal  resize / terminate     ─┘
//!
//! The app thread redraws only when something changed, at most every 16 ms.

use crate::history::{Entry, History};
use crate::input::{self, Action};
use crate::shell::Launch;
use crate::term::{InputModes, Pane, TermView};
use crossterm::{cursor, execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;
use ratatui::style::{Modifier, Style};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use sv_proto::{Mode, ShellEvent};

const FRAME: Duration = Duration::from_millis(16);
const NOTICE: Duration = Duration::from_secs(3);

pub struct Options {
    pub launch: Launch,
    pub extra_env: Vec<(String, String)>,
    pub scrollback: usize,
    pub mouse: bool,
    pub history: History,
}

enum Ev {
    Input(Vec<u8>),
    Output(Vec<u8>),
    ChildExit(i32),
    Resize,
    Quit,
}

static RESTORED: AtomicBool = AtomicBool::new(false);

/// Put the real terminal back the way we found it. Safe to call twice.
fn restore_terminal() {
    if RESTORED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    let _ = out.write_all(InputModes::reset());
    let _ = execute!(out, terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
}

fn spawn<F: FnOnce() + Send + 'static>(name: &str, f: F) -> io::Result<()> {
    thread::Builder::new()
        .name(name.into())
        .stack_size(128 * 1024)
        .spawn(f)
        .map(|_| ())
}

pub fn run(opts: Options) -> io::Result<i32> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let spawned = Pane::spawn(&opts.launch, &opts.extra_env, rows, cols, opts.scrollback)?;
    let mut pane = spawned.pane;
    let (tx, rx) = mpsc::channel::<Ev>();

    {
        let tx = tx.clone();
        let mut reader = spawned.reader;
        spawn("pty", move || {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Ev::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
    }
    {
        let tx = tx.clone();
        let mut child = spawned.child;
        spawn("wait", move || {
            let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(1);
            let _ = tx.send(Ev::ChildExit(code));
        })?;
    }
    {
        use signal_hook::consts::{SIGHUP, SIGTERM, SIGWINCH};
        let tx = tx.clone();
        let mut signals = signal_hook::iterator::Signals::new([SIGWINCH, SIGTERM, SIGHUP])?;
        spawn("signal", move || {
            for s in signals.forever() {
                let ev = if s == SIGWINCH { Ev::Resize } else { Ev::Quit };
                if tx.send(ev).is_err() {
                    break;
                }
            }
        })?;
    }

    // Take over the screen. From here on, always restore — even on panic.
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), terminal::EnterAlternateScreen)?;
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    {
        let tx = tx.clone();
        spawn("input", move || {
            let stdin = io::stdin();
            let mut lock = stdin.lock();
            let mut buf = [0u8; 8192];
            loop {
                match lock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Ev::Input(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
    }
    drop(tx);

    let result = event_loop(&mut pane, rx, &opts);
    pane.kill();
    restore_terminal();
    result
}

struct State {
    cwd: String,
    command: String,
    started: Option<Instant>,
    notice: Option<(String, Instant)>,
    outer_modes: InputModes,
}

fn event_loop(pane: &mut Pane, rx: mpsc::Receiver<Ev>, opts: &Options) -> io::Result<i32> {
    let mut term = fresh_terminal()?;
    let mut st = State {
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        command: String::new(),
        started: None,
        notice: None,
        outer_modes: InputModes::default(),
    };
    sync_modes(&mut term, pane, &mut st, opts.mouse)?;

    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;

    loop {
        let timeout = if dirty {
            FRAME.saturating_sub(last_draw.elapsed())
        } else if let Some((_, at)) = &st.notice {
            (*at + NOTICE).saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(3600)
        };

        let mut batch = Vec::new();
        match rx.recv_timeout(timeout) {
            Ok(ev) => batch.push(ev),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(0),
        }
        while batch.len() < 512 {
            match rx.try_recv() {
                Ok(ev) => batch.push(ev),
                Err(_) => break,
            }
        }

        for ev in batch {
            match ev {
                Ev::Output(bytes) => {
                    let fx = pane.feed(&bytes);
                    if !fx.outer.is_empty() {
                        term.backend_mut().write_all(&fx.outer)?;
                        term.backend_mut().flush()?;
                    }
                    for e in fx.events {
                        if let Some(code) = on_shell_event(e, &mut st, opts) {
                            return Ok(code);
                        }
                    }
                    sync_modes(&mut term, pane, &mut st, opts.mouse)?;
                    dirty = true;
                }
                Ev::Input(bytes) => {
                    let own_mouse = opts.mouse && !pane.wants_mouse();
                    let page = (pane.screen().size().0 as isize / 2).max(1);
                    for action in input::parse(&bytes, own_mouse, page) {
                        match action {
                            Action::Quit => return Ok(0),
                            Action::ToggleView => {
                                notice(
                                    &mut st,
                                    "Full view arrives in the next release — this build is basic view only",
                                );
                                dirty = true;
                            }
                            Action::Scroll(n) if pane.in_alternate_screen() => {
                                // Like real terminals: wheel in a full-screen app = arrow keys.
                                let key: &[u8] = match (n > 0, pane.application_cursor()) {
                                    (true, true) => b"\x1bOA",
                                    (true, false) => b"\x1b[A",
                                    (false, true) => b"\x1bOB",
                                    (false, false) => b"\x1b[B",
                                };
                                pane.write(&key.repeat(n.unsigned_abs().min(3)))?;
                            }
                            Action::Scroll(n) => {
                                pane.scroll_by(n);
                                dirty = true;
                            }
                            Action::Forward(b) => {
                                if pane.scroll() > 0 {
                                    pane.scroll_by(isize::MIN / 2);
                                    dirty = true;
                                }
                                pane.write(&b)?;
                            }
                        }
                    }
                }
                Ev::Resize => {
                    let (cols, rows) = terminal::size().unwrap_or((80, 24));
                    pane.resize(rows, cols);
                    term = fresh_terminal()?;
                    dirty = true;
                }
                Ev::ChildExit(code) => return Ok(code),
                Ev::Quit => return Ok(0),
            }
        }

        if let Some((_, at)) = &st.notice {
            if at.elapsed() >= NOTICE {
                st.notice = None;
                dirty = true;
            }
        }

        if dirty && last_draw.elapsed() >= FRAME {
            draw(&mut term, pane, &st)?;
            dirty = false;
            last_draw = Instant::now();
        }
    }
}

/// A blank full-screen ratatui terminal. Built directly (not via
/// `Terminal::clear`/`autoresize`) so we never ask the real terminal for its
/// cursor position — that round-trip slows startup and fails on terminals
/// that don't answer.
fn fresh_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    let mut out = io::stdout();
    execute!(out, terminal::Clear(terminal::ClearType::All))?;
    Terminal::new(CrosstermBackend::new(out))
}

fn notice(st: &mut State, msg: &str) {
    st.notice = Some((msg.to_string(), Instant::now()));
}

/// Keep the real terminal's input modes in line with the program inside.
fn sync_modes<W: Write>(
    term: &mut Terminal<CrosstermBackend<W>>,
    pane: &Pane,
    st: &mut State,
    mouse: bool,
) -> io::Result<()> {
    let want = pane.input_modes().effective(mouse);
    if want != st.outer_modes {
        term.backend_mut().write_all(&want.diff(&st.outer_modes))?;
        term.backend_mut().flush()?;
        st.outer_modes = want;
    }
    Ok(())
}

/// Returns Some(exit code) when ShellView should quit.
fn on_shell_event(e: ShellEvent, st: &mut State, opts: &Options) -> Option<i32> {
    match e {
        ShellEvent::Cwd(p) => st.cwd = p,
        ShellEvent::CommandText(t) => st.command = t,
        ShellEvent::CommandStarted => st.started = Some(Instant::now()),
        ShellEvent::CommandFinished(code) => {
            if !st.command.is_empty() {
                let duration_ms = st
                    .started
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                opts.history.append(&Entry {
                    ts: History::now(),
                    cwd: &st.cwd,
                    cmd: &st.command,
                    exit: code,
                    duration_ms,
                    shell: &opts.launch.name,
                });
            }
            st.started = None;
            st.command.clear();
        }
        ShellEvent::PromptShown => {}
        ShellEvent::Mode(Mode::Exit) => return Some(0),
        ShellEvent::Mode(Mode::Off) => notice(st, "Already in basic view"),
        ShellEvent::Mode(Mode::On) => notice(
            st,
            "Full view arrives in the next release — this build is basic view only",
        ),
    }
    None
}

fn draw<W: Write>(
    term: &mut Terminal<CrosstermBackend<W>>,
    pane: &Pane,
    st: &State,
) -> io::Result<()> {
    term.draw(|f| {
        let area = f.area();
        f.render_widget(TermView(pane.screen()), area);
        let badge = Style::default().add_modifier(Modifier::REVERSED);
        let mut top_right = |text: &str, row: u16| {
            let w = text.chars().count() as u16;
            if area.width > w && row < area.height {
                f.buffer_mut().set_string(area.width - w, row, text, badge);
            }
        };
        if pane.scroll() > 0 {
            top_right(
                &format!(" ↑ {} lines · shift+PgDn or type to return ", pane.scroll()),
                0,
            );
        }
        if let Some((msg, _)) = &st.notice {
            top_right(&format!(" {msg} "), if pane.scroll() > 0 { 1 } else { 0 });
        }
        if let Some((x, y)) = pane.cursor() {
            f.set_cursor_position(Position { x, y });
        }
    })?;
    Ok(())
}
