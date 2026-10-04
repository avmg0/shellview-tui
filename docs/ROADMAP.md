# ShellView — Roadmap & Phases

Status: v0.2 · 2026-10-04 · **P0 done** · see [ARCHITECTURE.md](ARCHITECTURE.md)

Each phase ends with something you can **run and use daily**, and must stay inside the performance budget before the next phase starts. Sizes are relative (S ≈ a few days, M ≈ 1–2 weeks, L ≈ 2–4 weeks of focused work) — not promises.

| Phase | Name | Size | You can use it for… | Boards |
|---|---|---|---|---|
| **P0** ✅ | Foundations | M | ShellView as a plain, fast shell wrapper (basic view) | 11, 14 |
| **P1** | Workspace | L | tabs, splits, layouts, keymap — a tmux-like daily driver | 3, 4, 4b, 12, 14 |
| **P2** | Core panes | L | the default 4-pane workspace (files, stats, activity) | 1, 6, 9, 10 |
| **P3** | Data | M | data-flow view and `sv` structured commands | 2, 13 |
| **P4** | Agents I | L | run several coding agents side by side, manually | 5, 8, 16 |
| **P5** | Agents II | L | sub-agents, team.yaml, auto-plan | 7, 15, 16 |
| **P6** | Polish & ship | M | the rest of the file views, layout editor, packaging | 4, 9, 10, 11 |

---

## P0 · Foundations — ✅ done 2026-10-04
**Goal:** one Rust binary that runs your real zsh/bash full-screen with zero visible difference — the "basic view".

Deliverables
- Cargo workspace, CI (macOS arm64 + Linux), release profile tuned for size.
- Config loading (`config.yaml`) with defaults; pick the YAML crate.
- Event loop: input thread, PTY reactor, app thread, coalesced redraw.
- Terminal pane: PTY + vt100 + scrollback; resize; mouse passthrough; paste.
- Port the existing zsh/bash integration scripts (embedded) and the OSC extractor.
- Multi-call binary: `shellview` and `sv` (`sv status`, `sv exit`).
- Budget harness (`bench/`) + CI size/RSS checks.

Exit criteria
- vim, htop, less, ssh, fzf, git all behave exactly as in the bare terminal.
- cwd / command / exit code events fire for zsh, bash 5 and macOS bash 3.2.
- Binary ≤ 5 MB, RSS ≤ 8 MB idle, CPU ≈ 0 % idle, startup < 100 ms.

Risks: vt100 gaps with exotic apps → keep `alacritty_terminal` as plan B.

**Result:** all exit criteria met on Linux (bash 5.2, zsh 5.9, vim): 0.76 MB binary, 4 ms first frame, 3.5 MB RSS, 0 % idle CPU. 26 unit + 10 end-to-end PTY tests. macOS (bash 3.2, Terminal.app/iTerm2) is covered by CI and needs a hands-on check.

---

## P1 · Workspace
**Goal:** the frame everything else plugs into.

