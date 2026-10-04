"""Data-flow view: watch the shell's live job, or run a pipeline stage-by-stage."""
from __future__ import annotations

import os
import signal
import subprocess
import threading
import time
from collections import deque
from dataclasses import dataclass, field

import psutil
from rich.console import Group
from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.widgets import Button, Input, RichLog, Static, TabbedContent, TabPane

from .stats import human


# ---------------------------------------------------------------- parsing
def split_pipeline(cmd: str) -> list[str]:
    """Split on top-level '|' (not '||', not inside quotes / $( ) / backticks)."""
    parts, buf = [], []
    quote: str | None = None
    depth = 0
    i = 0
    while i < len(cmd):
        c = cmd[i]
        if c == "\\" and quote != "'":
            buf.append(cmd[i : i + 2])
            i += 2
            continue
        if quote:
            if c == quote:
                quote = None
        elif c in "'\"`":
            quote = c
        elif c == "(":
            depth += 1
        elif c == ")":
            depth = max(0, depth - 1)
        elif c == "|" and depth == 0:
            nxt = cmd[i + 1] if i + 1 < len(cmd) else ""
            if nxt == "|":
                buf.append("||")
                i += 2
                continue
            parts.append("".join(buf).strip())
            buf = []
            i += 1
            continue
        buf.append(c)
        i += 1
    parts.append("".join(buf).strip())
    return [p for p in parts if p]


# ---------------------------------------------------------------- runner
@dataclass
class Stage:
    cmd: str
    proc: subprocess.Popen | None = None
    bytes_in: int = 0
    lines_in: int = 0
    bytes_out: int = 0
    lines_out: int = 0
    last_line: str = ""
    stderr: deque = field(default_factory=lambda: deque(maxlen=50))
    rc: int | None = None
    started: float = 0.0
    ended: float | None = None
    rate_hist: deque = field(default_factory=lambda: deque(maxlen=24))
    _prev_out: int = 0


class PipelineRunner:
    def __init__(self, command: str, cwd: str, shell: str) -> None:
        self.command = command
        self.cwd = cwd
        self.shell = shell
        self.stages = [Stage(c) for c in split_pipeline(command)]
        self.output: deque[str] = deque()
        self.lock = threading.Lock()
        self.started = time.monotonic()
        self.stopped = False

    @property
    def done(self) -> bool:
        return all(s.rc is not None for s in self.stages)

    def start(self) -> None:
        n = len(self.stages)
        for i, st in enumerate(self.stages):
            st.proc = subprocess.Popen(
                st.cmd,
                shell=True,
                executable=self.shell if os.path.basename(self.shell) in ("bash", "zsh", "sh") else None,
                cwd=self.cwd,
                stdin=subprocess.DEVNULL if i == 0 else subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                bufsize=0,
                start_new_session=True,
            )
            st.started = time.monotonic()
        for i, st in enumerate(self.stages):
            nxt = self.stages[i + 1] if i + 1 < n else None
            threading.Thread(target=self._relay, args=(st, nxt), daemon=True).start()
            threading.Thread(target=self._stderr, args=(st,), daemon=True).start()
            threading.Thread(target=self._wait, args=(st,), daemon=True).start()

    def _relay(self, st: Stage, nxt: Stage | None) -> None:
        out = st.proc.stdout
        sink = nxt.proc.stdin if nxt else None
        partial = b""
        while True:
            try:
                chunk = os.read(out.fileno(), 65536)
            except OSError:
                break
            if not chunk:
                break
            nl = chunk.count(b"\n")
            st.bytes_out += len(chunk)
            st.lines_out += nl
            data = partial + chunk
            *complete, partial = data.split(b"\n")
            if complete:
                st.last_line = complete[-1].decode("utf-8", "replace")[:200]
            if len(partial) > 65536:
                partial = partial[-4096:]
            if nxt is not None:
                nxt.bytes_in += len(chunk)
                nxt.lines_in += nl
                if sink is not None:
                    try:
                        sink.write(chunk)
                    except (BrokenPipeError, OSError, ValueError):
                        sink = None  # downstream closed; keep draining
            elif complete:
                with self.lock:
                    self.output.extend(l.decode("utf-8", "replace") for l in complete)
        if partial:
            st.last_line = partial.decode("utf-8", "replace")[:200]
            if nxt is None:
                with self.lock:
                    self.output.append(partial.decode("utf-8", "replace"))
        if sink is not None:
            try:
                sink.close()
            except OSError:
                pass

    def _stderr(self, st: Stage) -> None:
        for line in iter(st.proc.stderr.readline, b""):
            st.stderr.append(line.decode("utf-8", "replace").rstrip())

    def _wait(self, st: Stage) -> None:
        st.rc = st.proc.wait()
        st.ended = time.monotonic()

    def stop(self) -> None:
        self.stopped = True
        for st in self.stages:
            if st.proc and st.proc.poll() is None:
                try:
                    os.killpg(st.proc.pid, signal.SIGTERM)
                except OSError:
                    pass

    def drain_output(self) -> list[str]:
        with self.lock:
            lines = list(self.output)
            self.output.clear()
        return lines


