//! End-to-end tests for ctrl+v in the view's composer: an image on the
//! clipboard goes on the line as `[Image #N]`, text goes on as a paste, and a
//! clipboard that cannot be read leaves the line as it was with a notice.
//!
//! Each test puts a stand-in `wl-paste` first on the view's PATH, so the
//! clipboard of the machine running the tests is never read.

mod common;

use common::{Harness, agents, argv_of, finished, path_with, press, said_in, types, until_empty};
use std::path::{Path, PathBuf};

/// The first bytes of a PNG, then a few more, as the image on the clipboard.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really pixels";

/// A stand-in `wl-paste` that offers what `offers` holds and answers
/// `-t <type>` with the file `contents` names for that type.
///
/// With `offers` empty it lists nothing and fails, as wl-paste does on an
/// empty clipboard. Answers with the folder to put first on PATH.
fn a_clipboard(amx: &Harness, offers: &[(&str, &Path)]) -> PathBuf {
    let bin = amx.home().join("clipboard");
    std::fs::create_dir_all(&bin).unwrap();
    let mut listed = String::new();
    let mut answers = String::new();
    for (offered, file) in offers {
        listed.push_str(offered);
        listed.push('\n');
        answers.push_str(&format!(
            "  '{offered}') exec cat '{}' ;;\n",
            file.display()
        ));
    }
    let types = bin.join("types");
    std::fs::write(&types, listed).unwrap();
    let script = bin.join("wl-paste");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --list-types ]; then\n\
             \x20 [ -s '{types}' ] || {{ echo 'Nothing is copied' >&2; exit 1; }}\n\
             \x20 exec cat '{types}'\n\
             fi\n\
             while [ $# -gt 0 ]; do\n\
             \x20 [ \"$1\" = -t ] && {{ shift; wanted=\"$1\"; }}\n\
             \x20 shift\n\
             done\n\
             case \"$wanted\" in\n\
             {answers}\
             esac\n\
             exit 1\n",
            types = types.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin
}

/// A file under the harness's home holding `bytes`.
fn a_file(amx: &Harness, name: &str, bytes: &[u8]) -> PathBuf {
    let path = amx.home().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A PNG of exactly `size` bytes, built here rather than kept in the repo.
fn a_png_of(amx: &Harness, size: u64) -> PathBuf {
    let path = a_file(amx, "big.png", PNG);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size)
        .unwrap();
    path
}

/// The view, on a session with the clipboard `bin` stands in for.
fn a_view_on(amx: &Harness, bin: &Path) -> String {
    let path = path_with(bin);
    amx.in_a_terminal(&[("PATH", &path), ("WAYLAND_DISPLAY", "wayland-9")], &[])
}

/// An empty view's task line, opened with `n`.
fn a_task_line(amx: &Harness, bin: &Path) -> String {
    let view = a_view_on(amx, bin);
    until_empty(amx, &view);
    press(amx, &view, "n");
    shown(amx, &view, "TASK");
    view
}

/// Wait for `needle` anywhere on the view's screen.
fn shown(amx: &Harness, view: &str, needle: &str) {
    amx.until(&format!("the view to show {needle:?}"), || {
        amx.capture(view).contains(needle).then_some(())
    });
}

/// Wait for a row of the screen reading `line`.
fn until_a_row(amx: &Harness, view: &str, line: &str) -> String {
    amx.until(&format!("a row reading {line:?}"), || {
        let drawn = amx.capture(view);
        drawn
            .lines()
            .any(|row| row.trim_end() == line)
            .then_some(drawn)
    })
}

/// The files under the state's holding directory.
fn held(amx: &Harness) -> Vec<PathBuf> {
    let holding = amx.state_root().parent().unwrap().join("holding");
    match std::fs::read_dir(holding) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn ctrl_v_puts_an_image_on_the_task_line_behind_a_marker_and_holds_its_bytes() {
    let amx = Harness::new();
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png), ("text/plain", &png)]);
    let view = a_task_line(&amx, &bin);

    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ [Image #1]");
    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ [Image #1][Image #2]");

    let held = held(&amx);
    assert_eq!(held.len(), 2, "one holding file per marker: {held:?}");
    for file in &held {
        assert_eq!(
            std::fs::read(file).unwrap(),
            PNG,
            "{file:?} holds the image"
        );
        assert_eq!(
            file.extension().and_then(|it| it.to_str()),
            Some("png"),
            "named for its type"
        );
    }
}

#[test]
fn ctrl_v_pastes_text_where_the_clipboard_holds_no_image() {
    let amx = Harness::new();
    let hello = a_file(&amx, "hello.txt", b"hello");
    let bin = a_clipboard(&amx, &[("text/plain", &hello), ("UTF8_STRING", &hello)]);
    let view = a_task_line(&amx, &bin);

    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ hello");
    assert!(held(&amx).is_empty(), "text holds no file");
}

#[test]
fn ctrl_v_folds_a_long_text_the_way_a_paste_folds() {
    let amx = Harness::new();
    let long = a_file(&amx, "long.txt", "a".repeat(1200).as_bytes());
    let bin = a_clipboard(&amx, &[("text/plain;charset=utf-8", &long)]);
    let view = a_task_line(&amx, &bin);

    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ [Pasted text #1]");
}

#[test]
fn ctrl_v_on_a_rename_line_leaves_the_name_alone() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    let hello = a_file(&amx, "hello.txt", b"hello");
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png), ("text/plain", &hello)]);
    let view = a_view_on(&amx, &bin);
    shown(&amx, &view, "fix-login-a1b");

    press(&amx, &view, "C-r");
    until_a_row(&amx, &view, "❯ fix-login-a1b");
    press(&amx, &view, "C-v");
    // A key typed after it lands right after the name, so nothing went on.
    types(&amx, &view, "x");
    until_a_row(&amx, &view, "❯ fix-login-a1bx");
    assert!(held(&amx).is_empty());
}

