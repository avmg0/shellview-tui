//! A terminal pane: a PTY running a shell, a vt100 screen, and the glue that
//! makes programs inside behave exactly as they would in the real terminal.

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use std::io::{self, Read, Write};
use sv_proto::ShellEvent;

use crate::shell::Launch;

/// Side effects collected while parsing shell output.
#[derive(Default)]
pub struct Effects {
    /// ShellView events reported by the shell hooks or `sv`.
    pub events: Vec<ShellEvent>,
    /// Answers to terminal queries (cursor position, device attributes) → back to the PTY.
    pub replies: Vec<u8>,
    /// Sequences to pass straight to the real terminal (bell, title, clipboard).
    pub outer: Vec<u8>,
}

/// vt100 callbacks: catch what the emulator doesn't handle itself.
#[derive(Default)]
pub struct Hooks {
    fx: Effects,
}

impl vt100::Callbacks for Hooks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.fx.outer.push(0x07);
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.fx.outer.extend_from_slice(b"\x1b]2;");
        self.fx
            .outer
            .extend(title.iter().filter(|b| !b.is_ascii_control()));
        self.fx.outer.push(0x07);
    }

    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, name: &[u8]) {
        self.fx.outer.extend_from_slice(b"\x1b]1;");
        self.fx
            .outer
            .extend(name.iter().filter(|b| !b.is_ascii_control()));
        self.fx.outer.push(0x07);
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        // OSC 52: lets vim / tmux-style "copy to system clipboard" keep working.
        self.fx.outer.extend_from_slice(b"\x1b]52;");
        self.fx.outer.extend_from_slice(ty);
        self.fx.outer.push(b';');
        self.fx.outer.extend_from_slice(data);
        self.fx.outer.push(0x07);
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let Some(ev) = sv_proto::parse_osc(params) {
            self.fx.events.push(ev);
        }
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (c, i1) {
            // Device status report: "are you ok?" / "where is the cursor?"
            ('n', None) if first == 5 => self.fx.replies.extend_from_slice(b"\x1b[0n"),
            ('n', None) if first == 6 => {
                let (row, col) = screen.cursor_position();
                self.fx
                    .replies
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // Primary device attributes: VT220-class with ANSI colour.
            ('c', None) if first == 0 => self.fx.replies.extend_from_slice(b"\x1b[?62;22c"),
            // Secondary device attributes.
            ('c', Some(b'>')) if first == 0 => self.fx.replies.extend_from_slice(b"\x1b[>1;10;0c"),
            _ => {}
        }
    }
}

/// Input modes a program inside can switch on. The real terminal must be in
/// the same mode, because it is the one encoding keys and mouse events.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct InputModes {
    keypad: bool,
    cursor: bool,
    paste: bool,
    mouse: vt100::MouseProtocolMode,
    encoding: vt100::MouseProtocolEncoding,
}

impl InputModes {
    /// The modes the real terminal should be in: the program's own, except
    /// that when it doesn't use the mouse and `capture` is on, ShellView takes
    /// mouse events itself (SGR encoding) so the wheel can scroll history.
    pub fn effective(self, capture: bool) -> Self {
        if capture && self.mouse == vt100::MouseProtocolMode::None {
            InputModes {
                mouse: vt100::MouseProtocolMode::PressRelease,
                encoding: vt100::MouseProtocolEncoding::Sgr,
                ..self
            }
        } else {
            self
        }
    }

    fn of(s: &vt100::Screen) -> Self {
        InputModes {
            keypad: s.application_keypad(),
            cursor: s.application_cursor(),
            paste: s.bracketed_paste(),
            mouse: s.mouse_protocol_mode(),
            encoding: s.mouse_protocol_encoding(),
        }
    }

