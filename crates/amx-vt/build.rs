//! Build the vendored libghostty-vt and generate its FFI bindings.
//!
//! Two things happen here, both per `docs/06-m0-plan.md` D-M0-1 and D-M0-2:
//! the vendored Zig source dist under `vendor/libghostty-vt` is compiled into
//! a static library and linked, and `bindgen` runs over the vendored headers
//! into `OUT_DIR`. The generated bindings are never committed, so a re-vendor
//! cannot leave stale declarations behind.
//!
//! Escape hatches, in the order they are consulted:
//!
//! - `AMX_LIBGHOSTTY_VT_LIB_DIR` — link `libghostty-vt` from that directory
//!   and skip Zig entirely. This is the distro-packager path.
//! - `AMX_ZIG` — use this compiler instead of searching.
//! - `AMX_LIBGHOSTTY_VT_ZIG_SYSTEM_DIR` — passed to `zig build --system`, for
//!   package managers that supply the whole Zig dependency set themselves.
//! - `AMX_LIBGHOSTTY_VT_OPTIMIZE` / `AMX_LIBGHOSTTY_VT_SIMD` — build knobs;
//!   the defaults (`ReleaseFast`, on) are what we ship.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "build/toolchain.rs"]
mod toolchain;

fn main() {
    let manifest_dir = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    // The vendored tree lives at the workspace root, not in the crate: it is
    // shared with the vendoring script and the patch manifest next to it.
    let vendor_dir = manifest_dir
        .join("..")
        .join("..")
        .join("vendor")
        .canonicalize()
        .unwrap_or_else(|err| fail(&format!("cannot resolve the vendor directory: {err}")));
    let source_dir = vendor_dir.join("libghostty-vt");
    let include_dir = source_dir.join("include");

    rerun_hints(&manifest_dir, &source_dir);

    match env::var_os("AMX_LIBGHOSTTY_VT_LIB_DIR") {
        Some(dir) => link(Path::new(&dir)),
        None => link(&build_vendored(&vendor_dir, &source_dir)),
    }

    generate_bindings(&include_dir);
}

fn var(name: &str) -> String {
    env::var(name).unwrap_or_else(|err| fail(&format!("{name}: {err}")))
}

/// Report a build failure the way a contributor can act on.
///
/// A panic in a build script prints as `error: failed to run custom build
/// command`, with the message buried; `cargo:warning` puts it on screen first.
fn fail(message: &str) -> ! {
    for line in message.lines() {
        println!("cargo:warning={line}");
    }
    panic!("{message}");
}

fn rerun_hints(manifest_dir: &Path, source_dir: &Path) {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("build/toolchain.rs").display()
    );
    for entry in [
        "build.zig",
        "build.zig.zon",
        "include",
        "pkg",
        "src",
        "VERSION",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            source_dir.join(entry).display()
        );
    }
    for name in [
        "AMX_LIBGHOSTTY_VT_LIB_DIR",
        "AMX_LIBGHOSTTY_VT_OPTIMIZE",
        "AMX_LIBGHOSTTY_VT_SIMD",
        "AMX_LIBGHOSTTY_VT_ZIG_SYSTEM_DIR",
        "AMX_ZIG",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
}

/// Compile the vendored dist and return the directory holding the archive.
fn build_vendored(vendor_dir: &Path, source_dir: &Path) -> PathBuf {
    let zig = toolchain::find_zig(vendor_dir, env::var_os("AMX_ZIG").map(PathBuf::from))
        .unwrap_or_else(|message| fail(&message));
    let zig = zig.as_path();

    let target = var("TARGET");
    let zig_target = toolchain::zig_target(&target).unwrap_or_else(|message| fail(&message));
    // The dist stamps its own version; passing it back keeps the built library
    // and the vendor manifest telling the same story.
    let version = fs::read_to_string(source_dir.join("VERSION"))
        .unwrap_or_else(|err| fail(&format!("cannot read the vendored VERSION: {err}")))
        .trim()
        .to_string();
    let optimize =
        env::var("AMX_LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".to_string());
    let simd = env::var("AMX_LIBGHOSTTY_VT_SIMD").unwrap_or_else(|_| "true".to_string());

    // Each target gets its own prefix: a cross-check (say, apple-darwin from
    // Linux) must never clobber the native static library under the default
    // shared zig-out, or the next native link pulls foreign objects.
    let prefix = source_dir.join("zig-out").join(&target);

    let mut command = Command::new(zig);
    command
        .current_dir(source_dir)
        .arg("build")
        .arg("--prefix")
        .arg(&prefix)
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dsimd={simd}"))
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={version}"))
        .arg("-Demit-xcframework=false");
    if let Some(system_dir) = env::var_os("AMX_LIBGHOSTTY_VT_ZIG_SYSTEM_DIR") {
        command.arg("--system").arg(system_dir);
    }

    let status = command
        .status()
        .unwrap_or_else(|err| fail(&format!("cannot run `{} build`: {err}", zig.display())));
    if !status.success() {
        fail(&format!(
            "`zig build` for the vendored libghostty-vt failed ({status}).\n\
             The first build resolves the Zig dependency set over the network; \
             vendor/README.md records why that set is not committed."
        ));
    }

    prefix.join("lib")
}

