# ShellView — Architecture

Status: v0.2 · 2026-10-04 · P0 implemented (see Decisions log at the end)
Wireframes: `docs/wireframes/` (to be exported) · Phases: [ROADMAP.md](ROADMAP.md)

---

## 1. What ShellView is

A full-screen terminal UI that runs **inside the user's existing terminal app** (Terminal.app, iTerm2, Ghostty, WezTerm, Kitty) — like `tmux`, `htop` or `vim` — and splits the window into panes:

- **Shell** — real zsh or bash, many tabs and splits
- **Files** — 8 view styles (tree, columns, list, grid, size map, git, find, recent)
- **System** — CPU, memory, disk, network, processes, thermals
- **Data flow** — the shell's live job and a stage-by-stage pipeline runner
- **Agents** — multiple coding agents (and their sub-agents) side by side, manual or auto-planned
- **Activity** — downloads, installs and long-running commands

Plus `sv`, a set of structured (nushell-style) commands that work inside the shell.

### Goals
1. Every existing shell command, alias, script and full-screen app keeps working unchanged.
2. Tiny footprint: ShellView itself ≤ 15 MB RAM idle, < 0.5 % CPU idle, ≤ 8 MB binary (agents' own memory excluded).
3. One binary, no runtime dependencies, macOS (Apple Silicon) first, Linux second.
4. Can be turned off and on at any moment without losing running work (`sv off` / `sv on`).

### Non-goals (for now)
- Being a terminal emulator app (we run *inside* one).
- Replacing zsh/bash with a new shell language.
- Windows support.
- Remote/collaborative sessions (beyond running agents over ssh).

---

## 2. Process model

```
 Terminal.app / iTerm2 / Ghostty            (the user's terminal)
 └── shellview                              (ONE process: UI + state, ~10–15 MB)
     ├── PTY ─ zsh   (tab 1)               ┐
     ├── PTY ─ bash  (tab 2, split a)      │ real shells, one PTY each
     ├── PTY ─ zsh   (tab 2, split b)      ┘
     ├── PTY ─ claude (agent "api", hidden tab)   ┐ agent CLIs run in their own
     │        └── PTY ─ claude (sub-agent "ws-push") │ PTYs; their memory is theirs
     ├── PTY ─ codex  (agent "ui")                ┘
     └── short-lived helpers: git, du-walk threads, pipeline stages (only on demand)

 sv <cmd>   ← same binary, invoked by the user or an agent inside any shell
            talks to its parent ShellView via (a) OSC escape on the tty or
            (b) a private unix socket — never the network
```

- **No daemon, no background service.** Everything lives as long as the `shellview` process. Quitting ends it all (with a warning if commands or agents are still running).
- **Multi-call binary:** `shellview` starts the UI; `sv …` (symlink or argv[0] check) runs a CLI subcommand.

---

## 3. Runtime model (threads & events)

Chosen for minimal memory and predictable latency — **std threads + channels, no async runtime**.

| Thread | Job | Wakes on |
|---|---|---|
| **app** (main) | owns all state, handles events, decides redraws, renders with ratatui | channel messages |
| **input** | reads terminal input (crossterm), forwards key/mouse/resize events | stdin |
| **pty-io** | reads shell output (P0: one small blocking reader thread per PTY; a single `polling` reactor for all PTYs once tabs/agents multiply them) | PTY readable |
| **sampler** | system stats + thermals + process list; sleeps when no visible pane needs it | timer (2 s / 5 s low-power) |
| **fs-watch** | `notify` (FSEvents on macOS) for the expanded folders only | file events |
| **workers (≤2)** | on-demand jobs: git status/diff, size scan, find, pipeline relays | job queue; exit when idle |
| **bus** | agent/CLI unix socket (only started when the first agent or `sv` query appears) | socket |

```
 input ─┐
 pty-io ─┼──▶ crossbeam channel ──▶ app thread ──▶ state update ──▶ dirty regions ──▶ render (≤ 60 fps, coalesced)
 sampler ─┤
 fs-watch ┤
 workers ─┤
 bus ─────┘
```

Rules that keep it light (from the performance budget board):
- Redraw **only on change**; frames are coalesced, capped at 60 fps, idle = 0 frames.
- **Hidden panes sleep**: sampler, fs-watch and parsers unsubscribe when nothing visible needs them.
- Scrollback capped (5,000 lines/tab, configurable); agent output kept in 2,000-line ring buffers, full logs streamed to disk.
- File tree loads folders lazily; size map and find only run when asked and are cancellable.

---

## 4. Code layout (single Cargo workspace)

Start as **one binary crate with clear modules** plus one small protocol crate. Split into more crates only if compile times hurt.

```
shellview/
├── Cargo.toml                 # workspace
├── crates/
│   ├── shellview/             # the binary (UI + everything below as modules)
│   │   └── src/
│   │       ├── main.rs        # argv[0] dispatch: shellview | sv
│   │       ├── app/           # App state, event loop, focus, commands, toasts
│   │       ├── config/        # config.yaml load/watch/defaults, keymap, layouts
│   │       ├── ui/            # ratatui widgets, theme, layout tree, editor, status bar
│   │       ├── input/         # key router: global → leader → pane → PTY passthrough
│   │       ├── term/          # PTY spawn, vt100 screen, scrollback, OSC extraction
│   │       ├── shell/         # zsh/bash launch + embedded integration scripts
│   │       ├── tabs/          # tabs, splits, session save/restore
│   │       ├── files/         # 8 views, lazy tree, preview, git (CLI), size, find
│   │       ├── sys/           # sysinfo stats, processes, thermals (macOS IOKit/SMC)
│   │       ├── flow/          # live-job inspector, pipeline runner
│   │       ├── activity/      # download/install detection, progress parsers
│   │       ├── data/          # `sv` tables: types, sources, filters, command-bar parser
│   │       ├── agents/        # registry, tree, limits, approvals, worktrees, adapters
│   │       └── cli/           # `sv` subcommands (on/off/exit, agent, connect, data…)
│   └── sv-proto/              # OSC 7770 + socket message types (serde), shared by app & cli
├── shell/                     # integration scripts (embedded with include_str!)
│   ├── zsh/  zshenv zprofile zshrc zlogin  (installed as dotfiles at runtime)
│   └── bash/ shellview.bash
├── docs/                      # ARCHITECTURE.md, ROADMAP.md, decisions/
├── tests/                     # integration tests with real PTYs
└── prototype/                 # the earlier Python/Textual prototype — reference only
```

### Main dependencies (all chosen for size and maturity)

| Need | Crate | Note |
|---|---|---|
| Drawing | `ratatui` + `crossterm` | immediate-mode, diffed output |
| PTYs | `portable-pty` | spawn shells and agent CLIs |
| Terminal emulation | `vt100` | fallback if gaps: `alacritty_terminal` (bigger) |
| System stats | `sysinfo` | + small macOS FFI for thermals |
| File watching | `notify` | FSEvents on macOS |
| Find | `ignore` + `grep-searcher` | respects .gitignore |
| Config | `serde` + `serde_yaml_ng` | maintained fork of serde_yaml |
| Channels | `std::sync::mpsc` (P0), `polling` later | no async runtime |
| Keychain | `security-framework` | API keys for agents |
| Git | **git CLI** on demand | avoids libgit2 size |

Release profile: `opt-level = "z"` or `"s"` (measure), `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.

---

## 5. Core building blocks

### 5.1 Terminal pane (`term/`)
- Each shell/agent = **PTY + `vt100::Parser` + scrollback ring**.
- Output path: `pty bytes → OSC extractor → vt100 parser → dirty rows → render`.
- The OSC extractor strips ShellView's own sequences (see 5.3) before the emulator sees them and turns them into events.
- Full-screen apps (vim, htop, less) use the alternate screen; ShellView just renders whatever vt100 holds.
- Mouse: passed through when the app inside requested mouse mode; otherwise used for focus/selection.

### 5.2 Shell integration (`shell/`) — *already built and tested in the prototype*
- **zsh:** start with `ZDOTDIR=<embedded dir>`; forwarder files source the user's real `.zshenv/.zprofile/.zshrc/.zlogin`, then add `precmd`/`preexec` hooks; `ZDOTDIR` is handed back.
- **bash:** `bash --rcfile shellview.bash -i`; emulates login startup (`/etc/profile`, `~/.bash_profile` | `~/.bash_login` | `~/.profile`), then `PROMPT_COMMAND` + `DEBUG` trap. Works with macOS bash 3.2. Caveat: replaces an existing DEBUG trap.
- Other shells run without hooks; cwd falls back to polling the process.

### 5.3 OSC protocol (`sv-proto`)

| Sequence | Sender | Meaning |
|---|---|---|
| `OSC 7 ; file://host/path` | shell hook | current folder |
| `OSC 133 ; A` | shell hook | prompt shown |
| `OSC 133 ; C` | shell hook | command started |
| `OSC 133 ; D ; <exit>` | shell hook | command finished + exit code |
| `OSC 7770 ; cmd ; <base64>` | shell hook | command text |
| `OSC 7770 ; mode ; on\|off\|exit` | `sv on/off/exit` | switch view (planned) |
| `OSC 7770 ; table ; <id>` | `sv` data cmds | "render this table inline" marker (planned) |

These drive: file pane following `cd`, tab states (running / ✔ / ✖ exit code), finish toasts, `sv history`, download detection, and the Flow pane.

### 5.4 Layout tree (`ui/`, `config/`)
- A layout is a tree: `Row | Col` nodes with child sizes (%), leaves are pane kinds (`shell`, `files.tree`, `stats`, `flow`, `thermals`, `agents`, `activity`, `git`, `preview`).
- Presets (Dev, Monitor, Files, Agents, Focus, Basic) + user layouts live in `config.yaml`; the editor (alt+e) edits the same tree live.
- **Basic view** = a layout with a single shell leaf and no chrome; everything else is suspended, not killed.

### 5.5 Input routing (`input/`)
```
key ─▶ global keys (ctrl+q, alt+`, F1, F5…)   — always win, even inside vim
    ─▶ leader key (ctrl+] then letter)
    ─▶ focused ShellView pane (files, agents, tables…)
    ─▶ PTY of the focused shell / agent       — everything else