#[test]
fn ctrl_v_on_the_find_line_leaves_the_search_alone() {
    let amx = Harness::new();
    finished(&amx, "port-a1b", "done", 60);
    let hello = a_file(&amx, "hello.txt", b"hello");
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png), ("text/plain", &hello)]);
    let view = a_view_on(&amx, &bin);
    shown(&amx, &view, "port-a1b");

    types(&amx, &view, "/");
    shown(&amx, &view, "/a name or task, or s:state");
    types(&amx, &view, "po");
    press(&amx, &view, "C-v");
    types(&amx, &view, "r");
    let drawn = amx.until("the search to read por", || {
        let drawn = amx.capture(&view);
        drawn
            .lines()
            .any(|row| row.trim_end() == "/por")
            .then_some(drawn)
    });
    assert!(!drawn.contains("Image"), "{drawn}");
    assert!(held(&amx).is_empty());
}

#[test]
fn ctrl_v_on_a_cards_reply_line_puts_the_image_on_it() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let view = a_view_on(&amx, &bin);
    shown(&amx, &view, "fix-login-a1b");

    press(&amx, &view, "Space");
    shown(&amx, &view, "❯ reply");
    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ [Image #1]");
    assert_eq!(held(&amx).len(), 1);
}

/// Type `look` on a task line, press ctrl+v, and answer with the screen once
/// `notice` is on it. The line must still read `look`.
fn refused(amx: &Harness, view: &str, notice: &str) -> String {
    types(amx, view, "look");
    until_a_row(amx, view, "❯ look");
    press(amx, view, "C-v");
    let drawn = amx.until(&format!("the notice {notice:?}"), || {
        let drawn = amx.capture(view);
        drawn.contains(notice).then_some(drawn)
    });
    assert!(
        drawn.lines().any(|row| row.trim_end() == "❯ look"),
        "the line is as it was:\n{drawn}"
    );
    assert!(held(amx).is_empty(), "and nothing is held");
    drawn
}

