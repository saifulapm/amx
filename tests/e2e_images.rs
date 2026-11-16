//! Images handed to an agent from the shell with `--image`: what each vendor
//! is given, and where the files are kept.

mod common;

use common::{Harness, argv_of};
use std::path::{Path, PathBuf};
use std::process::Output;

/// The first bytes of a PNG, which is all amx reads to know the type.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfirst image";
const OTHER_PNG: &[u8] = b"\x89PNG\r\n\x1a\nsecond image";

/// Run `amx new` with mock-claude copied to `claude` at the front of PATH,
/// so the table's claude entry decides how the images go.
fn new_as_claude(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the stand-in");
    std::fs::copy(amx.mock(), bin.join("claude")).expect("the stand-in under claude's name");
    amx.amx_command(&[&["new"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .current_dir(amx.home())
        .output()
        .expect("running amx new")
}

fn id_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Writes `bytes` to `name` under the harness's home.
fn an_image(amx: &Harness, name: &str, bytes: &[u8]) -> PathBuf {
    let path = amx.home().join(name);
    std::fs::write(&path, bytes).expect("the image");
    path
}

fn images_of(amx: &Harness, id: &str) -> PathBuf {
    amx.agent_dir(id).join("images")
}

#[test]
fn claude_is_given_each_image_as_an_at_word_before_the_task() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    an_image(&amx, "b.png", OTHER_PNG);
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--image", "a.png", "--image", "b.png", "x"],
    ));

    let images = images_of(&amx, &id);
    let argv = argv_of(&amx, &id);
    let want = format!(
        "@{} @{} x",
        images.join("1.png").display(),
        images.join("2.png").display()
    );
    assert!(argv.ends_with(&want), "{argv:?} ends with {want:?}");
    assert_eq!(std::fs::read(images.join("1.png")).unwrap(), PNG);
    assert_eq!(std::fs::read(images.join("2.png")).unwrap(), OTHER_PNG);
}

#[test]
fn a_missing_image_starts_nothing() {
    let amx = Harness::new();
    let out = new_as_claude(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "--image", "missing.png", "x"],
    );

    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("no such image: missing.png"), "{said}");
    assert!(no_records(&amx.state_root()), "no agent was recorded");
}

fn no_records(root: &Path) -> bool {
    std::fs::read_dir(root).map_or(true, |dirs| {
        dirs.flatten()
            .all(|entry| entry.file_name().to_string_lossy().starts_with('.'))
    })
}
