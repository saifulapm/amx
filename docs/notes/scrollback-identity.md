# Scrollback identity: what libghostty-vt actually supports (T12 spike)

The M0 plan hands T12 two unverified risks before any implementation:

- **R4** — the C API has no row-id concept and nothing that distinguishes rows
  *committed* to history from rows *pruned* off the top, which `RowId`
  allocation and the eviction floor both depend on.
- **R5** — reading an arbitrary history range *without moving the live
  viewport* is unconfirmed; `GHOSTTY_SCROLL_VIEWPORT_ROW` addresses history
  rows but mutates a viewport the application and other clients can see.

Ground truth below is the vendored headers under `vendor/libghostty-vt/include`
and experiments run against the built library (`vendor/libghostty-vt` at
`1.3.2-HEAD-+c5a21edfc`). Every claim marked **measured** came out of a test;
the ones worth keeping live in `crates/amx-vt/tests/history.rs` and
`crates/amx-server/tests/history.rs`.

**Verdict: 04 §3's contract is achievable as written.** It needs one primitive
the plan did not consider — tracked grid references — plus a content check the
plan did not ask for, because the library will hand you a *live* anchor that
silently points at a different row.

---

## R5 — reading history without touching the viewport: settled, yes

`ghostty_terminal_grid_ref(terminal, point, &ref)` resolves a point tagged
`GHOSTTY_POINT_TAG_HISTORY` and never touches the viewport; the cell and row
data come off the returned reference. T06 already built `Terminal::read_row` on
it. **Measured:** with the viewport parked at row 3 of a 51-row scrollable area,
reading 40 history rows left `GHOSTTY_TERMINAL_DATA_SCROLLBAR` at
`{total: 51, offset: 3, len: 4}` and `VIEWPORT_ACTIVE` false — byte-identical
before and after. `GHOSTTY_SCROLL_VIEWPORT_ROW` is therefore never needed to
*read*, only to move the user's own scroll.

**Cost (measured, release build, 80×24 pane, 5502 rows of scrollback):**

| Operation | Cost |
|---|---|
| `read_row` near the top of history | 3.8 µs/row |
| `read_row` 1000 rows from the bottom | 3.3 µs/row |
| resolving a tracked anchor to a history offset | 14 ns |

So a full 5000-row scrollback transfer costs ~17 ms of parser-thread time. That
is fine for a bulk command served on the parser thread and chunked (04 §4), and
it is exactly why it must never touch the frame path — which matches the
header's own warning that grid references are "not meant to be used as the core
of a render loop".

**Trap, measured:** the `history` tag does *not* stop at the active area.
`Point::history(y)` with `y >= scrollback_rows` resolves into the *live* rows
and returns them happily (history(16) and active(5) returned the same row after
a height change). Only `y` past the whole screen errors. Range serving must
therefore bound reads by `scrollback_rows()` itself; the library will not do it
for you, and the failure mode is serving live rows as history.

## R4 — telling committed rows from pruned rows

### What the counters alone cannot do

`SCROLLBACK_ROWS` is a level, not a counter: one tick's change is
`committed − pruned`, one equation with two unknowns. It is exact only while
nothing has been pruned yet, which is what the provisional tracking in
`pane_host/parser.rs` said about itself.

There is also no fixed row capacity to lean on. **Measured:** with
`max_scrollback = 8192`, a 20-column pane settles at 4269 scrollback rows and a
200-column pane at 387. The value is a *memory* bound, not a line count, despite
`terminal.h:187` calling it "maximum number of lines to keep in scrollback
history" and amx-vt's `TerminalOptions::max_scrollback` repeating it. Row
capacity moves with width and content, and it is not even monotonic within one
pane (observed 3464 → 3367 rows across writes as pages recycled). Nothing may be
derived from it.

### The primitive that does work: tracked grid references

`ghostty_terminal_grid_ref_track` returns a handle the library keeps pointing at
its cell across "scrolling, scrollback pruning, resize/reflow, and other
terminal mutations" (`grid_ref.h:48`), and
`ghostty_tracked_grid_ref_point(ref, tag, &out)` reports where it ended up.
That is the missing second measurement:

> Anchor on a row whose id you know. If it is still in history at offset `y`,
> then **`oldest_row = anchor_id − y`** — the number of rows the scrollback has
> discarded, exactly.

`head = oldest_row + scrollback_rows` follows, and ids are dense over surviving
history, so serving a range is `history_offset = id − oldest_row`.

Re-anchor after every observation on the **newest** history row, which is the
last row a prune reaches, so the anchor only dies if the entire scrollback turns
over inside one tick.

**Measured** (`s1`, `s2`): an anchor placed on a row still in the active area
follows it into history and then keeps a stable history offset while nothing is
pruned; once pruning starts its offset counts down and `oldest_row = id − y`
agrees with the row contents; when the whole history turns over the anchor
reports `has_value() == false`.

### Why `has_value()` is not enough — the anchor lies

Two cases where the anchor is **alive and wrong**, both measured:

1. **Height grow.** Growing the row count pulls the newest history rows back
   into the active area (scrollback 17 → 11 for rows 4 → 10). The anchor
   correctly follows its row, but its `History` coordinate is then stale
   (reported 16 while history held 11 rows) because the history tag keeps
   counting into the live area. Guard: an anchor is only a floor probe when its
   `Active` coordinate is `None` **and** its history offset is `< scrollback_rows`.
