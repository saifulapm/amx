# Worktrees and branches

Inside a git repository, each agent gets a worktree of its own, so several
agents can change one repository at once without touching your checkout or
each other.

```
~/code/app                         your checkout, untouched
~/code/app/.amx/worktrees/fix-login-a1b      branch amx/fix-login-a1b
~/code/app/.amx/worktrees/port-import-b2c    branch amx/port-import-b2c
```

`amx new` cuts the tree at `<repo>/.amx/worktrees/<id>` on a new branch
`amx/<id>`, from the commit you have checked out, and records that commit.
`.amx/` is added to `.git/info/exclude`, so it never shows in `git status` or
in your commits.

An agent runs in the directory as it is, with no worktree, when:

- the directory is not in a git repository,
- you pass `--no-worktree` (or `w:off` on the view's task line),
- the config says `worktrees = false`,
- it is a `--exec` command row,
- it is a subagent started with `amx sub` inside an agent's pane, without
  `--worktree`.

## Where the tree starts

| Flag | Task line | The tree |
| --- | --- | --- |
| (none) | | New branch `amx/<id>` from what is checked out, or from the `base` config key. |
| `--base <ref>` | `b:<ref>` | New branch `amx/<id>` from any branch, tag or commit. |
| `--branch <name>` | `on:<name>` | An existing branch. The agent commits onto it. |
| `--pr <n>` | `pr:<n>` | A pull request's branch, at its current head. |
| `--with-changes` | `w:changes` | New branch, plus your uncommitted work moved into it. |

```sh
amx new --base main "add the export"
amx new --branch spike "carry on with this"
amx new --pr 128 "review this and fix the nits"
amx new --with-changes "finish what I started"
```

Combinations that contradict each other are refused before anything is made:

| Flag | Refused with |
| --- | --- |
| `--branch` | `--base`, `--pr`, `--no-worktree`, `--exec` |
| `--pr` | `--base`, `--with-changes`, `--no-worktree`, `--exec` |
| `--with-changes` | `--no-worktree`, `--exec` |

`--branch` and `--pr` always make a worktree, even with `worktrees = false`.

### An existing branch

`--branch` uses a local branch as it stands, unpushed commits included. A
branch that only exists on origin is fetched first. A leading `origin/` is
dropped, so `--branch origin/spike` works. git allows a branch in one worktree
at a time, so a branch already checked out elsewhere is refused.

### A pull request

`--pr` asks `gh` for the request's head, fetches it from origin, and checks it
out under the head branch's name. If the request comes from a fork, or that
name is already checked out here, the branch is called `pr-<n>` instead. amx
never moves an existing local branch: if `pr-<n>` already has commits the
request lacks, the spawn is refused. This needs `gh` and works with GitHub
only.

### Moving your work

`--with-changes` moves everything git sees as changed (staged, unstaged and
new files not matched by `.gitignore`) into the new tree, and resets your
directory to its last commit. Ignored files, such as build output, stay where
they are. With nothing to move, no agent starts. The whole repository's
changes move, even when `--dir` points at a subdirectory.

## Setting up a fresh tree

A new worktree is a clean checkout: no `.env`, no `node_modules`. Three config
keys prepare it before the agent starts:

```toml
copy = [".env"]              # files copied from the repository root
link = ["node_modules"]      # directories symlinked to the repository's own
setup = ["pnpm install"]     # commands run in the tree, in order
```

Paths are relative to the repository root, with no globs. A path that does not
exist is skipped with a warning. If a `setup` command fails, the spawn is
refused with its stderr as the reason and the tree is removed. Setup commands
get `AMX_ID`, `AMX_WORKTREE`, `AMX_REPO` and `AMX_AGENT_DIR` in their
environment.

## Folder-trust screens

claude and pi ask "do you trust this folder?" the first time they run in a new
directory, and every worktree is a new directory. An agent stopped on that
screen shows as waiting. Answer it from the view, or pick the trust row by
number with `amx answer <id> <n>`; `amx status <id>` lists the choices. On
current claude the first row is `No, exit`, so read before you answer.

Set `trust = true` to have amx answer it for the worktrees it cuts:

- claude: amx writes an entry for the tree into claude's own trust store,
  `~/.claude.json`, under claude's lock, and removes it when the tree goes.
- pi: amx passes `--approve` and writes nothing.

codex keys trust on the main checkout, so a worktree of a repository you
already trust is trusted. opencode has no such screen.

`amx --dir <path> doctor` tells you whether an agent started there would meet
the screen.

## Diff

```sh
amx diff <id>             # the full patch
amx diff <id> --stat      # one line per file, and totals
amx diff <id> --from main # measured from a ref you choose
```

`diff` shows everything the agent changed, committed or not, measured from
where it started:

| Agent | Measured from |
| --- | --- |
| In a tree amx cut | The commit the tree was cut from. After a rebase, the point where the branch and that commit parted. |
| Run in a directory as it is | The commit the directory was on when the agent started. |
| Adopted, or no recorded start | Where the branch left the main branch. |
| Any, with `--from <ref>` | That ref. |

Set `diff` in the config to read patches in your own viewer at a terminal:

```toml
diff = "delta --paging=always"
```

amx runs it in the agent's tree with the patch on stdin. Down a pipe, or with
`--stat`, you always get git's own output. In the view, `d` shows the patch on
the card and `alt+d` opens the viewer.

## The pull request column

When an agent's branch has a pull request, its row shows the number:

```
  ∙ fix-login-a1b     #12  the login bug is fixed                             4m
  ✻ port-import-b2c   #40  Running Bash                                       4s
```

| Colour | Standing |
| --- | --- |
| green | merged, or approved with nothing failing |
| red | a check failed |
| amber | changes requested |
| grey | closed without merging |
| dim | draft |
| your terminal's colour | open and waiting, or checks still running |

The card spells out the standing and lists every request on the branch.
`/#12` in the view finds the row, and `o` opens the request in your browser.

amx asks `gh`, then `glab`. Without either, the column is not drawn. Answers
are cached beside the agent's record for a minute, or for good once the branch
is merged or closed, and refreshed in the background so the view never waits
on the network.

## Stopping an agent

```sh
amx stop <id>                                  # asks
amx stop <id> --force                          # takes the defaults
amx stop <id> --worktree keep --branch delete  # answers up front
amx stop <id> --delete                         # also removes the record
```

`stop` ends the agent's process (it asks first, waits five seconds, then
kills), then deals with what it leaves:

