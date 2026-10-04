"""A real interactive shell running inside a Textual widget (PTY + pyte)."""
from __future__ import annotations

import asyncio
import fcntl
import base64
import os
import pty
import re
import time
from urllib.parse import unquote, urlparse
import signal
import struct
import termios

import psutil
import pyte
from rich.segment import Segment
from rich.style import Style
from textual import events
from textual.geometry import Size
from textual.message import Message
from textual.strip import Strip
from textual.widget import Widget

from .shells import build_launch

# OSC sequences emitted by the bash/zsh integration scripts
OSC_RE = re.compile(rb"\x1b\](7|133|7770);([^\x07\x1b]*)(?:\x07|\x1b\\)")

DECCKM = 1 << 5  # application cursor keys (private mode 1, pyte shifts by 5)

_NAMED = {"black", "red", "green", "brown", "blue", "magenta", "cyan", "white"}

KEYMAP = {
    "enter": "\r",
    "tab": "\t",
    "shift+tab": "\x1b[Z",
    "escape": "\x1b",
    "backspace": "\x7f",
    "ctrl+h": "\x08",
    "home": "\x1b[H",
    "end": "\x1b[F",
    "pageup": "\x1b[5~",
    "pagedown": "\x1b[6~",
    "delete": "\x1b[3~",
    "insert": "\x1b[2~",
    "ctrl+left": "\x1b[1;5D",
    "ctrl+right": "\x1b[1;5C",
    "alt+left": "\x1bb",
    "alt+right": "\x1bf",
    "shift+left": "\x1b[1;2D",
    "shift+right": "\x1b[1;2C",
    "ctrl+space": "\x00",
    "ctrl+@": "\x00",
    "f8": "\x1b[19~",
    "f9": "\x1b[20~",
    "f10": "\x1b[21~",
    "f11": "\x1b[23~",
    "f12": "\x1b[24~",
}
ARROWS = {"up": "A", "down": "B", "right": "C", "left": "D"}


def _rich_color(c: str) -> str | None:
    if c == "default":
        return None
    bright = c.startswith("bright")
    base = c[6:] if bright else c
    if base == "brown":
        base = "yellow"
    if base in _NAMED or base == "yellow":
        return ("bright_" + base) if bright else base
    if len(c) == 6:
        try:
            int(c, 16)
            return "#" + c
        except ValueError:
            return None
    return None


class _PyteScreen(pyte.Screen):
    """pyte screen that can answer terminal queries (cursor pos, device attrs)."""

    def __init__(self, columns: int, lines: int, writer) -> None:
        super().__init__(columns, lines)
        self._writer = writer
        self.title = ""

    def write_process_input(self, data: str) -> None:  # noqa: D401
        self._writer(data.encode())