2. **`CSI 3J` (erase scrollback).** History is discarded, `scrollback_rows`
   drops to 0 — and the anchor still reports `has_value() == true` with
   `History = 0`, `Active = 0`, while the row at that position is a *different
   row* than the one anchored ("line17" where "line16" was anchored). The
   library remapped a discarded pin instead of dropping it.

So identity has to be confirmed by content, not by the handle. The tracker keeps
the anchor row's content hash and re-reads it each tick; a mismatch is treated
exactly like a lost anchor.

### Width reflow

**Measured:** narrowing 20 → 10 columns took history from 37 to 57 rows and
rewrote row 0's content ("line0 padded out to twenty" → "line0 padd"); widening
to 40 took it back to 17 rows. An anchor survives with an offset that is
meaningless afterwards. Every id in history is invalidated, which is exactly the
`history.invalidated{from_row}` case 04 §3 describes, with
`from_row = oldest_row` — the *whole* cached range, because reflow rewrites the
oldest row too, not just the tail.

### Alternate screen

**Measured:** with the alternate screen up, `SCROLLBACK_ROWS` reads 0 and
`TOTAL_ROWS` is just the grid; output written there commits nothing; leaving the
alternate screen restores the primary scrollback and the anchor's offset
unchanged. Both data queries are per *active screen*, so history tracking must
freeze while the alternate screen is up or it will read a wipe that never
happened.

### RIS is not observable

An application-issued `ESC c` discards history like `CSI 3J` does, and there is
no callback or datum that reports it: the effect callbacks cover bell, title,
pwd, clipboard and queries only, and mode 2027 (which R7 would have made a
signal) survives RIS because of the vendored patch. A reset is therefore
reported as `InvalidationCause::Clear`, and `Reset` is reserved for a reset amx
performs itself. This is a naming loss, not a correctness one: the client
action — drop the cache from `from_row` — is identical.

## The model this settles on

State per pane: `oldest_row` (floor), `head` (next id to be committed), the
highest id ever *issued*, one anchor with its row id and content hash, the floor
row's content hash, and the last observed grid size.

Each observation, on the parser thread, after a frame:

1. Alternate screen → do nothing.
2. `S = scrollback_rows()`.
3. Derive the floor:
   - width changed since the last observation → **rebaseline**, cause
     `WidthReflow`;
   - else anchor alive, not in the active area, offset `< S`, and its content
     hash still matches → `floor = anchor_id − offset`;
   - else only the height changed and the floor row's hash still matches →
     floor unchanged (rows were pulled back into the live area; the head
     recedes, nothing is invalidated — 04 §3 says only width changes reflow);
   - else → **rebaseline**, cause `Clear`.
4. `head = floor + S`.
5. Emit `Evicted{oldest_row}` if the floor advanced, and `Committed{range,
   hashes}` for `[head_prev, head)` if the head advanced.
6. Re-anchor on history row `S−1` and re-read both hashes.

**Rebaseline** means: emit `Invalidated{from_row: oldest_row_prev, cause}` if
anything was ever issued, then set `floor` to the highest id ever issued so the
surviving rows get ids that were never handed out before. It is the *issued*
high-water mark and not the head, because the head falls back below it whenever
a taller grid reclaims committed rows — rebaselining from the head there would
hand an announced id to a different row. Ids are allocated in increasing order
and no id is ever reassigned to a different row without an announcement, across
trimming, clear, reset and reflow alike.

The one place an id is *deliberately* reissued is that same reclaim: rows a
taller grid pulls back into the live area keep their ids and commit again under
them. If the application overwrote them meanwhile, the content changed and the
id's announcement carries a different hash, which is precisely what 04 §3 puts
the hash there for. No invalidation fires, because a height change reflows
nothing.

**Validated end to end (measured):** 28,900 lines written across 40 ticks of
wildly different sizes through an 8 KiB scrollback (repeated pruning, two anchor
losses), then every surviving history row was compared against the line its
derived id claims. Zero mismatches.

### What the model gives up, deliberately

- **Ids are not a census of rows ever produced.** If a tick commits more rows
  than the whole scrollback holds, the rows that flowed through unobserved get
  no ids at all: the floor jumps to the old head and the survivors take fresh
  ids. Every invariant clients depend on still holds — monotonic, never reused,
  refused below the floor — and the loss is announced, in the spirit of the
  event bus's `gap`. **Measured:** a single `vt_write` can commit ~11.7 M
  rows/s, so a full turnover inside one 16 ms frame is reachable under a
  `cat /dev/urandom`-class flood; the path is real, not theoretical.
- **Hashes are capped per tick.** Hashing a committed row costs a `read_row`
  (3.3 µs), so a flood cannot hash every row it commits. Rows whose ids were
  never issued before cannot be stale in any client's cache, so the cap only
  ever drops hashes for rows no client can hold; the re-commit window after a
  height change is bounded by the grid height and is always hashed.
- **History is served as text.** The packed cell layout the grid stream will use
  (`amx-proto`'s `Cells`) is not written yet, so history rows carry their
  characters and no styling. Scrollback renders unstyled in the client until
  that layout exists; the packing is one function in
  `amx-server/src/history/pack.rs`.

## Consequences for the docs

Nothing in 04 §3 needs to change. Two smaller corrections belong in a doc PR:

- The M0 plan's R4 and R5 can be closed: R5 is answered by
  `ghostty_terminal_grid_ref` with a `History` point, R4 by tracked grid
  references plus a content check.
- `TerminalOptions::max_scrollback` is documented in amx-vt as "how many lines
  of scrollback to keep", copied from `terminal.h:187`. It is a byte bound. The
  amx-vt doc is corrected in this change; the header is upstream's to fix.
