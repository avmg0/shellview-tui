"""Real-time system stats pane."""
from __future__ import annotations

import os
import time

import psutil
from rich.table import Table
from rich.text import Text
from textual import work
from textual.app import ComposeResult
from textual.containers import Vertical
from textual.widgets import DataTable, Sparkline, Static

HISTORY = 60


def human(n: float, suffix: str = "B") -> str:
    for unit in ("", "K", "M", "G", "T"):
        if abs(n) < 1024:
            return f"{n:.0f}{unit}{suffix}" if unit == "" else f"{n:.1f}{unit}{suffix}"
        n /= 1024
    return f"{n:.1f}P{suffix}"


def bar(pct: float, width: int = 18) -> Text:
    pct = max(0.0, min(100.0, pct or 0.0))
    filled = round(pct / 100 * width)
    color = "green" if pct < 60 else "yellow" if pct < 85 else "red"
    t = Text("█" * filled, style=color)
    t.append("░" * (width - filled), style="grey35")
    t.append(f" {pct:5.1f}%")
    return t


class StatsPane(Vertical):
    DEFAULT_CSS = """
    StatsPane Sparkline { height: 2; margin: 0 1; }
    StatsPane #cpu-label, StatsPane #net-label { padding: 0 1; color: $text-muted; }
    StatsPane #stats-body { padding: 0 1; height: auto; }
    StatsPane DataTable { height: 1fr; min-height: 4; }
    """

    def __init__(self, **kw) -> None:
        super().__init__(**kw)
        self.cpu_hist: list[float] = [0.0] * HISTORY
        self.net_hist: list[float] = [0.0] * HISTORY
        self._last_net = psutil.net_io_counters()
        self._last_disk = psutil.disk_io_counters()
        self._last_t = time.monotonic()
        psutil.cpu_percent(percpu=True)

    def compose(self) -> ComposeResult:
        yield Static("CPU", id="cpu-label")
        yield Sparkline(self.cpu_hist, id="cpu-spark")
        yield Static("NET", id="net-label")
        yield Sparkline(self.net_hist, id="net-spark")
        yield Static(id="stats-body")
        table = DataTable(id="procs", cursor_type="row", zebra_stripes=True)
        table.add_columns("PID", "NAME", "CPU%", "MEM%")
        yield table

    def on_mount(self) -> None:
        self.set_interval(1.0, self.refresh_stats)
        self.set_interval(2.0, self.refresh_procs)
        self.refresh_procs()

    def refresh_stats(self) -> None:
        now = time.monotonic()
        dt = max(now - self._last_t, 1e-3)
        self._last_t = now
        cores = psutil.cpu_percent(percpu=True)
        total = sum(cores) / max(len(cores), 1)
        self.cpu_hist = (self.cpu_hist + [total])[-HISTORY:]
        self.query_one("#cpu-spark", Sparkline).data = self.cpu_hist

        net = psutil.net_io_counters()
        rx = (net.bytes_recv - self._last_net.bytes_recv) / dt
        tx = (net.bytes_sent - self._last_net.bytes_sent) / dt
        self._last_net = net
        self.net_hist = (self.net_hist + [rx + tx])[-HISTORY:]
        self.query_one("#net-spark", Sparkline).data = self.net_hist
        self.query_one("#cpu-label", Static).update(
            Text.assemble(("CPU ", "bold"), f"{total:5.1f}%  ", ("cores ", "dim"), str(len(cores)))
        )
        self.query_one("#net-label", Static).update(
            Text.assemble(("NET ", "bold"), ("↓ ", "cyan"), f"{human(rx)}/s  ", ("↑ ", "magenta"), f"{human(tx)}/s")
        )

        vm = psutil.virtual_memory()
        sw = psutil.swap_memory()
        try:
            du = psutil.disk_usage(os.path.expanduser("~"))
        except OSError:
            du = psutil.disk_usage("/")
        disk = psutil.disk_io_counters()
        if disk and self._last_disk:
            rd = (disk.read_bytes - self._last_disk.read_bytes) / dt
            wr = (disk.write_bytes - self._last_disk.write_bytes) / dt
        else:
            rd = wr = 0.0
        self._last_disk = disk

        t = Table.grid(padding=(0, 1))
        t.add_column(style="bold", width=5)
        t.add_column()
        t.add_column(style="dim")
        # per-core mini bars, packed
        core_line = Text()
        for i, c in enumerate(cores):
            ch = " ▁▂▃▄▅▆▇█"[min(8, int(c / 12.5))]
            color = "green" if c < 60 else "yellow" if c < 85 else "red"
            core_line.append(ch, style=color)
        t.add_row("Cores", core_line, "")
        t.add_row("Mem", bar(vm.percent), f"{human(vm.used)}/{human(vm.total)}")
        t.add_row("Swap", bar(sw.percent), f"{human(sw.used)}/{human(sw.total)}")
        t.add_row("Disk", bar(du.percent), f"{human(du.free)} free")
        t.add_row("I/O", Text(f"R {human(rd)}/s  W {human(wr)}/s"), "")
        try:
            l1, l5, l15 = os.getloadavg()
            t.add_row("Load", Text(f"{l1:.2f} {l5:.2f} {l15:.2f}"), "")
        except OSError:
            pass
        up = time.time() - psutil.boot_time()
        t.add_row("Up", Text(f"{int(up // 86400)}d {int(up % 86400 // 3600)}h {int(up % 3600 // 60)}m"), "")
        try:
            batt = psutil.sensors_battery()
        except Exception:
            batt = None
        if batt:
            t.add_row("Batt", bar(batt.percent), "charging" if batt.power_plugged else "battery")
        self.query_one("#stats-body", Static).update(t)

    @work(thread=True, exclusive=True, group="procs")
    def refresh_procs(self) -> None:
        rows = []
        for p in psutil.process_iter(["pid", "name", "cpu_percent", "memory_percent"]):
            info = p.info
            rows.append(
                (
                    info["cpu_percent"] or 0.0,
                    str(info["pid"]),
                    (info["name"] or "?")[:22],
                    info["memory_percent"] or 0.0,
                )
            )
        rows.sort(reverse=True)
        self.app.call_from_thread(self._update_table, rows[:25])

    def _update_table(self, rows) -> None:
        table = self.query_one("#procs", DataTable)
        table.clear()
        for cpu, pid, name, mem in rows:
            table.add_row(pid, name, f"{cpu:5.1f}", f"{mem:4.1f}")
