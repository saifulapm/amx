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
            # Read and a sleep are pre-approved so a tool call can run without a
            # dialog (the Esc-during-a-tool-call scenario needs one).
            "allow": ["Read", "Bash(sleep:*)"],
            # An explicit ask rule is the only *deterministic* way to raise the
            # dialog: measured on 2.1.224, an unremarkable `echo` in the default
            # permission mode is approved without one.
            "ask": ["Bash(echo:*)", "Write"],
            "deny": [],
        },
    }


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
