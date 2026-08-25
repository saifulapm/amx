# Theming the view

A theme answers six questions and nothing else. This file is the whole of the
contract: what the six mean, what a value may be, where a name is looked up,
what happens when a file is wrong, and what is deliberately out of a theme's
reach. The code it describes is `src/theme.rs` and the two files under
`assets/themes/`.

## The six roles

A role is named by what it means, because that is how the view asks for a
colour: a row is painted for having failed, not for being red.

| role | painted on |
| ---- | ---------- |
| `waiting` | anything waiting on a person: the waiting rows and their group, the unread mark, the card's question and its answer prompt, the composer's confirm line |
| `done` | what went as intended: finished rows, merged and ready pull requests |
| `failed` | what was attempted and failed: failed rows, failing checks, the failure notice |
| `stopped` | what was ended by hand and is over: stopped rows, closed requests, the completed group's count |
| `accent` | the agent that does not exist yet: the dials' values in the header, the permission line under the composer |
| `cursor` | the line the cursor is on — a background, so it says where the cursor is without taking a colour away from what the line was saying |

Most of the screen wears none of them. What a row is, where a group begins and
what it holds are said in words, in bold and in the dim the terminal already
renders, which is why a wall of forty rows has two or three colours on it and
they are the two or three worth looking at.

## Values

A value is a colour the way a terminal names one, in any of three spellings:

- a name — `cyan`, `bright black`, the sixteen the terminal itself defines.
  A named colour is the terminal's to draw, so it follows the terminal's own
  theme. This is a feature, not a fallback: `accent` ships as `cyan` because
  what colour claude paints its own permission row is not something amx has
  measured, and an RGB nobody measured would read as one that was.
- a 256-colour index — `134`.
- a hex triple — `#4eba65`.

## Resolution

`theme` in `~/.config/amx/config.toml` is a name, and a name is looked up in
this order:

1. A name amx ships — `default`, `terminal` — is answered out of the binary.
2. A name with a `/` in it is a path, taken exactly as written. A theme kept
   in a repository, or synced between machines, is reached this way.
3. Anything else is a file of that name in `~/.config/amx/themes/`, with
   `.toml` added if it was left off: `mine` and `mine.toml` are the same wish.

## Failure

A theme is a convenience under the same law as the config: losing the view to
a stray comma is the worse outcome.

- A role left out of the file keeps the default palette's answer.
- An unknown key is a warning, and the rest of the file still applies.
- A file that cannot be read, or cannot be parsed, degrades to the default
  palette whole, with a warning saying why.

## Live reload

While the view is open, the active theme's file is stat'ed once a second,
beside the reading the view is already taking on that clock. When the mtime
moves, the file is read again and the next pass paints in it. An edit is seen
on the next pass rather than the instant it lands, which is the cadence
everything else on that screen moves at — and what it buys is no watcher
thread, no descriptor held open and nothing to unwind when the screen is
handed back.

A shipped theme lives in the binary, so there is nothing to watch; switching
`theme` in the config is read on the same clock.

## The two that ship

`default` is measured off claude's own palette — a view sitting beside
claude's panes should not be a different shade of the same idea. `terminal`
names no colour of its own: every value is a named colour, so the view wears
whatever the terminal does, light or dark.

## What a theme cannot touch, on purpose

- **An agent's own paint.** The card and `amx logs` replay what a pane drew,
  in the colours it drew them. Theming that would be repainting the agent's
  output.
- **The glyphs.** `✻`, `●`, `✗` and the rest are the view's vocabulary, not
  its palette.
- **amx's stderr.** Errors and warnings from the verbs land among git's and
  cargo's output and follow the terminal, not the view.
- **Dim and bold.** Weight is meaning in the view — an unselected row is dim,
  a selected name is bold — and a theme that could remove meaning is a theme
  that can lie.