fn link(lib_dir: &Path) {
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    // The archive is Zig-compiled C and Zig objects; both need the C runtime
    // and libm at final link time.
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=dylib=c");
    } else {
        println!("cargo:rustc-link-lib=dylib=m");
    }
}

/// Generate `sys` bindings into `OUT_DIR` (D-M0-2: never committed).
fn generate_bindings(include_dir: &Path) {
    let header = include_dir.join("ghostty").join("vt.h");
    let out = PathBuf::from(var("OUT_DIR")).join("bindings.rs");

    let bindings = match run_bindgen(&header, include_dir, None) {
        Ok(bindings) => bindings,
        Err(first) => {
            // Some distributions install libclang without the builtin headers
            // beside it — Arch puts them under /usr/lib/llvm*/lib/clang/*/
            // while libclang.so sits in /usr/lib — and clang then cannot find
            // stddef.h. Locate them once and retry rather than making every
            // contributor discover BINDGEN_EXTRA_CLANG_ARGS.
            let Some(resource_dir) = clang_resource_dir() else {
                fail(&bindgen_failure(&header, &first));
            };
            run_bindgen(&header, include_dir, Some(&resource_dir))
                .unwrap_or_else(|err| fail(&bindgen_failure(&header, &err)))
        }
    };

    bindings
        .write_to_file(&out)
        .unwrap_or_else(|err| fail(&format!("cannot write {}: {err}", out.display())));
}

fn bindgen_failure(header: &Path, err: &bindgen::BindgenError) -> String {
    format!(
        "bindgen failed on {}: {err}\n\
         bindgen needs libclang and its builtin headers; install clang, or set \
         BINDGEN_EXTRA_CLANG_ARGS=-resource-dir=$(clang -print-resource-dir).",
        header.display()
    )
}

fn run_bindgen(
    header: &Path,
    include_dir: &Path,
    resource_dir: Option<&Path>,
) -> Result<bindgen::Bindings, bindgen::BindgenError> {
    let mut builder = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.display()))
        // Cross-compilation: libclang defaults to the host, which drags the
        // host's libc headers into a foreign-target parse (glibc's stubs.h
        // breaks an apple-darwin check on Linux). The vendored headers only
        // need <stdbool.h>/<stddef.h>/<stdint.h>, all provided by clang's
        // builtins, so a freestanding parse needs no target sysroot at all.
        .clang_arg(format!("--target={}", var("TARGET")))
        .clang_arg("-ffreestanding")
        // The wrapper maps every constant through an exhaustive `match`, so
        // the constants must arrive as plain values, not as a Rust enum whose
        // variants would silently absorb a renumbering.
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .derive_debug(true)
        .derive_default(true)
        .prepend_enum_name(false)
        .layout_tests(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*");
    if let Some(resource_dir) = resource_dir {
        builder = builder.clang_arg(format!("-resource-dir={}", resource_dir.display()));
    }
    // The surfaces T06 wraps. Anything a wrapped function references is pulled
    // in transitively; this list is what the crate is allowed to name.
    for surface in [
        "ghostty_terminal_.*",
        "ghostty_render_state_.*",
        "ghostty_row_.*",
        "ghostty_cell_.*",
        "ghostty_key_.*",
        "ghostty_osc_.*",
        "ghostty_grid_ref.*",
        "ghostty_tracked_grid_ref_.*",
        "ghostty_result_.*",
        "ghostty_style_.*",
    ] {
        builder = builder.allowlist_function(surface);
    }
    builder.generate()
}

/// Find a clang resource directory whose `include/stddef.h` exists.
///
/// `clang -print-resource-dir` when the driver is installed, otherwise the
/// `clang/<version>` directories llvm installs alongside its libraries.
fn clang_resource_dir() -> Option<PathBuf> {
    if let Ok(output) = Command::new("clang").arg("-print-resource-dir").output()
        && output.status.success()
        && let Ok(path) = String::from_utf8(output.stdout)
    {
        let dir = PathBuf::from(path.trim());
        if dir.join("include").join("stddef.h").is_file() {
            return Some(dir);
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = env::var_os("LIBCLANG_PATH") {
        roots.push(PathBuf::from(dir));
    }
    roots.extend(["/usr/lib", "/usr/lib64", "/usr/local/lib"].map(PathBuf::from));

    let mut best: Option<PathBuf> = None;
    for root in roots {
        // Both layouts: <root>/clang/<version> and <root>/llvm*/lib/clang/<version>.
        let mut bases = vec![root.join("clang")];
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("llvm") {
                    bases.push(entry.path().join("lib").join("clang"));
                }
            }
        }
        for base in bases {
            let Ok(entries) = fs::read_dir(&base) else {
                continue;
            };
            for entry in entries.flatten() {
                let candidate = entry.path();
                if candidate.join("include").join("stddef.h").is_file()
                    && best.as_ref().is_none_or(|current| *current < candidate)
                {
                    // Directories are named by major version, so the greatest
                    // path is the newest clang installed.
                    best = Some(candidate);
                }
            }
        }
    }
    best
}