```
- Detects Option-as-Alt misconfiguration on macOS (e.g. `alt+1` arriving as `¡`) and shows the setup helper once.
- All bindings overridable in `config.yaml`.

### 5.6 Structured data — `sv` (`data/`, `cli/`)
- Value model: `Table = Vec<Row>`, `Row = ordered map`, cell types: string, int, float, size, duration, date, bool, list, record.
- Between `sv` stages: **JSON lines** over the pipe (detected because stdout isn't a tty).
- At the end of a pipe (stdout is ShellView's tty): emits a table marker → ShellView renders an interactive table inline (sort / filter / copy).
- Piped into a normal command: plain aligned text, or `--json` / `--csv`.
- Command bar (alt+;) uses its own parser with nu-style operators (`> < == =~ and or`); in shell pipes the same filters use word operators (`gt lt eq ne has`).

### 5.7 Agents (`agents/`)
```
             ┌──────────── ShellView agent runtime ────────────┐
 team.yaml ─▶│ registry ── tree (parent/child) ── limits       │
 Auto-plan ─▶│    │            │                    │          │
 sv agent  ─▶│ spawner ─▶ PTY + adapter ─▶ normalized events ─▶ UI columns / tree / timeline
             │    │         (claude / codex / gemini / generic) │
             │ approvals queue · budget ledger · worktree mgr   │
             │ message bus (unix socket, JSON lines, 0600)      │
             └──────────────────────────────────────────────────┘