#[test]
fn ctrl_v_without_a_wayland_session_says_so_and_keeps_the_line() {
    let amx = Harness::new();
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let path = path_with(&bin);
    let view = amx.in_a_terminal(&[("PATH", &path), ("WAYLAND_DISPLAY", "")], &[]);
    until_empty(&amx, &view);
    press(&amx, &view, "n");
    shown(&amx, &view, "TASK");

    refused(&amx, &view, "no clipboard (no Wayland session)");
}

#[test]
fn ctrl_v_on_an_empty_clipboard_says_it_holds_nothing_and_keeps_the_line() {
    let amx = Harness::new();
    let bin = a_clipboard(&amx, &[]);
    let view = a_task_line(&amx, &bin);

    refused(&amx, &view, "the clipboard holds no image or text");
}

#[test]
fn ctrl_v_on_a_clipboard_of_neither_says_it_holds_nothing() {
    let amx = Harness::new();
    let html = a_file(&amx, "page.html", b"<b>hi</b>");
    let bin = a_clipboard(&amx, &[("text/html", &html)]);
    let view = a_task_line(&amx, &bin);

    refused(&amx, &view, "the clipboard holds no image or text");
}

#[test]
fn ctrl_v_refuses_an_image_over_the_limit_and_keeps_the_line() {
    let amx = Harness::new();
    let big = a_png_of(&amx, 21 * 1024 * 1024);
    let bin = a_clipboard(&amx, &[("image/png", &big)]);
    let view = a_task_line(&amx, &bin);

    refused(&amx, &view, "the image is 21 MB; the limit is 20 MB");
}

#[test]
fn ctrl_v_takes_an_image_at_the_limit_and_the_next_key_still_lands() {
    let amx = Harness::new();
    let big = a_png_of(&amx, 20 * 1024 * 1024);
    let bin = a_clipboard(&amx, &[("image/png", &big)]);
    let view = a_task_line(&amx, &bin);

    press(&amx, &view, "C-v");
    types(&amx, &view, "x");
    until_a_row(&amx, &view, "❯ [Image #1]x");
    let held = held(&amx);
    assert_eq!(held.len(), 1);
    assert_eq!(std::fs::metadata(&held[0]).unwrap().len(), 20 * 1024 * 1024);
}

/// An empty view whose `claude` is mock-claude printing its argv, with the
/// clipboard `bin` stands in for and `env` on top. Tasks start as claude,
/// without a worktree.
fn a_view_that_starts_claude(amx: &Harness, bin: &Path, env: &[(&str, &str)]) -> String {
    std::fs::copy(amx.mock(), bin.join("claude")).unwrap();
    amx.config("agent = \"claude\"\nworktrees = false\n");
    let scenario = amx.scenario("a-dispatched-worker");
    let scenario = scenario.to_string_lossy();
    let path = path_with(bin);
    let mut pairs = vec![
        ("PATH", path.as_str()),
        ("WAYLAND_DISPLAY", "wayland-9"),
        ("MOCK_CLAUDE_SCENARIO", &scenario),
    ];
    pairs.extend_from_slice(env);
    let view = amx.in_a_terminal(&pairs, &[]);
    until_empty(amx, &view);
    view
}

/// The one agent the view started, once its pane is on the record.
fn started(amx: &Harness) -> String {
    amx.until("the agent to be started", || {
        let ids = agents(amx);
        let id = (ids.len() == 1).then(|| ids[0].clone())?;
        amx.meta(&id)["pane"].as_str().map(|_| id)
    })
}

/// Assert the started agent's argv ends with its first image, then `task`.
fn given_the_image(amx: &Harness, id: &str, task: &str) {
    let argv = argv_of(amx, id);
    let want = format!(
        "@{} {task}",
        amx.agent_dir(id).join("images/1.png").display()
    );
    assert!(argv.ends_with(&want), "{argv:?} ends with {want:?}");
}

