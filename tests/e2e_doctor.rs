//! `amx doctor` against a real tmux server.
//!
//! The check these prove exists because of a machine where every agent died in
//! under a second and doctor stayed green: a tmux server had outlived the
//! directory it was started in, and every pane it forked after that landed
//! somewhere that was not there. Nothing short of a real server proves it —
//! the deleted directory has to be one a real process is really holding.
//!
//! Linux only, which is where the check is. Elsewhere there is no way to read
//! another process's working directory and doctor says nothing about it, so
//! there is nothing here to exercise.
#![cfg(target_os = "linux")]

mod common;

use common::Harness;
use std::path::Path;
use std::process::Command;

/// A shell that sits there, so the server has a pane and stays up.
const IDLE: &[&str] = &["sh", "-c", "while :; do sleep 0.05; done"];

/// Start a server on this harness's socket from a client standing in `cwd`.
///
/// A server takes its working directory from whichever client started it and
/// not from the `-c` a session was asked for, so standing the client somewhere
/// is the only way to put a server there on purpose.
fn serve_from(amx: &Harness, cwd: &Path) {
    let out = Command::new("tmux")
        .args(["-L", amx.socket(), "-f", "/dev/null"])
        .args(["new-session", "-d"])
        .args(IDLE)
        .current_dir(cwd)
        .output()
        .expect("starting a server");
    assert!(out.status.success(), "{out:?}");
}

/// Doctor's line about the tmux server: whether it passed, and what it said.
///
/// Read by the check's name rather than by position, and the whole output is
/// carried along so a failure says what doctor actually printed.
fn server_line(printed: &str) -> (bool, String) {
    printed
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let verdict = fields.next()?;
            (fields.next()? == "server").then(|| (verdict == "ok", line.to_string()))
        })
        .unwrap_or_else(|| panic!("doctor said nothing about the server:\n{printed}"))
}

fn doctor(amx: &Harness) -> String {
    let out = amx.amx(&["doctor"]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_server_standing_somewhere_that_is_still_there_passes() {
    let amx = Harness::new();
    let dir = tempfile::TempDir::new().unwrap();
    serve_from(&amx, dir.path());

    let printed = doctor(&amx);
    let (ok, line) = server_line(&printed);
    assert!(ok, "nothing is wrong with this server: {line}");
}

#[test]
fn a_server_whose_directory_was_deleted_is_named_with_the_way_out() {
    let amx = Harness::new();
    let dir = tempfile::TempDir::new().unwrap();
    let gone = dir.path().canonicalize().unwrap();
    serve_from(&amx, &gone);

    // The whole failure, reproduced: the directory the server is standing in
    // goes, and the server carries on holding it.
    std::fs::remove_dir_all(&gone).unwrap();

    let printed = doctor(&amx);
    let (ok, line) = server_line(&printed);
    assert!(
        !ok,
        "the server is poisoned and doctor passed it: {printed}"
    );
    assert!(
        line.contains(&gone.display().to_string()),
        "the directory it is stuck holding is named: {line}"
    );
    assert!(
        printed.contains(&format!("tmux -L {} kill-server", amx.socket())),
        "and the way out is a command aimed at this server: {printed}"
    );
}

#[test]
fn a_machine_with_no_server_yet_has_nothing_to_report() {
    // Never having started a server is not a fault, and the next one amx
    // starts will stand somewhere real.
    let amx = Harness::new();

    let printed = doctor(&amx);
    // Doctor ran and said its piece, so the absence below is a check that was
    // not asked rather than output that never arrived.
    assert!(printed.contains("tmux"), "doctor said nothing at all");
    assert!(
        !printed.lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next();
            fields.next() == Some("server")
        }),
        "no line at all rather than a green one nobody measured:\n{printed}"
    );
}