```
- **Agent** = an agent CLI in its own PTY (hidden tab) + an **adapter** that turns its output into events: `Thinking`, `ToolCall`, `FileEdit{path,+,-}`, `Command`, `NeedsApproval`, `Usage{tokens,cost}`, `SpawnRequest`, `Done`. CLIs with structured output modes are parsed; others fall back to a generic text adapter.
- **Sub-agents:** any agent can run `sv agent spawn …` in its shell → bus → runtime checks limits (depth ≤ 3, agents ≤ 10, children ≤ 3 by default) → spawns child with inherited budget share, approval rules and worktree. Over-limit requests are refused with a reason. Agents' *internal* sub-agents (e.g. a CLI's own helper agents) are shown dotted when the adapter can see them.
- **Isolation:** one `git worktree` + branch per top-level agent; children share their parent's.
- **Approvals:** policy from team.yaml (auto / ask / never per action class: read, test, lint, install, network, push, delete).
- **Budget:** ledger per agent, rolled up per branch and team; pause-all at the cap.
- **Auto-plan:** an orchestrator agent proposes a staged task graph → user edits → dispatch.
- **Connections:** detects installed agent CLIs and login state, configured MCP servers, ssh hosts; API keys in macOS Keychain, never displayed or logged.

### 5.8 System & thermals (`sys/`)
- `sysinfo` for CPU/mem/disk/net/processes; process list sampled only while visible.
- macOS thermals: thermal state via `NSProcessInfo.thermalState` (public API); per-sensor temps and fans via IOKit/SMC (best effort — show "not available" rather than guess).
- ShellView reports its own RSS/CPU ("self") so the budget is always visible.

---

## 6. Files on disk

| Path | Purpose |
|---|---|
| `~/.config/shellview/config.yaml` | settings, keymap overrides, layouts, theme |
| `~/.local/state/shellview/session.yaml` | last session's tabs/splits/folders (restore) |
| `~/.local/state/shellview/history.jsonl` | command history with exit code, duration, tab |
| `~/.local/state/shellview/agents/<run>/` | full agent logs, usage ledger |
| `<project>/.shellview/team.yaml` | agent team script |
| `$TMPDIR/shellview-<uid>/bus.sock` | agent/CLI socket (0600, removed on exit) |

---

## 7. Security & safety
- No network listener; the bus is a unix socket readable only by the user.
- API keys only in Keychain; redacted from logs and agent output panes.
- Agents work in worktrees; anything destructive outside them is denied by default.
- Destructive UI actions (kill process, delete files, stop agent tree, close busy tab) always confirm.
- Shell integration never changes the user's dotfiles; `sv autostart on` adds a single, clearly marked line and `sv autostart off` removes it.

---

## 8. Performance budget & how it is enforced

| Metric | Target |
|---|---|
| Binary | ≤ 8 MB |
| RSS idle (1 shell, 4 panes) | ≤ 15 MB |
| RSS basic view | ≈ 6–8 MB |
| CPU idle | < 0.5 % (low-power < 0.1 %) |
| Startup | < 100 ms |
| Per extra tab | +2–4 MB |
| Per agent column | ~+1 MB (agent CLI excluded) |

Enforced by: a `bench/` harness that launches ShellView in a pseudo-terminal, idles 30 s and records RSS/CPU; CI fails if binary size or idle RSS regress past the budget.

---

## 9. Testing
- **Unit:** layout tree math, keymap routing, OSC extractor, `sv` parser and filters, limits/approvals logic.
- **Snapshot:** ratatui `TestBackend` renders of each pane compared to golden text frames (the wireframes become the spec).
- **Integration:** real zsh and bash (incl. macOS bash 3.2) in PTYs — cd tracking, exit codes, vim/htop passthrough, sv on/off.
- **Agents:** fake agent CLI that replays recorded transcripts, so adapters and tree logic are tested without spending tokens.
- **Budget:** see §8.

---

## 10. Open questions
1. File views for first release: all 8, or Tree + List + Git + Find first?
2. Leader key: `ctrl+]` (current pick) or `ctrl+space`?
3. ~~YAML crate choice~~ → `serde_yaml_ng` (decided in P0).
4. Thermals on Apple Silicon: which sensors are readable without elevated permissions?
5. Agent adapters: which CLIs expose structured output reliably enough to parse sub-agents and usage?
6. Session restore: offer scrollback restore at all (privacy vs convenience)?

---

## 11. Decisions log

| Date | Decision | Why |
|---|---|---|
| 2026-10-04 | **Input is forwarded byte-for-byte** in basic view; ShellView only picks out its own keys (`ctrl+q`, `alt+\``, `shift+PgUp/PgDn`) and, when it owns the mouse, wheel events. A key decoder is added in P1 for pane-local keys. | Zero translation loss: every key, paste, IME and mouse sequence reaches the shell exactly as the real terminal sent it. |
| 2026-10-04 | **Input modes are mirrored** to the real terminal (application cursor/keypad, bracketed paste, mouse mode + encoding). | The real terminal encodes keys and mouse events, so it must be in the mode the program inside asked for. |
| 2026-10-04 | **ShellView answers terminal queries itself** (cursor position `CSI 6n`, status `CSI 5n`, device attributes `CSI c` / `CSI > c`); passes bell, window title and OSC 52 clipboard through. | Programs inside talk to ShellView's emulator, not the real terminal. |
| 2026-10-04 | **Mouse wheel = scrollback** by default (ShellView enables SGR mouse reporting when the program inside doesn't use the mouse). In full-screen apps without mouse support the wheel becomes arrow keys, like real terminals do. Config `mouse: false` turns it off. | On macOS, `shift+PgUp` is taken by Terminal.app/iTerm2, so the wheel is the only reliable scroll gesture. Trade-off: text selection needs Option (iTerm2) or turning off mouse reporting (Terminal.app). |
| 2026-10-04 | **Never query the real terminal's cursor position** (ratatui `Terminal::clear`/`autoresize` are avoided; the screen is cleared with a plain escape and the ratatui terminal is rebuilt on resize). | Saves a round-trip at startup and works with terminals that don't answer. |
| 2026-10-04 | `sv` is linked as `<cache>/bin/sv` and that folder is prepended to `PATH` inside ShellView. | `sv` works in every shell tab with no install step. |

### P0 measurements (Linux x86_64, release build, bash)

| Metric | Budget | Measured |
|---|---|---|
| Binary | ≤ 5 MB (P0) | 0.76 MB |
| First frame | ≤ 100 ms | 4 ms |
| Memory idle | ≤ 8 MB (P0) | 3.5 MB |
| CPU idle | ≤ 0.5 % | 0.00 % |