#[test]
fn a_task_line_starts_the_agent_with_its_image_and_lets_the_holding_file_go() {
    let amx = Harness::new();
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let view = a_view_that_starts_claude(&amx, &bin, &[]);

    press(&amx, &view, "n");
    shown(&amx, &view, "TASK");
    press(&amx, &view, "C-v");
    types(&amx, &view, " what is this");
    until_a_row(&amx, &view, "❯ [Image #1] what is this");
    press(&amx, &view, "Enter");

    let id = started(&amx);
    given_the_image(&amx, &id, "what is this");
    assert_eq!(
        std::fs::read(amx.agent_dir(&id).join("images/1.png")).unwrap(),
        PNG
    );
    assert!(held(&amx).is_empty(), "nothing is held: {:?}", held(&amx));
}

#[test]
fn a_reply_from_an_idle_claudes_card_hands_it_the_image() {
    let amx = Harness::new();
    amx.play("fix-login-a1b", "takes-a-message");
    amx.until_state("fix-login-a1b", "idle");
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let view = a_view_on(&amx, &bin);
    shown(&amx, &view, "fix-login-a1b");

    press(&amx, &view, "Space");
    shown(&amx, &view, "❯ reply");
    types(&amx, &view, "look ");
    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ look [Image #1]");
    press(&amx, &view, "Enter");

    let want = format!(
        "@{} look",
        amx.agent_dir("fix-login-a1b")
            .join("images/1.png")
            .display()
    );
    let pane = amx.pane_of("fix-login-a1b");
    amx.until("the image on claude's pane", || {
        said_in(&amx, &pane).contains(&want).then_some(())
    });
    assert!(held(&amx).is_empty(), "nothing is held: {:?}", held(&amx));
}

#[test]
fn esc_on_a_line_holding_an_image_lets_the_holding_file_go() {
    let amx = Harness::new();
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let view = a_task_line(&amx, &bin);

    press(&amx, &view, "C-v");
    until_a_row(&amx, &view, "❯ [Image #1]");
    assert_eq!(held(&amx).len(), 1);
    press(&amx, &view, "Escape");

    amx.until("the holding file to go", || {
        held(&amx).is_empty().then_some(())
    });
}

#[test]
fn a_line_sent_with_an_image_comes_back_as_its_words() {
    let amx = Harness::new();
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let view = a_view_that_starts_claude(&amx, &bin, &[]);

    press(&amx, &view, "n");
    shown(&amx, &view, "TASK");
    press(&amx, &view, "C-v");
    types(&amx, &view, " look");
    until_a_row(&amx, &view, "❯ [Image #1] look");
    press(&amx, &view, "Enter");
    started(&amx);

    press(&amx, &view, "n");
    shown(&amx, &view, "TASK");
    press(&amx, &view, "Up");
    until_a_row(&amx, &view, "❯ look");
}

#[test]
fn ctrl_g_gives_the_markers_back_with_their_images() {
    let amx = Harness::new();
    let png = a_file(&amx, "shot.png", PNG);
    let bin = a_clipboard(&amx, &[("image/png", &png)]);
    let view = a_view_that_starts_claude(&amx, &bin, &[("VISUAL", "true"), ("EDITOR", "true")]);

    press(&amx, &view, "n");
    shown(&amx, &view, "TASK");
    press(&amx, &view, "C-v");
    types(&amx, &view, " look");
    until_a_row(&amx, &view, "❯ [Image #1] look");
    press(&amx, &view, "C-g");
    // A key typed after the editor lands after what it saved.
    types(&amx, &view, "s");
    until_a_row(&amx, &view, "❯ [Image #1] looks");
    press(&amx, &view, "Enter");

    let id = started(&amx);
    given_the_image(&amx, &id, "looks");
}
