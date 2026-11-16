//! ctrl+v on a line: the Wayland clipboard read through `wl-paste`.
//!
//! An image is written to a holding file and goes on the line as a marker;
//! text goes on as a paste. Any clipboard owner can be slow or hang, so the
//! whole read is bounded and the view goes back to its keys either way.

use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use super::act::Composer;
use super::paint::Notice;
use crate::images::too_big;
use crate::paths;

/// How long the whole read may take, both calls to `wl-paste` together.
const WAIT: Duration = Duration::from_secs(2);

/// The image types a line takes, in the order one is chosen when several are
/// offered, with the extension its holding file gets.
const IMAGES: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
];

/// The names a clipboard owner offers plain text under.
const TEXTS: [&str; 5] = [
    "text/plain",
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "TEXT",
    "STRING",
];

const NOTHING: &str = "the clipboard holds no image or text";

/// What the clipboard held.
enum Held {
    Image {
        bytes: Vec<u8>,
        extension: &'static str,
    },
    Text(String),
}

/// Puts what the clipboard holds on `composer` at the cursor, or answers why
/// nothing went on. An image is held under `state_root`'s holding directory.
pub fn paste_onto(composer: &mut Composer, state_root: &Path) -> Option<Notice> {
    let path = std::env::var_os("PATH");
    let display = std::env::var_os("WAYLAND_DISPLAY");
    match read(path.as_deref(), display.as_deref()) {
        Ok(Held::Text(text)) => composer.paste(&text.replace("\r\n", "\n").replace('\r', "\n")),
        Ok(Held::Image { bytes, extension }) => match hold(state_root, &bytes, extension) {
            Ok(holding) => composer.attach(holding),
            Err(e) => return Some(Notice::Failed(format!("{e:#}"))),
        },
        Err(notice) => return Some(notice),
    }
    None
}

/// Reads the clipboard with the `wl-paste` found on `path`, in the Wayland
/// session `display` names.
fn read(path: Option<&OsStr>, display: Option<&OsStr>) -> Result<Held, Notice> {
    let Some(program) = wl_paste_on(path) else {
        return Err(Notice::Refused(
            "no clipboard (wl-paste not found)".to_string(),
        ));
    };
    if display.is_none_or(OsStr::is_empty) {
        return Err(Notice::Refused(
            "no clipboard (no Wayland session)".to_string(),
        ));
    }
    let deadline = Instant::now() + WAIT;
    let nothing = || Notice::Refused(NOTHING.to_string());

    // An empty clipboard makes wl-paste fail rather than list nothing.
    let listed = output(&program, &["--list-types"], deadline)?.ok_or_else(nothing)?;
    let listed = String::from_utf8_lossy(&listed);
    let offered: Vec<&str> = listed.lines().map(str::trim).collect();

    if let Some((kind, extension)) = IMAGES.iter().find(|(kind, _)| offered.contains(kind)) {
        let bytes = output(&program, &["-n", "-t", kind], deadline)?.ok_or_else(nothing)?;
        if let Some(why) = too_big(bytes.len() as u64) {
            return Err(Notice::Refused(why));
        }
        return Ok(Held::Image { bytes, extension });
    }
    if let Some(kind) = TEXTS.iter().find(|kind| offered.contains(kind)) {
        // Without -n, wl-paste ends text with a newline the copy did not have.
        let bytes = output(&program, &["-n", "-t", kind], deadline)?.ok_or_else(nothing)?;
        return Ok(Held::Text(String::from_utf8_lossy(&bytes).into_owned()));
    }
    Err(nothing())
}

/// The first runnable `wl-paste` on `path`.
fn wl_paste_on(path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|dir| dir.join("wl-paste"))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|found| found.is_file() && found.permissions().mode() & 0o111 != 0)
        })
}

/// What `program` wrote to stdout, or `None` when it failed.
///
/// stdout is read on a thread while the program runs, so a 20 MB image does
/// not fill the pipe, and the wait ends at `deadline` whatever the program
/// does.
fn output(program: &Path, args: &[&str], deadline: Instant) -> Result<Option<Vec<u8>>, Notice> {
    let failed = |e: std::io::Error| Notice::Failed(format!("wl-paste: {e}"));
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failed)?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (written, read) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = written.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });

    let bytes = match read.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(bytes) => bytes.map_err(failed)?,
        Err(_) => return Err(given_up(child)),
    };
    loop {
        if let Some(ended) = child.try_wait().map_err(failed)? {
            return Ok(ended.success().then_some(bytes));
        }
        if Instant::now() >= deadline {
            return Err(given_up(child));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Kills a `wl-paste` that ran past the deadline, and says so.
fn given_up(mut child: Child) -> Notice {
    let _ = child.kill();
    let _ = child.wait();
    Notice::Failed(format!(
        "the clipboard did not answer within {} s",
        WAIT.as_secs()
    ))
}

/// Writes an image to a new file in the holding directory and answers its
/// path.
fn hold(state_root: &Path, bytes: &[u8], extension: &str) -> Result<PathBuf> {
    // Two images pasted within the same clock tick still get two names.
    static PASTED: AtomicUsize = AtomicUsize::new(0);
    let dir = paths::holding_dir(state_root).context("no directory to hold an image in")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let since = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!(
        "{since}-{}-{}.{extension}",
        std::process::id(),
        PASTED.fetch_add(1, Ordering::Relaxed)
    );
    let holding = dir.join(name);
    let mut file = std::fs::File::create_new(&holding)
        .with_context(|| format!("creating {}", holding.display()))?;
    // A screenshot can hold anything that was on the screen.
    paths::keep_to_the_owner(&holding, 0o600)?;
    std::io::Write::write_all(&mut file, bytes)
        .with_context(|| format!("writing {}", holding.display()))?;
    Ok(holding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn refusal(read: Result<Held, Notice>) -> String {
        match read {
            Err(Notice::Refused(why)) => why,
            Err(Notice::Failed(why) | Notice::Advice(why)) => panic!("not a refusal: {why}"),
            Ok(_) => panic!("the clipboard was read"),
        }
    }

    #[test]
    fn clipboard_with_no_wl_paste_on_path_says_so_before_asking_for_a_session() {
        let empty = TempDir::new().unwrap();
        assert_eq!(
            refusal(read(Some(empty.path().as_os_str()), None)),
            "no clipboard (wl-paste not found)"
        );
        assert_eq!(
            refusal(read(None, Some(OsStr::new("wayland-0")))),
            "no clipboard (wl-paste not found)"
        );
    }

    #[test]
    fn clipboard_outside_a_wayland_session_says_so() {
        let bin = TempDir::new().unwrap();
        let program = bin.path().join("wl-paste");
        std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = Some(bin.path().as_os_str());

        for display in [None, Some(OsStr::new(""))] {
            assert_eq!(
                refusal(read(path, display)),
                "no clipboard (no Wayland session)"
            );
        }
    }
}
