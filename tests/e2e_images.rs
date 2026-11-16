//! Images handed to an agent from the shell with `--image`: what each vendor
//! is given, and where the files are kept.

mod common;

use common::{AMX, Harness, argv_of, said_in};
use std::path::{Path, PathBuf};
use std::process::Output;

/// The first bytes of a PNG, which is all amx reads to know the type.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfirst image";
const OTHER_PNG: &[u8] = b"\x89PNG\r\n\x1a\nsecond image";

/// Run `amx new` with mock-claude copied to `claude` at the front of PATH,
/// so the table's claude entry decides how the images go.
fn new_as_claude(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    as_claude(amx, scenario, &[&["new"], args].concat())
}

/// Run amx with mock-claude under `claude`'s name on PATH, playing
/// `scenario`, from the harness's home.
fn as_claude(amx: &Harness, scenario: &str, args: &[&str]) -> Output {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the stand-in");
    std::fs::copy(amx.mock(), bin.join("claude")).expect("the stand-in under claude's name");
    amx.amx_command(args)
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("PATH", on_path(&[&bin]))
        .current_dir(amx.home())
        .output()
        .expect("running amx")
}

/// PATH with `dirs` in front.
fn on_path(dirs: &[&Path]) -> String {
    let ours: Vec<String> = dirs.iter().map(|dir| dir.display().to_string()).collect();
    format!("{}:{}", ours.join(":"), common::quiet_path())
}

/// Run amx with the stand-in in `tests/<stand_in>` on PATH, playing the
/// scenario at `scenario` under `variable`, from the harness's home.
///
/// This build of amx goes on PATH too, for the opencode plugin, which runs
/// `amx`.
fn as_stand_in(
    amx: &Harness,
    stand_in: &str,
    variable: &str,
    scenario: &Path,
    args: &[&str],
) -> Output {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for amx");
    if !bin.join("amx").exists() {
        std::os::unix::fs::symlink(AMX, bin.join("amx")).expect("amx on the PATH");
    }
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(stand_in);
    amx.amx_command(args)
        .env(variable, scenario)
        .env("PATH", on_path(&[&bin, &fixtures]))
        .current_dir(amx.home())
        .output()
        .expect("running amx")
}

fn pi(amx: &Harness, scenario: &Path, args: &[&str]) -> Output {
    as_stand_in(amx, "mock_pi", "MOCK_PI_SCENARIO", scenario, args)
}

fn opencode(amx: &Harness, scenario: &Path, args: &[&str]) -> Output {
    as_stand_in(
        amx,
        "mock_opencode",
        "MOCK_OPENCODE_SCENARIO",
        scenario,
        args,
    )
}

fn codex(amx: &Harness, scenario: &Path, args: &[&str]) -> Output {
    as_stand_in(amx, "mock_codex", "MOCK_CODEX_SCENARIO", scenario, args)
}

fn scenario_of(stand_in: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(stand_in)
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// Assert `out` succeeded, naming the verb and what it said if not.
fn went(out: &Output, verb: &str) {
    assert!(
        out.status.success(),
        "amx {verb}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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

#[test]
fn a_send_hands_claude_its_image_numbered_on_from_the_first() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    an_image(&amx, "b.png", OTHER_PNG);
    let id = id_of(&new_as_claude(
        &amx,
        "takes-a-message",
        &["--no-worktree", "--image", "a.png", "x"],
    ));
    amx.until_state(&id, "idle");

    let out = as_claude(
        &amx,
        "takes-a-message",
        &["send", &id, "--image", "b.png", "look"],
    );
    went(&out, "send");

    let want = format!("@{}/{id}/images/2.png look", amx.state_root().display());
    let pane = amx.pane_of(&id);
    amx.until("the message on claude's pane", || {
        said_in(&amx, &pane).contains(&want).then_some(())
    });
    assert_eq!(
        std::fs::read(images_of(&amx, &id).join("2.png")).unwrap(),
        OTHER_PNG
    );
}

#[test]
fn a_fork_keeps_its_image_in_the_copys_own_record() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    let id = id_of(&new_as_claude(
        &amx,
        "a-dispatched-worker",
        &["--no-worktree", "fix the login bug"],
    ));
    amx.until("the hook to report a session", || {
        amx.meta(&id)["session"].as_str().map(str::to_string)
    });

    let out = as_claude(
        &amx,
        "continues-a-session",
        &["fork", &id, "--image", "a.png", "x"],
    );
    went(&out, "fork");

    let copy = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        std::fs::read(amx.state_root().join(&copy).join("images/1.png")).unwrap(),
        PNG
    );
}

#[test]
fn pi_is_given_each_image_as_an_at_argument_before_the_task() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    let id = id_of(&pi(
        &amx,
        &scenario_of("mock_pi", "takes-a-turn"),
        &[
            "new",
            "--no-worktree",
            "--agent",
            "pi",
            "--image",
            "a.png",
            "x",
        ],
    ));

    // pi repaints over its argv line, which stays in the pane's history.
    let pane = amx.pane_of(&id);
    let argv = amx.until("pi to say how it was called", || {
        said_in(&amx, &pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    });
    let want = format!("-- @{}/{id}/images/1.png x", amx.state_root().display());
    assert!(argv.ends_with(&want), "{argv:?} ends with {want:?}");
}