    /// Escape sequences that move the real terminal from `prev` to `self`.
    pub fn diff(&self, prev: &InputModes) -> Vec<u8> {
        use vt100::MouseProtocolEncoding as E;
        use vt100::MouseProtocolMode as M;
        let mut out = Vec::new();
        if self.keypad != prev.keypad {
            out.extend_from_slice(if self.keypad { b"\x1b=" } else { b"\x1b>" });
        }
        if self.cursor != prev.cursor {
            out.extend_from_slice(if self.cursor {
                b"\x1b[?1h"
            } else {
                b"\x1b[?1l"
            });
        }
        if self.paste != prev.paste {
            out.extend_from_slice(if self.paste {
                b"\x1b[?2004h"
            } else {
                b"\x1b[?2004l"
            });
        }
        if self.mouse != prev.mouse {
            out.extend_from_slice(b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l");
            match self.mouse {
                M::None => {}
                M::Press => out.extend_from_slice(b"\x1b[?9h"),
                M::PressRelease => out.extend_from_slice(b"\x1b[?1000h"),
                M::ButtonMotion => out.extend_from_slice(b"\x1b[?1002h"),
                M::AnyMotion => out.extend_from_slice(b"\x1b[?1003h"),
            }
        }
        if self.encoding != prev.encoding {
            out.extend_from_slice(b"\x1b[?1005l\x1b[?1006l");
            match self.encoding {
                E::Default => {}
                E::Utf8 => out.extend_from_slice(b"\x1b[?1005h"),
                E::Sgr => out.extend_from_slice(b"\x1b[?1006h"),
            }
        }
        out
    }

    /// Sequences that turn every mode off (used on exit).
    pub fn reset() -> &'static [u8] {
        b"\x1b>\x1b[?1l\x1b[?2004l\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l"
    }
}

pub struct Pane {
    parser: vt100::Parser<Hooks>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Lines scrolled back from the bottom (0 = live).
    scroll: usize,
}