Deliverables
- Layout tree (row/col with sizes), pane framework (focus, borders, titles, zoom F5).
- Tabs (alt+t/F10, alt+1…9, rename, close-with-running dialog) and splits (alt+\ / alt+-).
- Tab states from shell events (running, ✔, ✖ exit, new output, waiting for input, bell).
- Keymap router: global → leader (ctrl+]) → pane → PTY; config overrides; F1 help.
- Mac key setup helper (Option-as-Alt detection).
- `sv on` / `sv off` / `sv exit` / alt+` with full ⇄ basic view.
- Status bar + toasts; session save/restore.
- Presets: Dev (empty pane placeholders), Focus, Basic.

Exit criteria: usable as a daily tmux-replacement; idle RSS ≤ 10 MB with 3 tabs.

---

## P2 · Core panes
**Goal:** board 1 working for real.

Deliverables
- **Files:** lazy tree with git + agent badges, filter, hidden toggle; List view; Git changes view; Find (names + contents); preview with light syntax colors; follows shell cwd; insert path / cd / open in $EDITOR.
- **System:** CPU (sparkline, per-core), memory, swap, disk, network, I/O, load, process list (visible-only sampling), "self" usage; **thermals v1** (thermal state + whatever sensors are readable).
- **Activity:** detect git clone / npm / pip / uv / brew / docker / cargo / curl from command text; progress parsing; loading circle in header, tab and pane; finish toasts.
- Presets: Dev, Monitor, Files.
- Low-power mode (manual + auto on battery).

Exit criteria: default workspace runs all day at ≤ 15 MB, < 0.5 % CPU idle.

---

## P3 · Data
**Goal:** see and shape data flowing through commands.

Deliverables
- **Flow pane:** live foreground job (processes, cpu, rss, pipes); pipeline runner with per-stage bytes/lines/rate, filter ratio, bottleneck, per-stage stderr, output tail; "send to shell".
- **`sv` structured commands:** value model, JSON-lines between stages, plain-text/`--json`/`--csv` output; sources (ls, ps, sys, df, ports, env, history, open json/csv/toml/yaml, git-log, git-status); filters (where, select, reject, first, last, skip, uniq, get, sort-by, reverse, group-by, count, sum, avg, min, max, from, to).
- Inline interactive tables (sort, filter, copy as csv/json/md).
- Command bar (alt+;) with nu-style parser; sv pipelines shown in Flow with rows per stage.

Exit criteria: board 13 examples all work in zsh and bash; tables of 100k rows stay responsive.

---

## P4 · Agents I — side by side
**Goal:** run several coding agents manually and watch them live.

Deliverables
- Agent runtime: spawn CLI in hidden PTY, adapters (Claude Code, Codex, Gemini CLI, Aider, generic), normalized events.
- Agents view (F8): columns with live output, status, tokens/cost, files touched; message an agent; Manual "New agent" dialog; attach an agent already running in a tab.
- Git worktree per agent; file-overlap warnings; file tree badges.
- Approvals queue (allow / deny / always) with per-action policy.
- Connections (`sv connect`): detect CLIs + login, MCP servers, Keychain keys, ssh hosts.
- Agents preset layout.

Exit criteria: 3 agents on one repo for an hour without UI lag; ShellView's own overhead ≤ +3 MB.

---

## P5 · Agents II — teams & sub-agents
**Goal:** orchestration.

Deliverables
- Message bus (unix socket); `sv agent` CLI (run, spawn, send, wait, tree, logs -f, pause/resume, stop --tree, new).
- Agent tree with limits (depth/agents/children), inheritance, budget roll-up, branch actions; detection of CLIs' internal sub-agents where visible.
- `team.yaml` (team, agents, on-event rules) with validation and clear errors.
- Auto-plan: goal → staged task graph → edit/reassign/drop → dispatch / step-by-step / re-plan; orchestrator review before merge.
- Timeline view; message-bus log.

Exit criteria: the board 16 team.yaml runs end-to-end; over-limit spawns are refused with a reason; budget cap pauses everything.

---

## P6 · Polish & ship
Deliverables
- Remaining file views: Columns, Grid, Size map, Recent activity.
- Layout editor (alt+e) with palette, drag-resize, save/default/per-folder layouts.
- Full thermal view (history, throttling alerts, per-sensor list).
- Themes (dark/light), accessibility pass (contrast, no color-only signals).
- Packaging: Homebrew tap, `cargo install`, signed release binaries; `sv autostart`.
- User docs and `sv help`.

Exit criteria: v1.0 — all boards implemented, budget met, install in one command.

---

## Cross-cutting, every phase
- Budget checks in CI (size, idle RSS, idle CPU).
- Snapshot tests for every new pane against the wireframes.
- Real-shell integration tests (zsh, bash 5, macOS bash 3.2).
- Update ARCHITECTURE.md decisions log when anything changes.

## Before P0 starts
- [x] Designs signed off.
- [x] YAML crate chosen (`serde_yaml_ng`). Still open: first file views, leader key.
- [x] Python prototype moved into `prototype/` (reference only).
