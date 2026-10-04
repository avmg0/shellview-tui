# ShellView

**Your real shell, plus a workspace around it — inside the terminal you already use.**

ShellView is a full-screen terminal app (like `tmux`, `htop` or `vim`) that runs your own zsh or bash and, as it grows, splits the window into panes: files, live system stats, a data-flow view of your pipelines, and coding agents working side by side. Every command, alias and full-screen program you use today keeps working unchanged.

> **Status: early development (phase P0 of 7).**
> This release is the foundation: ShellView runs your shell full-screen in *basic view* — it looks and behaves exactly like a plain terminal while tracking folders, commands and exit codes in the background. The multi-pane workspace arrives in P1–P2. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Why

- **One binary, tiny footprint.** Written in Rust. Budget: ≤ 8 MB binary, ≤ 15 MB memory, < 0.5 % CPU when idle. The current build is ~0.8 MB and uses ~3.5 MB of memory.
- **Nothing to relearn.** Your `.zshrc` / `.bash_profile` load as usual; vim, less, fzf, ssh and git all behave as before.
- **Off is one command away.** `sv off` hides the panels, `sv exit` leaves completely.

## Install (from source)

Requires Rust 1.85+.

```sh
git clone https://github.com/avmg0/shellview-tui
cd shellview-tui
cargo install --path crates/shellview
```

This installs a single `shellview` binary. Inside ShellView, `sv` is available automatically. To use `sv on` from a plain terminal, link it once:

```sh
ln -s "$(command -v shellview)" ~/.cargo/bin/sv
```

## Use

```sh
shellview                 # start with your login shell
shellview --shell bash    # or pick: zsh, bash, or a full path
```

| Key / command | What it does |
|---|---|
| `ctrl+q` | quit ShellView |
| mouse wheel · `shift+PgUp` / `shift+PgDn` | scroll back through output |
| `sv status` | is ShellView running here? which shell? |
| `sv on` | start ShellView in this folder (full view, once it exists) |
| `sv off` | basic view — just your shell |
| `sv exit` | leave ShellView |

**Selecting text:** ShellView uses the mouse wheel for scrollback. To select text with the mouse, hold **Option** (iTerm2) or turn off *View → Allow Mouse Reporting* (Terminal.app). Or set `mouse: false` in the config.

## Configuration

`~/.config/shellview/config.yaml` — every key is optional:

```yaml
shell: zsh              # zsh, bash or a full path (default: $SHELL)
scrollback_lines: 5000  # lines of history kept per shell
shell_integration: true # report folder / command / exit code to ShellView
mouse: true             # wheel scrolls history
```

## How the shell integration works

ShellView adds small hooks to your shell *without editing your dotfiles*:

- **zsh** starts with a temporary `ZDOTDIR` whose files source your real `.zshenv`, `.zprofile`, `.zshrc` and `.zlogin`, then add `precmd`/`preexec` hooks.
- **bash** starts with `--rcfile`, which runs your normal login files, then adds a `PROMPT_COMMAND` and a `DEBUG` trap. Works with macOS's bash 3.2 and newer bash. *Note: this replaces an existing `DEBUG` trap.*
- Other shells run fine, just without the hooks.

The hooks print invisible escape sequences (`OSC 7`, `OSC 133`, `OSC 7770`) that ShellView reads and removes. Command history with exit codes and durations is kept in `~/.local/state/shellview/history.jsonl`.

## Development

```sh
cargo test --all                                       # unit + end-to-end tests (real bash/zsh in a PTY)
cargo build --release
cargo run --release --example bench -- --check         # performance budget
```

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how it is built
- [docs/ROADMAP.md](docs/ROADMAP.md) — phases P0 → P6

## License

Apache License 2.0 — see [LICENSE](LICENSE) and [NOTICE](NOTICE).
