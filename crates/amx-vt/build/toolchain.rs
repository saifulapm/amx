//! Pure helpers shared by `build.rs` and the vendoring tests.
//!
//! Nothing here touches cargo or the filesystem beyond probing for a compiler,
//! so the acceptance tests can drive the same code the build script runs.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Zig version the vendored source dist requires.
///
/// `vendor/libghostty-vt/build.zig.zon` declares
/// `minimum_zig_version = "0.15.2"` and `src/build/zig.zig` turns a mismatch
/// into a `@compileError`, so this is an equality check, not a floor: 0.16.0
/// fails both that check and on `std.Io.Dir.readFileAlloc`, whose signature
/// changed after 0.15.2.
pub const REQUIRED_ZIG_VERSION: &str = "0.15.2";

/// Map a rust target triple onto the zig triple `-Dtarget` expects.
pub fn zig_target(target: &str) -> Result<&'static str, String> {
    Ok(match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        other => {
            return Err(format!(
                "no zig target mapping for {other}; add one to \
                 crates/amx-vt/build/toolchain.rs or supply a prebuilt library \
                 through AMX_LIBGHOSTTY_VT_LIB_DIR"
            ));
        }
    })
}

/// The host-specific directory name the vendored toolchain is unpacked into.
pub fn vendored_zig_dir_name() -> String {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    format!("zig-{arch}-{os}-{REQUIRED_ZIG_VERSION}")
}

/// Where a zig candidate came from, so the error message can say what to fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZigSource {
    /// `AMX_ZIG` named it explicitly.
    Env,
    /// `vendor/toolchain/` holds it — the toolchain the vendoring script fetches.
    Vendored,
    /// It was found on `PATH`.
    Path,
}

/// The message printed when no usable compiler is available.
///
/// It names the one command that fixes it, because the alternative — a raw
/// `@compileError` from deep inside the vendored build graph — sends people
/// reading zig source instead of running one script.
pub fn wrong_version_message(source: ZigSource, path: &Path, found: &str) -> String {
    let origin = match source {
        ZigSource::Env => "the compiler AMX_ZIG names",
        ZigSource::Vendored => "the vendored toolchain",
        ZigSource::Path => "the zig on PATH",
    };
    format!(
        "libghostty-vt needs Zig {REQUIRED_ZIG_VERSION}, but {origin} \
         ({}) is {found}.\n\
         Run `scripts/vendor-libghostty-vt.sh toolchain` to fetch the pinned \
         compiler into vendor/toolchain/, or set AMX_ZIG to a \
         {REQUIRED_ZIG_VERSION} binary, or set AMX_LIBGHOSTTY_VT_LIB_DIR to a \
         directory holding a prebuilt libghostty-vt to skip Zig entirely.",
        path.display()
    )
}

/// The message printed when no zig binary was found at all.
pub fn missing_zig_message(vendored: &Path) -> String {
    format!(
        "libghostty-vt needs Zig {REQUIRED_ZIG_VERSION} and no zig binary was \
         found — not at {}, not in AMX_ZIG, not on PATH.\n\
         Run `scripts/vendor-libghostty-vt.sh toolchain` to fetch the pinned \
         compiler, or set AMX_LIBGHOSTTY_VT_LIB_DIR to a directory holding a \
         prebuilt libghostty-vt to skip Zig entirely.",
        vendored.display()
    )
}

/// Read `zig version`, trimmed. `Err` carries a description of what failed.
pub fn probe_version(zig: &Path) -> Result<String, String> {
    let output = Command::new(zig)
        .arg("version")
        .output()
        .map_err(|err| format!("failed to run `{} version`: {err}", zig.display()))?;
    if !output.status.success() {
        return Err(format!(
            "`{} version` exited with {}",
            zig.display(),
            output.status
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|err| format!("`{} version` printed non-utf8: {err}", zig.display()))
        .map(|version| version.trim().to_string())
}

/// Find a Zig of exactly [`REQUIRED_ZIG_VERSION`].
///
/// Order: `AMX_ZIG`, then the vendored toolchain, then `PATH`. An explicit
/// `AMX_ZIG` of the wrong version is an error rather than a fallback — a
/// contributor who named a compiler wants to hear that it is the wrong one.
pub fn find_zig(vendor_dir: &Path, env_zig: Option<PathBuf>) -> Result<PathBuf, String> {
    let vendored = vendor_dir
        .join("toolchain")
        .join(vendored_zig_dir_name())
        .join("zig");

    let candidates: Vec<(ZigSource, PathBuf)> = match env_zig {
        Some(path) => vec![(ZigSource::Env, path)],
        None => vec![
            (ZigSource::Vendored, vendored.clone()),
            (ZigSource::Path, PathBuf::from("zig")),
        ],
    };

    let mut wrong: Option<String> = None;
    for (source, path) in candidates {
        let Ok(version) = probe_version(&path) else {
            continue;
        };
        if version == REQUIRED_ZIG_VERSION {
            return Ok(path);
        }
        // Keep the first mismatch: it is the most specific candidate, so it is
        // the one whose version the contributor most likely meant to use.
        wrong.get_or_insert_with(|| wrong_version_message(source, &path, &version));
    }

    Err(wrong.unwrap_or_else(|| missing_zig_message(&vendored)))
}