| Thing | Default | Always kept when |
| --- | --- | --- |
| Worktree | delete | It holds uncommitted work. |
| Branch | keep | It has commits on no other branch or remote. |
| Record | keep | The worktree is still on disk, even with `--delete`. |

The defaults lose nothing: committed work stays on the branch. A branch cannot
be deleted while a worktree still has it checked out.

When a tree goes, amx also removes its entry from claude's trust store. It
never touches the entry for your own checkout.

`--force` answers every question with its default; `--delete` decides the
record. They are separate so you can clear a record without also giving up the
questions.

## Sweep

`amx sweep` clears agents whose work has landed. An agent qualifies when it is
finished or idle, has a branch, and one of these holds:

- its pull request merged or closed,
- git sees the branch as merged into the main branch,
- the branch is gone from origin (what a squash merge on the forge leaves).

```
$ amx sweep
fix-login-a1b  #12 merged
port-import-b2c  amx/port-import-b2c merged into main
add-search-c3d  amx/add-search-c3d gone from origin
sweep 3? [y/N]
```

One answer covers the list. `--force` skips the question. Each agent then goes
as `amx stop --force --delete` would take it, branch included.

Before it looks, `sweep` runs `git fetch --prune` once per repository and asks
the forge directly about any request not checked in the last minute, so it
works without the view ever having been open.

A branch is only deleted when nothing on it would be lost: every commit is on
another branch or remote, or the tip is exactly the commit a merged request
went in at. So a squash-merged request's branch goes, but a branch with commits
added after the merge is kept (`kept amx/fix-login-a1b: 2 commits are on no
other branch`). A branch that is only gone from origin, with no merged request
to vouch for it, is kept. `sweep` and `clear` only delete branches amx named
(`amx/<id>` or `pr-<n>`), never one you passed with `--branch`.

A worktree with uncommitted work is kept, and its record with it, whatever the
flags say.

## Clear

`amx clear` forgets every finished agent (done, failed or stopped), whether
its work landed or not.

```
$ amx clear
fix-login-a1b  #12 merged
add-search-b2c  stopped
port-import-c3d  done
clear 3? [y/N]
```

Agents whose work landed go as `sweep` takes them, branch and all. The rest
lose their record and worktree and keep their branch. Idle agents are not on
the list: they still have a session you can send a turn to.

In the view, `c` does the same for the whole wall, and `ctrl+x` twice does it
for one row. The view refreshes the upstream state with a background
`git fetch --prune` every five minutes per repository while it is open.

## Parking idle agents

A vendor idling at its prompt still holds its memory. After `park_after`
seconds (default 3600) with nobody attached, amx closes an idle agent's pane
and keeps everything else: record, transcript, worktree, last answer. Its row
glyph changes from `✻` to `∙`.

`enter` in the view, `amx attach` or `amx resume` starts it again on the same
session. A pinned agent (`ctrl+t`) or one you are attached to is never
parked. `park_after = 0` turns parking off.
