"""Builds the scratch project the spike drives the agent inside.

The project is disposable and fully described here: a settings file that
subscribes every lifecycle event to one logging command, a couple of files for
the agent to touch, and permissions arranged so that some tools run silently
and some are guaranteed to raise a permission dialog (the dialog is a
measurement subject, not an obstacle).
"""

from __future__ import annotations

import json
import os
import shutil

# The lifecycle events fusion could plausibly consume. Names are the ones
# Claude Code 2.1.224 accepts; the full 31-name list it validates against is
# recorded in docs/notes/hook-coverage.md.
LIFECYCLE_EVENTS = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "PreCompact",
    "PostCompact",
]

# The rest of what 2.1.224 validates. Subscribed only by the inventory run:
# some of these fire per rendered message, and the fork storm would skew the
# latency numbers the matrix is trying to measure.
OTHER_EVENTS = [
    "Setup",
    "TeammateIdle",
    "TaskCreated",
    "TaskCompleted",
    "Elicitation",
    "ElicitationResult",
    "ConfigChange",
    "WorktreeCreate",
    "WorktreeRemove",
    "InstructionsLoaded",
    "CwdChanged",
    "FileChanged",
    "DirectoryAdded",
    "MessageDisplay",
]

# Events whose entry takes a tool-name matcher.
TOOL_SCOPED = {
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PermissionRequest",
    "PermissionDenied",
}

# A second subscription on a few events, to measure the spread between two
# hook processes dispatched for the same event (hooks run in parallel).
DOUBLED = {"UserPromptSubmit", "PreToolUse", "Stop"}

CLAUDE_MD = """\
# Scratch project

Answer in as few words as possible. Never explain what you are about to do.
Do not read or write files unless the instruction says to.
"""

NOTES = """\
alpha
bravo
charlie
delta
"""


def settings(hook_cmd: str, log_path: str, events: list[str]) -> dict:
    hooks: dict[str, list] = {}
    for event in events:
        entries = [{"type": "command", "command": f"{hook_cmd} {event} {log_path}", "timeout": 10}]
        if event in DOUBLED:
            entries.append(
                {"type": "command", "command": f"{hook_cmd} {event}#b {log_path}", "timeout": 10}
            )
        entry: dict = {"hooks": entries}
        if event in TOOL_SCOPED:
            entry["matcher"] = "*"
        hooks[event] = [entry]
    return {
        "hooks": hooks,
        "permissions": {
            # Read and Bash are pre-approved so a tool call can run without a
            # dialog (the Esc-during-a-tool-call scenario needs a long one).
            # A bare `sleep` is refused by the tool's own policy on 2.1.224, so
            # the long call is a wait loop, which that policy endorses.
            "allow": ["Read", "Bash"],
            # An explicit ask rule is the only *deterministic* way to raise the
            # dialog: measured on 2.1.224, an unremarkable `echo` in the default
            # permission mode is approved without one.
            "ask": ["Bash(echo:*)", "Write"],
            # A rule-denied call, to find out whether PermissionDenied fires for
            # anything at all once the human "no" has been measured silent.
            "deny": ["Bash(whoami:*)"],
        },
    }


# Codex 0.147.0's hook events, as its own TUI lists them. Same PascalCase names
# as Claude Code, configured in $CODEX_HOME/hooks.json rather than settings.json.
CODEX_EVENTS = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
]


def build_codex_home(home: str, hook_cmd: str, log_path: str, borrow_auth: str = None) -> str:
    """A throwaway CODEX_HOME whose hooks.json subscribes every event.

    Credentials are *borrowed by symlink* from the real Codex home rather than
    copied: the spike needs an authenticated agent, not a second copy of a
    token, and nothing it writes touches the user's own configuration.
    """
    if os.path.exists(home):
        shutil.rmtree(home)
    os.makedirs(home)
    cfg = {
        "hooks": {
            e: [{"hooks": [{"type": "command", "command": f"{hook_cmd} {e} {log_path}"}]}]
            for e in CODEX_EVENTS
        }
    }
    with open(os.path.join(home, "hooks.json"), "w") as f:
        json.dump(cfg, f, indent=2)
    auth = borrow_auth or os.path.expanduser("~/.codex/auth.json")
    if os.path.exists(auth):
        os.symlink(auth, os.path.join(home, "auth.json"))
    open(os.path.join(home, "config.toml"), "w").close()
    return home


def build(root: str, hook_cmd: str, log_path: str, all_events: bool = False) -> str:
    """(Re)create the scratch project at `root`. Returns the path."""
    if os.path.exists(root):
        shutil.rmtree(root)
    os.makedirs(os.path.join(root, ".claude"))
    events = LIFECYCLE_EVENTS + (OTHER_EVENTS if all_events else [])
    with open(os.path.join(root, ".claude", "settings.json"), "w") as f:
        json.dump(settings(hook_cmd, log_path, events), f, indent=2)
    with open(os.path.join(root, "CLAUDE.md"), "w") as f:
        f.write(CLAUDE_MD)
    with open(os.path.join(root, "notes.txt"), "w") as f:
        f.write(NOTES)
    return root
