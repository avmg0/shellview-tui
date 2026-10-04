"""Shell discovery and launch with ShellView integration (bash + zsh)."""
from __future__ import annotations

import os
import shutil
from dataclasses import dataclass, field
from pathlib import Path

INTEGRATION = Path(__file__).parent / "integration"
SUPPORTED = ("zsh", "bash")


@dataclass
class ShellLaunch:
    path: str
    name: str
    argv: list[str]
    env: dict[str, str] = field(default_factory=dict)
    integrated: bool = False


def resolve_shell(choice: str | None = None) -> str:
    """Pick a shell: explicit choice (name or path) > $SHELL > zsh > bash > sh."""
    candidates = [choice] if choice else []
    candidates += [os.environ.get("SHELL"), "zsh", "bash", "sh"]
    for c in candidates:
        if not c:
            continue
        if os.path.sep in c:
            if os.access(c, os.X_OK):
                return c
            continue
        found = shutil.which(c)
        if found:
            return found
    raise RuntimeError(f"No usable shell found (asked for {choice!r})")


def available_shells() -> list[str]:
    return [s for s in SUPPORTED if shutil.which(s)]


def build_launch(choice: str | None = None, integrate: bool = True) -> ShellLaunch:
    path = resolve_shell(choice)
    name = os.path.basename(path)
    if integrate and name == "zsh":
        return ShellLaunch(
            path, name, [path, "-l"],
            env={
                "ZDOTDIR": str(INTEGRATION / "zsh"),
                "SHELLVIEW_ORIG_ZDOTDIR": os.environ.get("ZDOTDIR", ""),
            },
            integrated=True,
        )
    if integrate and name == "bash":
        # --rcfile is ignored by login shells, so we emulate login startup inside the rcfile.
        return ShellLaunch(
            path, name, [path, "--rcfile", str(INTEGRATION / "bash" / "shellview.bash"), "-i"],
            env={"SHELLVIEW_LOGIN": "1"},
            integrated=True,
        )
    return ShellLaunch(path, name, [path, "-l"])