class Terminal(Widget, can_focus=True):
    """Interactive PTY-backed shell."""

    DEFAULT_CSS = """
    Terminal { width: 1fr; height: 1fr; }
    """

    class CwdChanged(Message):
        def __init__(self, path: str) -> None:
            super().__init__()
            self.path = path

    class Exited(Message):
        pass

    class CommandStarted(Message):
        def __init__(self, command: str) -> None:
            super().__init__()
            self.command = command

    class CommandFinished(Message):
        def __init__(self, command: str, exit_code: int, duration: float) -> None:
            super().__init__()
            self.command = command
            self.exit_code = exit_code
            self.duration = duration

    def __init__(self, shell: str | None = None, cwd: str | None = None, **kw) -> None:
        super().__init__(**kw)
        self.launch = build_launch(shell)
        self.shell = self.launch.path
        self.shell_name = self.launch.name
        self.current_command = ""
        self._cmd_started: float | None = None
        self._osc_buf = b""
        self._osc_cwd = False  # True once the shell reports cwd itself (OSC 7)
        self.start_dir = cwd or os.getcwd()
        self.pid: int | None = None
        self.fd: int | None = None
        self.cwd = self.start_dir
        self._screen = _PyteScreen(80, 24, self._write_bytes)
        self._stream = pyte.ByteStream(self._screen)
        self._dirty = False
        self._styles: dict = {}

    # ------------------------------------------------------------------ PTY
    def _spawn(self, cols: int, rows: int) -> None:
        self._screen.resize(rows, cols)
        pid, fd = pty.fork()
        if pid == 0:  # child
            env = os.environ.copy()
            env["TERM"] = "xterm-256color"
            env["COLORTERM"] = "truecolor"
            env["SHELLVIEW"] = "1"
            env.pop("COLUMNS", None)
            env.pop("LINES", None)
            env.update(self.launch.env)
            try:
                os.chdir(self.start_dir)
            except OSError:
                pass
            os.execvpe(self.shell, self.launch.argv, env)
        self.pid, self.fd = pid, fd
        self._set_winsize(cols, rows)
        asyncio.get_running_loop().add_reader(fd, self._on_readable)
        self.set_interval(1 / 30, self._maybe_refresh)
        self.set_interval(0.75, self._poll_cwd)

    def _set_winsize(self, cols: int, rows: int) -> None:
        if self.fd is not None:
            fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def _on_readable(self) -> None:
        try:
            data = os.read(self.fd, 65536)
        except OSError:
            data = b""
        if not data:
            asyncio.get_running_loop().remove_reader(self.fd)
            self.post_message(self.Exited())
            return
        data = self._handle_osc(data)
        if data:
            self._stream.feed(data)
        self._dirty = True

    def _handle_osc(self, data: bytes) -> bytes:
        data = self._osc_buf + data
        self._osc_buf = b""
        tail = data.rfind(b"\x1b]")
        if tail != -1 and b"\x07" not in data[tail:] and b"\x1b\\" not in data[tail:] and len(data) - tail < 8192:
            self._osc_buf, data = data[tail:], data[:tail]
        if b"\x1b]" not in data:
            return data
        for m in OSC_RE.finditer(data):
            kind, payload = m.group(1), m.group(2).decode("utf-8", "replace")
            if kind == b"7":
                path = unquote(urlparse(payload).path)
                self._osc_cwd = True
                if path and path != self.cwd:
                    self.cwd = path
                    self.post_message(self.CwdChanged(path))
            elif kind == b"7770" and payload.startswith("cmd;"):
                try:
                    self.current_command = base64.b64decode(payload[4:]).decode("utf-8", "replace")
                except ValueError:
                    self.current_command = ""
            elif kind == b"133":
                if payload == "C":
                    self._cmd_started = time.monotonic()
                    self.post_message(self.CommandStarted(self.current_command))
                elif payload.startswith("D"):
                    parts = payload.split(";")
                    code = int(parts[1]) if len(parts) > 1 and parts[1].lstrip("-").isdigit() else 0
                    dur = time.monotonic() - self._cmd_started if self._cmd_started else 0.0
                    self.post_message(self.CommandFinished(self.current_command, code, dur))
                    self._cmd_started = None
                    self.current_command = ""
        return OSC_RE.sub(b"", data)

    def _maybe_refresh(self) -> None:
        if self._dirty:
            self._dirty = False
            if self._screen.title:
                self.border_title = self._screen.title
                if self.parent is not None:
                    self.parent.border_subtitle = self._screen.title[:60]
            self.refresh()

    def _write_bytes(self, data: bytes) -> None:
        if self.fd is None:
            return
        try:
            os.write(self.fd, data)
        except OSError:
            pass

    def write(self, text: str) -> None:
        """Send text to the shell as if typed."""
        self._write_bytes(text.encode())

    def _poll_cwd(self) -> None:
        if self.pid is None or self._osc_cwd:
            return
        try:
            cwd = psutil.Process(self.pid).cwd()
        except (psutil.Error, OSError):
            return
        if cwd and cwd != self.cwd:
            self.cwd = cwd
            self.post_message(self.CwdChanged(cwd))

    def foreground_processes(self) -> list[psutil.Process]:
        """Processes in the shell's foreground job (e.g. every stage of a pipeline)."""
        if self.fd is None or self.pid is None:
            return []
        try:
            fg = os.tcgetpgrp(self.fd)
        except OSError:
            return []
        if fg == self.pid:
            return []
        try:
            kids = psutil.Process(self.pid).children(recursive=True)
        except psutil.Error:
            return []
        out = []
        for p in kids:
            try:
                if os.getpgid(p.pid) == fg:
                    out.append(p)
            except OSError:
                pass
        return sorted(out, key=lambda p: p.pid)

    def shell_children(self) -> list[psutil.Process]:
        try:
            return psutil.Process(self.pid).children(recursive=True) if self.pid else []
        except psutil.Error:
            return []

    # ------------------------------------------------------------ lifecycle
    def on_resize(self, event: events.Resize) -> None:
        cols, rows = max(event.size.width, 10), max(event.size.height, 3)
        if self.pid is None:
            self._spawn(cols, rows)
        else:
            self._screen.resize(rows, cols)
            self._set_winsize(cols, rows)
        self.refresh()

    def on_unmount(self) -> None:
        self.terminate()

    def terminate(self) -> None:
        if self.pid:
            try:
                os.kill(self.pid, signal.SIGHUP)
            except OSError:
                pass
            self.pid = None

    # ------------------------------------------------------------- input
    def on_key(self, event: events.Key) -> None:
        key = event.key
        seq: str | None = None
        if key in ARROWS:
            app_mode = DECCKM in self._screen.mode
            seq = ("\x1bO" if app_mode else "\x1b[") + ARROWS[key]
        elif key in KEYMAP:
            seq = KEYMAP[key]
        elif event.character:
            seq = event.character
        elif key.startswith("ctrl+") and len(key) == 6 and key[-1].isalpha():
            seq = chr(ord(key[-1].lower()) - 96)
        elif key.startswith("alt+") and len(key) == 5:
            seq = "\x1b" + key[-1]
        if seq is not None:
            event.prevent_default()
            event.stop()
            self._write_bytes(seq.encode())

    def on_paste(self, event: events.Paste) -> None:
        event.stop()
        text = event.text
        if (2004 << 5) in self._screen.mode:
            text = f"\x1b[200~{text}\x1b[201~"
        self.write(text)

    def on_mouse_scroll_up(self, event) -> None:
        event.stop()

    def on_mouse_scroll_down(self, event) -> None:
        event.stop()

    def on_focus(self) -> None:
        self.refresh()

    def on_blur(self) -> None:
        self.refresh()

    # ------------------------------------------------------------ render
    def _style(self, ch) -> Style:
        key = (ch.fg, ch.bg, ch.bold, ch.italics, ch.underscore, ch.reverse, ch.strikethrough)
        st = self._styles.get(key)
        if st is None:
            st = Style(
                color=_rich_color(ch.fg),
                bgcolor=_rich_color(ch.bg),
                bold=ch.bold or None,
                italic=ch.italics or None,
                underline=ch.underscore or None,
                reverse=ch.reverse or None,
                strike=ch.strikethrough or None,
            )
            self._styles[key] = st
        return st

    def get_content_width(self, container: Size, viewport: Size) -> int:
        return container.width

    def get_content_height(self, container: Size, viewport: Size, width: int) -> int:
        return container.height

    def render_line(self, y: int) -> Strip:
        width = self.size.width
        scr = self._screen
        if y >= scr.lines:
            return Strip.blank(width)
        row = scr.buffer[y]
        cur = scr.cursor
        show_cursor = self.has_focus and not cur.hidden and cur.y == y
        segs: list[Segment] = []
        run: list[str] = []
        run_style: Style | None = None
        for x in range(min(scr.columns, width)):
            ch = row[x]
            if ch.data == "":  # right half of a wide char
                continue
            st = self._style(ch)
            if show_cursor and x == cur.x:
                st = st + Style(reverse=not ch.reverse)
            if st is not run_style and run:
                segs.append(Segment("".join(run), run_style))
                run = []
            run_style = st
            run.append(ch.data)
        if run:
            segs.append(Segment("".join(run), run_style))
        return Strip(segs).adjust_cell_length(width)