pub struct Spawned {
    pub pane: Pane,
    pub reader: Box<dyn Read + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Pane {
    pub fn spawn(
        launch: &Launch,
        extra_env: &[(String, String)],
        rows: u16,
        cols: u16,
        scrollback: usize,
    ) -> io::Result<Spawned> {
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(size(rows, cols))
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut cmd = CommandBuilder::new(&launch.program);
        cmd.args(&launch.args);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        cmd.env_remove("COLUMNS");
        cmd.env_remove("LINES");
        for (k, v) in launch.env.iter().chain(extra_env) {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::other(e.to_string()))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let killer = child.clone_killer();
        let parser = vt100::Parser::new_with_callbacks(rows, cols, scrollback, Hooks::default());
        Ok(Spawned {
            pane: Pane {
                parser,
                writer,
                master: pair.master,
                killer,
                scroll: 0,
            },
            reader,
            child,
        })
    }

    /// Feed shell output. Answers terminal queries directly and returns the
    /// remaining side effects for the caller.
    pub fn feed(&mut self, bytes: &[u8]) -> Effects {
        self.parser.process(bytes);
        let mut fx = std::mem::take(&mut self.parser.callbacks_mut().fx);
        if !fx.replies.is_empty() {
            let replies = std::mem::take(&mut fx.replies);
            let _ = self.write(&replies);
        }
        fx
    }

    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        let _ = self.master.resize(size(rows, cols));
        self.set_scroll(self.scroll);
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Scroll back by `delta` lines (negative = towards live output).
    pub fn scroll_by(&mut self, delta: isize) {
        let target = (self.scroll as isize + delta).max(0) as usize;
        self.set_scroll(target);
    }

    fn set_scroll(&mut self, lines: usize) {
        self.parser.screen_mut().set_scrollback(lines);
        // vt100 clamps to what is actually available.
        self.scroll = self.parser.screen().scrollback();
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Input modes the program inside has asked for.
    pub fn input_modes(&self) -> InputModes {
        InputModes::of(self.parser.screen())
    }

    /// Does the program inside want mouse events itself?
    pub fn wants_mouse(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn in_alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// Where the cursor should be drawn, if visible.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        let s = self.parser.screen();
        if self.scroll > 0 || s.hide_cursor() {
            return None;
        }
        let (row, col) = s.cursor_position();
        Some((col, row))
    }
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Draws a vt100 screen into a ratatui buffer.
pub struct TermView<'a>(pub &'a vt100::Screen);

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut m = Modifier::empty();
    if cell.bold() {
        m |= Modifier::BOLD;
    }
    if cell.dim() {
        m |= Modifier::DIM;
    }
    if cell.italic() {
        m |= Modifier::ITALIC;
    }
    if cell.underline() {
        m |= Modifier::UNDERLINED;
    }
    if cell.inverse() {
        m |= Modifier::REVERSED;
    }
    Style::default()
        .fg(color(cell.fgcolor()))
        .bg(color(cell.bgcolor()))
        .add_modifier(m)
}

impl Widget for TermView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (rows, cols) = self.0.size();
        for row in 0..area.height.min(rows) {
            let y = area.y + row;
            let mut col = 0;
            while col < area.width.min(cols) {
                let x = area.x + col;
                let Some(cell) = self.0.cell(row, col) else {
                    break;
                };
                if cell.is_wide_continuation() {
                    col += 1;
                    continue;
                }
                let style = cell_style(cell);
                let text = if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                };
                if cell.is_wide() && col + 1 < area.width {
                    buf.set_stringn(x, y, text, 2, style);
                    col += 2;
                } else {
                    buf[(x, y)].set_symbol(text).set_style(style);
                    col += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> vt100::Parser<Hooks> {
        vt100::Parser::new_with_callbacks(5, 20, 100, Hooks::default())
    }

    #[test]
    fn shellview_osc_becomes_event_and_is_not_drawn() {
        let mut p = parser();
        p.process(b"hi\x1b]7;file://h/tmp\x07\x1b]133;D;2\x1b\\there");
        let fx = std::mem::take(&mut p.callbacks_mut().fx);
        assert_eq!(
            fx.events,
            vec![
                ShellEvent::Cwd("/tmp".into()),
                ShellEvent::CommandFinished(Some(2))
            ]
        );
        assert_eq!(p.screen().contents(), "hithere");
    }

    #[test]
    fn osc_split_across_reads() {
        let mut p = parser();
        p.process(b"\x1b]7770;mo");
        p.process(b"de;exit\x07");
        let fx = std::mem::take(&mut p.callbacks_mut().fx);
        assert_eq!(fx.events, vec![ShellEvent::Mode(sv_proto::Mode::Exit)]);
    }

    #[test]
    fn cursor_position_query_is_answered() {
        let mut p = parser();
        p.process(b"abc\x1b[6n");
        let fx = std::mem::take(&mut p.callbacks_mut().fx);
        assert_eq!(fx.replies, b"\x1b[1;4R");
    }

    #[test]
    fn title_and_bell_pass_through() {
        let mut p = parser();
        p.process(b"\x1b]2;vim\x07\x07");
        let fx = std::mem::take(&mut p.callbacks_mut().fx);
        assert_eq!(fx.outer, b"\x1b]2;vim\x07\x07");
    }

    #[test]
    fn input_modes_are_mirrored() {
        let mut p = parser();
        let before = InputModes::of(p.screen());
        p.process(b"\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h");
        let after = InputModes::of(p.screen());
        let d = after.diff(&before);
        let s = String::from_utf8_lossy(&d);
        assert!(s.contains("\x1b[?1h") && s.contains("\x1b[?2004h"));
        assert!(s.contains("\x1b[?1002h") && s.contains("\x1b[?1006h"));
        assert!(after.diff(&after).is_empty());
        let back = before.diff(&after);
        assert!(String::from_utf8_lossy(&back).contains("\x1b[?2004l"));
    }

    #[test]
    fn renders_wide_and_styled_cells() {
        let mut p = parser();
        p.process("\x1b[1;31mR\x1b[0m界x".as_bytes());
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        TermView(p.screen()).render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "R");
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(1));
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(1, 0)].symbol(), "界");
        assert_eq!(buf[(3, 0)].symbol(), "x");
    }
}
