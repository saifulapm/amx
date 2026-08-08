//! `amx work <branch>` and `amx work done`: git worktrees (D-M3-10).
//!
//! Against **real `git`** in a temporary repository, because every claim this
//! feature makes is a claim about what git did. A fake would let the suite pass
//! while `worktree add` was being handed its arguments in the wrong order, and
//! the one verb in the tree that can delete a directory somebody was working in
//! is not a verb to test against a stub.
//!
//! The rig is [`support::Rig`] — W10 wrote it inline for `update.rs`, W11 lifted
//! it out, and it is here because a worktree flow needs a binary with somewhere
//! for a repository to sit *beside* it: the default `work.dir` template puts the
//! tree next to the repository, so the harness has to own the parent directory
//! too.
//!
//! `git` is assumed present. Every runner amx builds on has it — the checkout
//! got there somehow — and a suite that skipped itself when the tool under test
//! was missing would be a suite that silently stopped testing this.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use amx_proto::control::session::{
    ReportReply, RestoreEntity, RestoreSeverity, StateReply, WorkspaceState,
};
use support::{Output, Rig};

/// A stanza for an agent that is nothing but a process that stays open.
///
/// `coverage = "none"` is what makes it this short: the class has no held state
/// to leave, so it needs neither a tier-2 manifest nor a hook subscription
/// (`agents.toml`'s conformance rules). What the test needs from an agent is that
/// `agent.start` splits a pane in the right directory and labels it — not that
/// fusion tracks it, which `agent_verbs`' own suite owns.
const FAKE_AGENT: &str = r#"
[[agent]]
id          = "fake"
label       = "Fake Agent"
executables = ["cat"]
coverage    = "none"
start       = ["cat"]
"#;

/// A rig with a one-commit git repository at `<root>/repo`, and a planted binary.
///
/// The repository's `user.name`/`user.email` are set on the repository rather
/// than read from the developer's own config, and `commit.gpgsign` is turned off:
/// a CI runner with no signing key and a developer with `commit.gpgsign = true`
/// must produce the same fixture.
///
/// The root is **canonical**, and every path this suite builds hangs off it.
/// Every path amx reports here has been through git, and git prints realpaths;
/// on darwin the rig's own root is not one, `$TMPDIR` sitting under `/var` and
/// `/var` being a symlink to `/private/var`. A fixture that compared its own
/// spelling against git's would be asserting the platform rather than the
/// feature — and would have nothing to say on Linux, where the two coincide.
struct Fixture {
    rig: Rig,
    exe: PathBuf,
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let rig = Rig::new(tag);
        let exe = rig.plant("bin/amx");
        let root = rig.root().canonicalize().expect("the rig root exists");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("create the repo directory");