SPARK = " ▁▂▃▄▅▆▇█"


def spark(values) -> str:
    vals = list(values)
    if not vals:
        return ""
    top = max(vals) or 1
    return "".join(SPARK[min(8, int(v / top * 8))] for v in vals)


def render_pipeline(r: PipelineRunner, dt: float) -> Group:
    rows = []
    elapsed = time.monotonic() - r.started
    head = Text.assemble(("● " if not r.done else "✔ ", "green" if not r.done else "cyan"),
                         (f"{len(r.stages)} stages", "bold"), f"   {elapsed:6.1f}s   cwd ", (r.cwd, "dim"))
    rows.append(head)
    for i, st in enumerate(r.stages):
        rate = max(0, st.bytes_out - st._prev_out) / max(dt, 1e-3)
        st._prev_out = st.bytes_out
        if st.rc is None:
            st.rate_hist.append(rate)
        if st.rc is None:
            status = Text("▶ running", style="green")
        elif st.rc == 0:
            status = Text("✔ exit 0", style="cyan")
        else:
            status = Text(f"✖ exit {st.rc}", style="red")
        box = Text()
        box.append(f" [{i + 1}] ", style="bold reverse")
        box.append(f" {st.cmd}", style="bold")
        box.append("  ")
        box.append_text(status)
        rows.append(box)
        if i > 0:
            rows.append(Text(f"     in : {human(st.bytes_in):>8}  {st.lines_in:>9,} lines", style="dim"))
        rows.append(Text.assemble(
            f"     out: {human(st.bytes_out):>8}  {st.lines_out:>9,} lines  ",
            (f"{human(rate)}/s ".rjust(10), "yellow"),
            (spark(st.rate_hist), "yellow"),
        ))
        if st.last_line:
            rows.append(Text(f"     ▸ {st.last_line[:120]}", style="grey62 italic"))
        for e in list(st.stderr)[-3:]:
            rows.append(Text(f"     ! {e[:120]}", style="red"))
        if i + 1 < len(r.stages):
            rows.append(Text("      │\n      ▼", style="blue"))
    return Group(*rows)


