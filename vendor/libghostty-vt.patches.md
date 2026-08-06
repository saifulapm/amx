# libghostty-vt local patches

Every file in `vendor/libghostty-vt/` that differs from the source dist is
listed here, one section per patch. `scripts/vendor-libghostty-vt.sh sync`
re-applies each patch in `vendor/patches/libghostty-vt/` after extracting a
fresh dist and refuses to finish if one does not apply, so this file and the
tree cannot drift apart silently.

Drop a patch when the vendored source commit carries the upstream behavior and
its verification still passes without it.

## 0001 grapheme cluster as the default mode

status: active

patch: `vendor/patches/libghostty-vt/0001-grapheme-cluster-default-mode.patch`

upstream discussion: none opened; the C API exposes current-mode mutation
(`ghostty_terminal_mode_set`) but `GhosttyTerminalOptions` has no field for a
terminal's *default* modes, which is what a reset restores

upstream pr: none opened

vendored base: `c5a21edfcbc2d5b46540ad91b7980aca31f5f1f3`

local files:

- `vendor/libghostty-vt/src/terminal/c/terminal.zig`

reason: amx renders server-authoritative cells directly (04 §3), so a
multi-codepoint grapheme cluster — a flag, a ZWJ sequence, an emoji with a
skin-tone modifier — has to live in one cell. That is DEC private mode 2027,
and it is off by default. Measured against the unpatched dist:

| step | mode 2027 | `\u{1F1FA}\u{1F1F8}` |
|---|---|---|
| new terminal | false | two cells |
| after `ghostty_terminal_mode_set(2027, true)` | true | one cell |
| after DECSTR (`ESC [ ! p`) | true | — |
| after RIS (`ESC c`) through `vt_write` | false | — |
| after `ghostty_terminal_reset` | false | — |
| RIS **and** the flag in one `vt_write` batch | false | two cells |

The last row is why setting the mode at creation and re-applying it when a
reset is observed is not enough: a reset is only observable *after* the write
returns, and everything the application printed after its own `ESC c` in that
same batch has already been stored unclustered. Setting `default_modes`
instead makes 2027 both the initial and the post-reset value.

remove when: `GhosttyTerminalOptions` (or another C entry point) can set
default modes, or upstream makes clustering the lib-vt default — then delete
the patch and confirm `grapheme_cluster_mode_is_default_and_survives_full_reset`
still passes.

verification:

```sh
cargo test -p amx-vt --test sys grapheme_cluster
```