        git(&repo, &["init", "-q", "-b", "main", "."]);
        git(&repo, &["config", "user.name", "amx tests"]);
        git(&repo, &["config", "user.email", "tests@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("README"), "a repository\n").expect("write a file");
        git(&repo, &["add", "README"]);
        git(&repo, &["commit", "-qm", "one"]);

        Self {
            rig,
            exe,
            root,
            repo,
        }
    }

    /// This fixture's root, as the filesystem spells it.
    fn root(&self) -> &Path {
        &self.root
    }

    /// Write `config.toml` for this rig.
    fn config(&self, text: &str) {
        std::fs::write(self.root().join("config/amx/config.toml"), text)
            .expect("write config.toml");
    }

    /// Plant the registry override, so `--kind fake` resolves.
    fn agents(&self) {
        std::fs::write(self.root().join("config/amx/agents.toml"), FAKE_AGENT)
            .expect("write agents.toml");
    }

    /// Start this fixture's server.
    fn serve(&mut self) {
        self.rig.serve(&self.exe);
    }

    /// Run `amx` with the repository as the working directory.
    ///
    /// From inside the repository, because that is the whole of how `amx work`
    /// finds one: `Repo::discover` asks git what it is standing in.
    fn amx(&self, args: &[&str]) -> Output {
        self.at(&self.repo, args)
    }

    /// Run `amx` from `dir`.
    fn at(&self, dir: &Path, args: &[&str]) -> Output {
        let out = self
            .rig
            .command(&self.exe)
            .current_dir(dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run amx");
        Output::of(&out)
    }

    /// The session as `session state` reports it.
    fn state(&self) -> StateReply {
        serde_json::from_str(self.amx(&["session", "state"]).ok()).expect("state deserializes")
    }

    /// The restore report, typed.
    fn report(&self) -> ReportReply {
        serde_json::from_str(self.amx(&["session", "report", "--json"]).ok())
            .expect("report deserializes")
    }

    /// The workspace labelled `label`, if the session has one.
    fn workspace(&self, label: &str) -> Option<WorkspaceState> {
        self.state()
            .workspaces
            .into_iter()
            .find(|ws| ws.label.as_deref() == Some(label))
    }

    /// Every worktree the repository has registered.
    fn worktrees(&self) -> Vec<String> {
        let listed = git(&self.repo, &["worktree", "list", "--porcelain"]);
        listed
            .lines()
            .filter_map(|line| line.strip_prefix("worktree ").map(str::to_owned))
            .collect()
    }
}

/// Run git in `dir` and hand back its stdout, failing the test if it refuses.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).expect("git spoke UTF-8")
}

/// The three things `amx work <branch>` makes, all named after the branch.
///
/// One command, three artifacts, and the point of the test is that they are
/// joined: the tree is a real registered worktree of the repository, the
/// workspace carries the membership block naming that exact path, and the agent
/// runs in a pane inside it. A version of this feature that made three unrelated
/// things would pass every assertion but the ones about the block.
#[test]
fn work_branch_creates_worktree_workspace_and_agent_and_names_all_three() {
    let mut fixture = Fixture::new("wka");
    fixture.agents();
    fixture.serve();

    let out = fixture.amx(&["work", "feat", "--kind", "fake"]);
    let said = out.ok().to_owned();
    let tree = fixture.root().join("repo--feat");

    // Named, all three, in what the command printed.
    assert!(said.contains(&tree.display().to_string()), "{said}");
    assert!(said.contains("workspace"), "{said}");
    assert!(said.contains("agent feat"), "{said}");

    // The tree: a real checkout of the branch, registered by the repository.
    assert!(tree.join("README").is_file(), "the branch was checked out");
    assert!(
        fixture.worktrees().contains(&tree.display().to_string()),
        "git lists the tree it was asked to add: {:?}",
        fixture.worktrees(),
    );
    // A new branch, made from HEAD, because `feat` did not exist.
    assert_eq!(
        git(&tree, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "feat",
    );

    // The workspace: labelled for the branch, and carrying the membership that
    // is the whole of what the server remembers about git.
    let ws = fixture
        .workspace("feat")
        .expect("a workspace labelled for the branch");
    let block = ws
        .worktree
        .expect("the workspace carries its worktree block");
    assert_eq!(block.branch, "feat");
    assert_eq!(block.path, tree);
    assert_eq!(block.repo, fixture.repo);

    // The agent: a pane in that workspace, labelled for the branch.
    let state = fixture.state();
    let panes = ws.layout.panes();
    assert_eq!(panes.len(), 2, "a root shell and the agent's pane");
    let named = state
        .panes
        .iter()
        .find(|pane| pane.label.as_deref() == Some("feat"))
        .expect("a pane labelled for the branch");
    assert!(
        panes.contains(&named.pane),
        "the agent's pane is in the branch's workspace",
    );

    // And the workspace really is *on* the tree: its root shell was started
    // there, which is what makes the membership more than a label.
    let root = state
        .panes
        .iter()
        .find(|pane| pane.pane == panes[0])
        .expect("the root pane is reported");
    assert!(
        root.label.is_none(),
        "the root pane is the shell, not the agent"
    );
}

/// `done` takes the workspace, its panes and the tree — and refuses to take any
/// of them while the tree has uncommitted work in it.
///
/// The refusal is asserted to have changed *nothing*: this is the ordering that
/// matters most in the whole feature, because a `done` that killed the panes and
/// then declined to remove the tree would have destroyed the half of the pair the
/// user could not rebuild.
#[test]
fn work_done_collapses_all_three_and_refuses_a_dirty_tree_without_force() {
    let mut fixture = Fixture::new("wkd");
    fixture.serve();
    fixture.amx(&["work", "feat"]).ok();

    let tree = fixture.root().join("repo--feat");
    let panes_before = fixture.state().panes.len();
    std::fs::write(tree.join("scratch.txt"), "unfinished\n").expect("dirty the tree");

    let said = fixture.amx(&["work", "done", "feat"]).failed().to_owned();
    assert!(said.contains("uncommitted"), "{said}");
    assert!(said.contains("--force"), "the way past it is named: {said}");
    assert!(tree.is_dir(), "the refusal left the tree alone");
    assert!(
        fixture.workspace("feat").is_some(),
        "the refusal left the workspace alone",
    );
    assert_eq!(
        fixture.state().panes.len(),
        panes_before,
        "the refusal killed no pane",
    );

    // A tracked file modified rather than an untracked one added: the same
    // refusal, because `git worktree remove`'s own rule is either.
    std::fs::remove_file(tree.join("scratch.txt")).expect("clean up");
    std::fs::write(tree.join("README"), "edited\n").expect("modify a tracked file");
    assert!(
        fixture
            .amx(&["work", "done", "feat"])
            .failed()
            .contains("uncommitted"),
    );
    assert!(tree.is_dir());

    // Forced: all three collapse together.
    let said = fixture
        .amx(&["work", "done", "feat", "--force"])
        .ok()
        .to_owned();
    assert!(said.contains("removed"), "{said}");
    assert!(!tree.exists(), "the tree is gone: {said}");
    assert!(fixture.workspace("feat").is_none(), "the workspace is gone");
    assert!(
        !fixture.worktrees().contains(&tree.display().to_string()),
        "git no longer lists it: {:?}",
        fixture.worktrees(),
    );
    // The branch is a place work lives; nothing here deletes one.
    assert_eq!(
        git(&fixture.repo, &["rev-parse", "--abbrev-ref", "feat"]).trim(),
        "feat",
    );
    assert!(said.contains("branch feat is untouched"), "{said}");
}

/// `amx work done` with no branch is about the focused workspace — and refuses
/// outright when that workspace is not one `amx work` made.
///
/// The property this pins is that `done` cannot become a way to lose a workspace
/// nobody asked it about. A no-branch `done` typed in the wrong window has to be
/// a refusal, not a kill, because `amx workspace kill` is right there for a
/// workspace that is only a workspace.
#[test]
fn work_done_with_no_branch_takes_the_focused_tree_and_refuses_a_plain_workspace() {
    let mut fixture = Fixture::new("wkf");
    fixture.serve();

    // A plain workspace, focused: the case that must refuse.
    let created: serde_json::Value = serde_json::from_str(
        fixture
            .amx(&["workspace", "create", "--params", r#"{"label":"plain"}"#])
            .ok(),
    )
    .expect("create reply is JSON");
    let plain = created["workspace"].as_str().expect("a workspace id");
    fixture
        .amx(&[
            "workspace",
            "switch",
            "--params",
            &format!(r#"{{"workspace":"{plain}"}}"#),
        ])
        .ok();
    let said = fixture.amx(&["work", "done"]).failed().to_owned();
    assert!(said.contains("not made by `amx work`"), "{said}");
    assert!(said.contains("plain"), "it names which one: {said}");
    assert!(
        fixture.workspace("plain").is_some(),
        "the refusal killed nothing",
    );

    // `amx work` switches to what it made, so a no-branch `done` right after it
    // means that tree — the "the workspace this pane is in" default.
    fixture.amx(&["work", "feat"]).ok();
    let tree = fixture.root().join("repo--feat");
    assert!(tree.is_dir());
    fixture.amx(&["work", "done"]).ok();
    assert!(!tree.exists(), "the focused tree came down");
    assert!(fixture.workspace("feat").is_none());
    assert!(
        fixture.workspace("plain").is_some(),
        "and only that one did",
    );
}

/// A tree somebody removed between two runs of the server: the workspace comes
/// back as a plain one, and the loss reaches `amx session report`.
///
/// Never log-only — 04 §6's rule, and M1's report machinery is exactly where a
/// loss like this belongs. Degraded and not lost, because every pane, the layout
/// and the label all come back: one field does not.
#[test]
fn a_vanished_worktree_restores_as_a_plain_workspace_with_a_report_entry() {
    let mut fixture = Fixture::new("wkv");
    fixture.serve();
    fixture.amx(&["work", "feat"]).ok();
    let tree = fixture.root().join("repo--feat");
    let workspace = fixture
        .workspace("feat")
        .expect("the workspace exists before the restart")
        .workspace;

    // Stopped rather than killed, so the snapshot on disk is the one M1's
    // shutdown push writes.
    fixture.amx(&["session", "stop"]).ok();
    std::fs::remove_dir_all(&tree).expect("remove the tree behind amx's back");
    fixture.serve();

    // The workspace is still here, and it is plain now.
    let ws = fixture
        .workspace("feat")
        .expect("the workspace survives the vanished tree");
    assert_eq!(ws.workspace, workspace, "under the id it had");
    assert_eq!(
        ws.worktree, None,
        "a workspace whose tree is gone is a plain workspace",
    );

    // And the loss is queryable, with the path and the branch in it.
    let report = fixture.report();
    let entry = report
        .report
        .entries
        .iter()
        .find(|entry| entry.entity == RestoreEntity::Workspace)
        .expect("the report names the workspace");
    assert_eq!(entry.severity, RestoreSeverity::Degraded);
    assert_eq!(entry.workspace, Some(workspace));
    assert_eq!(entry.path.as_deref(), Some(tree.as_path()));
    assert!(entry.reason.contains("feat"), "{}", entry.reason);
    assert!(entry.reason.contains("worktree"), "{}", entry.reason);

    // The human form says it too, since that is the one a person runs.
    let printed = fixture.amx(&["session", "report"]).ok().to_owned();
    assert!(printed.contains("degraded"), "{printed}");
    assert!(printed.contains(&tree.display().to_string()), "{printed}");

    // `done` still works on it: the membership is gone, so the branch names
    // nothing — which is a refusal that says so rather than a wrong removal.
    assert!(
        fixture
            .amx(&["work", "done", "feat"])
            .failed()
            .contains("no workspace"),
    );
}

/// Run from inside one worktree, `amx work` puts the next one beside the
/// *repository* — not beside its sibling.
///
/// The whole reason `Repo::discover` reads `git worktree list` rather than
/// `--show-toplevel`: a template built on `{repo_parent}` would otherwise stack
/// trees inside each other the moment somebody ran the verb from a tree they had
/// just made, which is the ordinary way to use it.
#[test]
fn work_from_inside_a_worktree_places_the_next_one_beside_the_repository() {
    let mut fixture = Fixture::new("wkn");
    fixture.serve();
    fixture.amx(&["work", "first"]).ok();
    let first = fixture.root().join("repo--first");

    fixture.at(&first, &["work", "second"]).ok();
    let second = fixture.root().join("repo--second");
    assert!(second.join("README").is_file(), "beside the repository");
    assert!(
        !first.join("repo--second").exists(),
        "and not nested inside the tree the verb was run from",
    );
    assert_eq!(
        fixture
            .workspace("second")
            .and_then(|ws| ws.worktree)
            .map(|tree| tree.repo),
        Some(fixture.repo.clone()),
        "the membership names the repository, not the worktree it was run from",
    );
}

/// `work.dir` decides where the tree goes, and `done` re-derives it.
///
/// Both halves in one test on purpose: a template that only the *creating* side
/// honored would make `done` refuse to take down what `work` had just made, and a
/// template only `done` honored would make it refuse to take down anything at
/// all. The pin on the destructive path is that the two agree.
#[test]
fn work_dir_template_is_config_overridable() {
    let mut fixture = Fixture::new("wkt");
    fixture.config("[work]\ndir = \"{repo_parent}/trees/{branch}\"\n");
    fixture.serve();

    fixture.amx(&["work", "feat"]).ok();
    let configured = fixture.root().join("trees").join("feat");
    assert!(
        configured.join("README").is_file(),
        "the tree went where the template said",
    );
    assert!(
        !fixture.root().join("repo--feat").exists(),
        "and not where the default would have put it",
    );
    assert_eq!(
        fixture
            .workspace("feat")
            .and_then(|ws| ws.worktree)
            .map(|tree| tree.path),
        Some(configured.clone()),
        "the membership records the configured path",
    );

    // `done` derives the same path from the same template, and takes it down.
    fixture.amx(&["work", "done", "feat"]).ok();
    assert!(!configured.exists(), "the configured tree is gone");

    // Now the template moves under a tree that already exists. `done` must
    // refuse rather than remove a directory the current template does not
    // derive: the recorded path is not user input, and neither is this one.
    fixture.amx(&["work", "second"]).ok();
    let second = fixture.root().join("trees").join("second");
    assert!(second.is_dir());
    fixture.config("[work]\ndir = \"{repo_parent}/other/{branch}\"\n");
    let said = fixture.amx(&["work", "done", "second"]).failed().to_owned();
    assert!(said.contains("refusing to remove"), "{said}");
    assert!(said.contains(&second.display().to_string()), "{said}");
    assert!(second.is_dir(), "and it removed nothing");
    assert!(
        fixture.workspace("second").is_some(),
        "nor killed the workspace",
    );
}

/// A `work.dir` that reaches its tree through a symlink is still one worktree.
///
/// darwin's bug, made reproducible anywhere. There, `$TMPDIR` is under `/var`
/// and `/var` is a symlink to `/private/var`, so *every* path derived from a
/// template rooted in a temporary directory is spelled one way while `git
/// worktree list` prints the other — and the whole feature was verified on
/// Linux, where the two coincide and nothing is wrong. A symlinked `work.dir`
/// is the same mismatch, asked for on purpose, on any platform.
///
/// What it pins is the destructive path: the membership records the spelling
/// git registered, and `done`'s second lock — "the repository lists this as one
/// of its worktrees" — compares directories rather than characters. Compared as
/// characters it refuses a tree amx made itself, which is `amx work done`
/// unusable rather than `amx work done` cautious.
#[test]
fn a_work_dir_through_a_symlink_is_the_worktree_git_registered() {
    let mut fixture = Fixture::new("wkl");
    let real = fixture.root().join("elsewhere");
    let link = fixture.root().join("link");
    std::fs::create_dir_all(&real).expect("create the directory the link points at");
    std::os::unix::fs::symlink(&real, &link).expect("plant the symlink");
    fixture.config(&format!(
        "[work]\ndir = \"{}/{{branch}}\"\n",
        link.display()
    ));
    fixture.serve();

    fixture.amx(&["work", "feat"]).ok();
    let tree = real.join("feat");
    assert!(
        tree.join("README").is_file(),
        "the tree was checked out through the link",
    );
    assert!(
        fixture.worktrees().contains(&tree.display().to_string()),
        "git registers the tree under the target's spelling: {:?}",
        fixture.worktrees(),
    );

    // The membership is git's spelling too — the one `done` joins against and
    // the one restore stats — and not the template's.
    assert_eq!(
        fixture
            .workspace("feat")
            .and_then(|ws| ws.worktree)
            .map(|block| block.path),
        Some(tree.clone()),
        "the recorded path is the directory, not the template's characters",
    );

    // And `done` takes it down instead of refusing a tree of its own making.
    let said = fixture.amx(&["work", "done", "feat"]).ok().to_owned();
    assert!(said.contains("removed"), "{said}");
    assert!(!tree.exists(), "the tree is gone: {said}");
    assert!(
        fixture.workspace("feat").is_none(),
        "and so is the workspace"
    );
    assert!(
        link.symlink_metadata().is_ok(),
        "removing the tree did not follow the link out of its own directory",
    );
}
