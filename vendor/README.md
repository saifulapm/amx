# vendor/

One vendored dependency: the libghostty-vt source dist that `amx-vt` compiles
and links (`docs/06-m0-plan.md` D-M0-1). The PTY layer is written directly on
`rustix`, so there is no second vendored tree (D-M0-3).

```
libghostty-vt/              the extracted source dist, patches applied
libghostty-vt.vendor.json   source commit, dist archive, extracted dir, zig pin
libghostty-vt.patches.md    one section per local patch (see it before editing)
patches/libghostty-vt/      the patch files themselves
toolchain/                  downloaded Zig, gitignored
```

Re-vendoring — only when the pinned commit moves:

```sh
git clone https://github.com/ghostty-org/ghostty
git -C ghostty checkout <pinned commit>
scripts/vendor-libghostty-vt.sh sync --source-repo ghostty
```

## Zig 0.15.2 is exact, not a floor

`build.zig.zon` declares `minimum_zig_version = "0.15.2"` and
`src/build/zig.zig` turns anything else into a `@compileError`. Zig 0.16.0 was
tried against this tree and fails twice: the version check, and
`build.zig:27`'s call to `std.Io.Dir.readFileAlloc`, whose signature changed
after 0.15.2. There is no "newer is fine" path here.

`scripts/vendor-libghostty-vt.sh toolchain` downloads and checksums the pinned
compiler into `vendor/toolchain/`, which is the first place `build.rs` looks —
before `PATH`, so a system Zig of the wrong version does not break the build.
`AMX_ZIG` overrides the search; `AMX_LIBGHOSTTY_VT_LIB_DIR` skips Zig entirely
and links a library you supply.

## The first build downloads ~347 MB of Zig packages

`zig build` resolves the **whole declared dependency graph** before it compiles
anything, regardless of laziness. Measured on this tree:

- `zig build --fetch` (mode `needed`) resolves exactly one package, `uucode`,
  5.4 MB — that is all the lib-vt build graph actually uses.
- a plain `zig build` with only `uucode` in the package cache still fetches 29
  packages, 347 MB: every URL dependency in `build.zig.zon` plus the URL
  dependencies of the in-tree `pkg/*` packages (imgui, freetype, harfbuzz,
  glslang, the fonts, …). Offline it fails on all of them.

So committing "the resolved set" does not buy an offline build, and committing
what an offline build actually demands means committing 347 MB into a repo
whose entire vendored tree is 19 MB. Two escapes were tried and rejected:

- `zig build --system <dir>` refuses to start unless *every* declared lazy
  package is present, and empty placeholders fail at comptime
  (`lazyImport` needs a real `build.zig`). It also flips on every system
  integration, which would link the archive against system simdutf/highway.
- pruning the unused dependencies out of `build.zig.zon` works up to a point,
  but `build.zig` calls `b.lazyDependency` on names it never uses (`zlib` via
  `GhosttyFrameData`), and an undeclared name is a hard panic — the prune would
  have to reach into `build.zig` and sixteen `pkg/*/build.zig.zon` files and be
  re-authored on every re-vendor.

The fetch is therefore documented rather than vendored, and CI caches Zig's
global package cache (`~/.cache/zig`) keyed on `build.zig.zon`, which is what
herdr does too. Packagers who already have the whole dependency set can pass it
with `AMX_LIBGHOSTTY_VT_ZIG_SYSTEM_DIR`.

One gotcha if you seed a package cache by hand: the entries under `p/` must be
real directories. Symlinking them makes zig resolve a build step's relative
paths through the link target and the build fails with `failed to spawn … :
FileNotFound`.