#[test]
fn a_send_hands_pi_an_image_line_before_the_words() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    // Idle, then the second agent_start pi reports when a message arrives,
    // with no repaint after it, so what was typed stays on the pane.
    let timeline = amx.home().join("takes-a-message.scenario");
    std::fs::write(
        &timeline,
        "screen boot\n\
         hook session_start {}\n\
         sleep 50\n\
         hook agent_start {}\n\
         screen idle\n\
         hook agent_settled {\"last_assistant_message\":\"the tests pass now\"}\n\
         sleep 3000\n\
         hook agent_start {}\n\
         sleep 600000\n",
    )
    .unwrap();
    let id = id_of(&pi(
        &amx,
        &timeline,
        &["new", "--no-worktree", "--agent", "pi", "fix the login bug"],
    ));
    amx.until_state(&id, "idle");

    let out = pi(
        &amx,
        &timeline,
        &["send", &id, "--image", "a.png", "describe"],
    );
    went(&out, "send");

    let want = format!(
        "image: {}/{id}/images/1.png\ndescribe",
        amx.state_root().display()
    );
    let pane = amx.pane_of(&id);
    amx.until("the message on pi's pane", || {
        said_in(&amx, &pane).contains(&want).then_some(())
    });
}

#[test]
fn opencode_is_given_an_image_line_inside_its_prompt_word() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    let id = id_of(&opencode(
        &amx,
        &scenario_of("mock_opencode", "takes-a-turn"),
        &[
            "new",
            "--no-worktree",
            "--agent",
            "opencode",
            "--image",
            "a.png",
            "x",
        ],
    ));

    let want = format!(
        "prompt: 'image: {}/{id}/images/1.png\nx'",
        amx.state_root().display()
    );
    let pane = amx.pane_of(&id);
    amx.until("opencode to say its prompt", || {
        said_in(&amx, &pane).contains(&want).then_some(())
    });
}

#[test]
fn codex_is_given_each_image_as_an_image_flag_before_its_options_end() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    let id = id_of(&codex(
        &amx,
        &scenario_of("mock_codex", "takes-a-turn"),
        &[
            "new",
            "--no-worktree",
            "--agent",
            "codex",
            "--image",
            "a.png",
            "x",
        ],
    ));

    let pane = amx.pane_of(&id);
    let argv = amx.until("codex to say how it was called", || {
        said_in(&amx, &pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    });
    let want = format!(
        "argv: --no-daemon --image={}/{id}/images/1.png -- x",
        amx.state_root().display()
    );
    assert_eq!(argv, want);
}

/// A codex agent started on `scenario` with a task, once its first turn has
/// ended.
fn idle_codex(amx: &Harness, scenario: &Path) -> String {
    let id = id_of(&codex(
        amx,
        scenario,
        &[
            "new",
            "--no-worktree",
            "--agent",
            "codex",
            "fix the login bug",
        ],
    ));
    amx.until_state(&id, "idle");
    id
}

#[test]
fn a_send_pastes_codex_its_image_alone_and_the_words_once_the_chip_shows() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    let scenario = scenario_of("mock_codex", "takes-a-message");
    let id = idle_codex(&amx, &scenario);

    let out = codex(
        &amx,
        &scenario,
        &["send", &id, "--image", "a.png", "describe"],
    );
    went(&out, "send");

    let pane = amx.pane_of(&id);
    amx.until("the words on codex's pane after the chip", || {
        said_in(&amx, &pane)
            .contains("[Image #1]\ntyped: describe")
            .then_some(())
    });
}

#[test]
fn a_send_codex_shows_no_chip_for_presses_no_enter_and_fails() {
    let amx = Harness::new();
    an_image(&amx, "a.png", PNG);
    let scenario = scenario_of("mock_codex", "leaves-an-image-as-text");
    let id = idle_codex(&amx, &scenario);

    let began = std::time::Instant::now();
    let out = codex(
        &amx,
        &scenario,
        &["send", &id, "--image", "a.png", "describe"],
    );
    let took = began.elapsed();

    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("codex did not take the image"), "{said}");
    assert!(
        took >= std::time::Duration::from_secs(5) && took < std::time::Duration::from_secs(10),
        "gave up after the 5 s wait: {took:?}"
    );
    assert!(
        amx.event_kinds(&id).iter().any(|kind| kind == "unsent"),
        "the send is logged as unsent: {:?}",
        amx.event_kinds(&id)
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    let history = said_in(&amx, &amx.pane_of(&id));
    assert!(
        !history.contains("typed:"),
        "no Enter reached codex:\n{history}"
    );
}

#[test]
fn a_send_with_a_missing_image_types_nothing() {
    let amx = Harness::new();
    let id = id_of(&new_as_claude(
        &amx,
        "takes-a-message",
        &["--no-worktree", "fix the login bug"],
    ));
    amx.until_state(&id, "idle");

    let out = as_claude(
        &amx,
        "takes-a-message",
        &["send", &id, "--image", "missing.png", "carry on"],
    );

    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("no such image: missing.png"), "{said}");
    assert!(
        !amx.event_kinds(&id).iter().any(|kind| kind == "send"),
        "no send was logged: {:?}",
        amx.event_kinds(&id)
    );
    assert!(
        !said_in(&amx, &amx.pane_of(&id)).contains("carry on"),
        "and nothing was typed"
    );
}

fn no_records(root: &Path) -> bool {
    std::fs::read_dir(root).map_or(true, |dirs| {
        dirs.flatten()
            .all(|entry| entry.file_name().to_string_lossy().starts_with('.'))
    })
}