# ---------------------------------------------------------------- live job
def render_live(terminal) -> Group:
    rows: list = []
    procs = terminal.foreground_processes()
    rows.append(Text.assemble(("cwd ", "bold"), (terminal.cwd, "cyan")))
    if not procs:
        kids = terminal.shell_children()
        rows.append(Text("Shell idle — waiting at prompt.", style="dim"))
        if kids:
            rows.append(Text(f"{len(kids)} background process(es):", style="dim"))
            for p in kids[:8]:
                try:
                    rows.append(Text(f"  {p.pid:>7}  {p.name()}", style="dim"))
                except psutil.Error:
                    pass
        rows.append(Text("\nRun something in the shell (e.g. `find / | grep x | wc -l`)\n"
                         "and each stage of the job shows up here live.", style="grey50"))
        return Group(*rows)
    rows.append(Text(f"Foreground job · {len(procs)} process(es)", style="bold green"))
    for i, p in enumerate(procs):
        try:
            with p.oneshot():
                name = p.name()
                cmd = " ".join(p.cmdline())[:90] or name
                cpu = p.cpu_percent(None)
                mem = p.memory_info().rss
                status = p.status()
                elapsed = time.time() - p.create_time()
            io = ""
            if hasattr(p, "io_counters"):
                try:
                    c = p.io_counters()
                    io = f"  R {human(c.read_bytes)} W {human(c.write_bytes)}"
                except (psutil.Error, OSError):
                    pass
        except psutil.Error:
            continue
        t = Text()
        t.append(f" {name} ", style="bold reverse")
        t.append(f"  pid {p.pid}  {status}", style="dim")
        rows.append(t)
        rows.append(Text(f"   {cmd}", style="grey62"))
        rows.append(Text.assemble("   cpu ", (f"{cpu:5.1f}%", "yellow"), "  rss ", (human(mem), "magenta"),
                                  f"  {elapsed:5.0f}s{io}"))
        if i + 1 < len(procs):
            rows.append(Text("    │ pipe\n    ▼", style="blue"))
    return Group(*rows)


class FlowPane(Vertical):
    DEFAULT_CSS = """
    FlowPane TabbedContent { height: 1fr; }
    FlowPane TabPane { padding: 0; }
    FlowPane #pipe-bar { height: 3; }
    FlowPane #pipe-input { width: 1fr; }
    FlowPane #pipe-stop { min-width: 8; }
    FlowPane #pipe-scroll { height: 1fr; }
    FlowPane #pipe-out { height: 10; border-top: solid $panel; }
    FlowPane #live { padding: 0 1; }
    """

    def __init__(self, terminal, **kw) -> None:
        super().__init__(**kw)
        self.terminal = terminal
        self.runner: PipelineRunner | None = None
        self._last = time.monotonic()

    def compose(self) -> ComposeResult:
        with TabbedContent(initial="tab-live"):
            with TabPane("Live job", id="tab-live"):
                with VerticalScroll():
                    yield Static(id="live")
            with TabPane("Pipeline", id="tab-pipe"):
                with Horizontal(id="pipe-bar"):
                    yield Input(placeholder="cat log | grep ERROR | sort | uniq -c   ⏎ run", id="pipe-input")
                    yield Button("Stop", id="pipe-stop", variant="error")
                with VerticalScroll(id="pipe-scroll"):
                    yield Static(id="pipe-diagram")
                yield RichLog(id="pipe-out", max_lines=2000, highlight=False, wrap=False)

    def on_mount(self) -> None:
        self.set_interval(1.0, self._tick_live)
        self.set_interval(0.5, self._tick_pipe)

    def _tick_live(self) -> None:
        self.query_one("#live", Static).update(render_live(self.terminal))

    def _tick_pipe(self) -> None:
        now = time.monotonic()
        dt, self._last = now - self._last, now
        r = self.runner
        if r is None:
            return
        self.query_one("#pipe-diagram", Static).update(render_pipeline(r, dt))
        log = self.query_one("#pipe-out", RichLog)
        for line in r.drain_output()[-500:]:
            log.write(Text(line))

    def on_input_submitted(self, event: Input.Submitted) -> None:
        cmd = event.value.strip()
        if not cmd:
            return
        if self.runner and not self.runner.done:
            self.runner.stop()
        self.query_one("#pipe-out", RichLog).clear()
        try:
            self.runner = PipelineRunner(cmd, self.terminal.cwd, self.terminal.shell)
            self.runner.start()
        except Exception as e:  # noqa: BLE001
            self.app.notify(f"Pipeline failed: {e}", severity="error")
            self.runner = None

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "pipe-stop" and self.runner:
            self.runner.stop()

    def run_pipeline(self, cmd: str) -> None:
        self.query_one(TabbedContent).active = "tab-pipe"
        inp = self.query_one("#pipe-input", Input)
        inp.value = cmd
        self.on_input_submitted(Input.Submitted(inp, cmd))

    def on_unmount(self) -> None:
        if self.runner:
            self.runner.stop()
