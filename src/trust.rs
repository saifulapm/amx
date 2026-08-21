//! Answering claude's folder-trust screen for a tree amx cut.
//!
//! claude asks once per folder it has never worked in — "Is this a project you
//! created or one you trust?" — and it draws that question in front of the
//! session every hook comes from, so no hook can report it. An agent that
//! meets the screen sits on it until somebody attaches and answers.
//!
//! A tree amx cut a second ago, inside a repository the person is already
//! working in, is the one case where there is nothing to decide. So amx
//! answers it, by writing the entry the vendor's own message tells you to
//! write, and it answers for that tree and nothing else: never the repository
//! around it, and never a directory somebody merely pointed amx at.
//!
//! Everything here was measured on claude 2.1.237, and is the vendor's own
//! file, so re-measure it at every vendor bump:
//!
//! * The store is `$CLAUDE_CONFIG_DIR/.claude.json`, and `~/.claude.json`
//!   when that variable is unset. Both were watched being written.
//! * The entry is `projects["<dir>"].hasTrustDialogAccepted: true`. That is
//!   the vendor's own instruction, printed verbatim when it drops
//!   project-scoped settings from a workspace nobody has trusted.
//! * A linked worktree also inherits the trust of the repository it belongs
//!   to, so a tree in a repository the person has already trusted needs no
//!   entry at all — which is why [`seed`] looks before it writes, and why
//!   the store does not grow an entry per agent on the usual day.
//!
//! One measured limit stays: where the tree carries a `.claude/settings.json`
//! that pre-approves tool permissions, and the *repository* has never been
//! trusted, the vendor still asks about those permissions. Only the
//! repository's own entry silences that, and pre-approving somebody else's
//! tool permissions in a repository they have never opened is not a decision
//! amx makes for them. The trust screen itself is gone either way.
//!
//! The three rules `install` holds over settings.json hold here too: nothing
//! is touched without a copy of the file as it was, foreign keys are carried
//! through untouched, and a file amx cannot parse is a file amx does not
//! write. The round trip costs key order — this file is re-printed the way
//! serde prints it — which is what the copy is for, and what the vendor
//! undoes on its own next write.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::{install, registry, worktree};

/// The one vendor whose trust amx knows how to answer.
const VENDOR: &str = "claude";

/// How long amx waits for the vendor's lock before leaving the file alone,
/// and how often it looks again while it waits.
const PATIENCE: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_millis(20);

/// When a lock nobody has refreshed counts as abandoned. The vendor's own
/// threshold, so that amx judges an orphan the way the vendor judges one.
const STALE: Duration = Duration::from_secs(10);

/// The vendor's config file, and the two keys inside it that decide the
/// screen.
const STORE: &str = ".claude.json";
const PROJECTS: &str = "projects";
const ACCEPTED: &str = "hasTrustDialogAccepted";

/// The variable that moves the whole file somewhere else.
const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// Whether the agent about to be launched is the one amx can answer for.
pub fn is_vendor(agent: &str) -> bool {
    registry::program(agent) == VENDOR
}

/// Where the store is for an agent that will run with `env`.
///
/// The agent's own environment rather than amx's, because it is the agent
/// that will read the file. An empty variable reads as unset, which is what a
/// shell that exports `FOO=` produces.
pub fn store_in(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let set = |key: &str| env.get(key).filter(|value| !value.is_empty());
    set(CONFIG_DIR)
        .or_else(|| set("HOME"))
        .map(|dir| Path::new(dir).join(STORE))
}

/// The key the vendor looks a folder up by.
///
/// It asks the operating system where it is running and gets a path with
/// every symlink already resolved, so amx resolves the same way or writes an
/// entry nothing will ever match. A path that will not resolve is used as it
/// stands.
pub fn key_for(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Whether the store already lets an agent work in `dir` without asking.
pub fn trusted(store: &Value, dir: &Path) -> bool {
    store[PROJECTS][key_for(dir)][ACCEPTED] == Value::Bool(true)
}

/// Say that `dir` is trusted, leaving every other key exactly as it was, and
/// answer whether anything needed saying.
pub fn merge(store: &mut Value, dir: &Path) -> bool {
    if !readable(store, dir) || trusted(store, dir) {
        return false;
    }
    let projects = store
        .as_object_mut()
        .expect("an object")
        .entry(PROJECTS)
        .or_insert_with(|| json!({}));
    let entry = projects
        .as_object_mut()
        .expect("an object")
        .entry(key_for(dir))
        .or_insert_with(|| json!({}));
    entry[ACCEPTED] = json!(true);
    true
}

/// Answer the screen for `tree`, and say whether that took writing anything.
///
/// `inherits` is the repository the tree belongs to: the vendor resolves a
/// linked worktree to it, so a repository somebody has already trusted covers
/// every tree amx cuts inside it and there is nothing to write.
pub fn seed(store: &Path, tree: &Path, inherits: Option<&Path>, now: u64) -> Result<bool> {
    seed_within(store, tree, inherits, now, PATIENCE)
}

/// The same, with the wait for the vendor's lock named.
fn seed_within(
    store: &Path,
    tree: &Path,
    inherits: Option<&Path>,
    now: u64,
    patience: Duration,
) -> Result<bool> {
    if !worktree::is_amx_tree(tree) {
        bail!(
            "{} is not a tree amx made, so its trust is not amx's to answer",
            tree.display()
        );
    }

    // Looked at before the lock is asked for: the ordinary spawn is into a
    // repository somebody has trusted already, and standing in the vendor's
    // way to find out there is nothing to do would be a poor trade.
    if covered(&read(store)?.unwrap_or_else(|| json!({})), tree, inherits) {
        return Ok(false);
    }

    let Some(_held) = Held::take(store, patience, STALE)? else {
        bail!(
            "{} is being written by claude, so amx left it alone",
            store.display()
        );
    };

    // Read again inside the lock: what was looked at a moment ago is what the
    // vendor may have been in the middle of replacing.
    let existing = read(store)?;
    let mut document = existing.clone().unwrap_or_else(|| json!({}));
    if covered(&document, tree, inherits) {
        return Ok(false);
    }
    if !merge(&mut document, tree) {
        bail!("{} is not a trust store amx can read", store.display());
    }

    back_up(store, now, existing.is_some())?;
    write(store, &document)?;
    Ok(true)
}

/// Whether the vendor would let an agent into `tree` as things stand, either
/// because the tree is trusted or because the repository it belongs to is.
fn covered(store: &Value, tree: &Path, inherits: Option<&Path>) -> bool {
    trusted(store, tree) || inherits.is_some_and(|repo| trusted(store, repo))
}

/// The lock the vendor holds while it writes, taken the vendor's own way.
///
/// claude guards every write to this file with a directory beside it: whoever
/// manages to make `<store>.lock` holds it, and one nobody has touched for
/// [`STALE`] was left behind by a claude that died. amx takes the same lock
/// the same way rather than writing behind the vendor's back — a claude in
/// another pane saving its own session rewrites this whole document, and two
/// writers who do not agree on a lock lose one of the two sets of changes.
struct Held(PathBuf);

impl Held {
    /// Take it, or answer `None` when somebody else has it and will not let go
    /// inside `patience`.
    fn take(store: &Path, patience: Duration, stale: Duration) -> Result<Option<Held>> {
        let path = lock_beside(store);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let waiting = Instant::now();
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Some(Held(path))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e).with_context(|| format!("taking {}", path.display())),
            }
            if abandoned(&path, stale) {
                sweep(&path, stale);
                continue;
            }
            if waiting.elapsed() >= patience {
                return Ok(None);
            }
            std::thread::sleep(RETRY);
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Clear away a lock that reads as abandoned, without ever taking a live one.
///
/// Judged stale and then removed are two acts, and the lock a plain remove
/// meets may not be the lock the judgement was about: another taker can sweep
/// and make a fresh lock of its own in between, and removing *that* leaves
/// two writers each believing the store is theirs. So the lock is renamed
/// aside first — to a name only this process uses, making whatever was caught
/// this process's alone — and only then looked at. A catch that really is
/// abandoned goes away; a live lock caught in the window between somebody's
/// create and this rename goes straight back where it was.
fn sweep(path: &Path, stale: Duration) {
    // Unique among every taker there could be: two processes differ by pid,
    // two sweeps within one process by the count.
    static SWEPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nth = SWEPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let aside = path.with_file_name(format!("{name}.stale-{}-{nth}", std::process::id()));
    if std::fs::rename(path, &aside).is_err() {
        // Somebody else swept or released it first; the path is theirs to
        // have cleared, and the next create will find out.
        return;
    }
    if !abandoned(&aside, stale) && std::fs::rename(&aside, path).is_ok() {
        return;
    }
    // Truly abandoned — or a putback that lost its place, in which case the
    // lock it held is already gone to whoever created meanwhile.
    let _ = std::fs::remove_dir_all(&aside);
}

/// Where the vendor's lock for `store` goes.
fn lock_beside(store: &Path) -> PathBuf {
    let name = store.file_name().unwrap_or_default().to_string_lossy();
    store.with_file_name(format!("{name}.lock"))
}

/// Whether a lock has gone unrefreshed long enough to be nobody's. The vendor
/// touches its own while it holds it, so age is the whole of the question.
fn abandoned(lock: &Path, stale: Duration) -> bool {
    std::fs::metadata(lock)
        .and_then(|found| found.modified())
        .is_ok_and(|touched| {
            SystemTime::now()
                .duration_since(touched)
                .is_ok_and(|since| since >= stale)
        })
}

/// Whether this document is shaped the way the vendor writes one, as far as
/// the one entry amx is about to touch. Anything else is somebody's own file
/// that happens to share a name.
fn readable(store: &Value, dir: &Path) -> bool {
    store.is_object()
        && store.get(PROJECTS).is_none_or(Value::is_object)
        && store[PROJECTS]
            .get(key_for(dir))
            .is_none_or(Value::is_object)
}

/// Copy the file aside before changing it, once.
///
/// Once, and not once per spawn: the copy worth keeping is the file as it was
/// before amx ever wrote in it, and a run of agents in a repository nobody has
/// trusted would otherwise leave a copy of a large file behind for each one.
fn back_up(store: &Path, now: u64, exists: bool) -> Result<()> {
    if !exists || install::latest_backup(store)?.is_some() {
        return Ok(());
    }
    // The name `install` reads back, so both of amx's edits to the vendor's
    // files leave their copies the same way.
    let name = store.file_name().unwrap_or_default().to_string_lossy();
    let backup = store.with_file_name(format!("{name}.amx-backup-{now}"));
    std::fs::copy(store, &backup)
        .with_context(|| format!("copying {} to {}", store.display(), backup.display()))?;
    Ok(())
}

/// Read the store, telling "not there" from "not readable".
fn read(path: &Path) -> Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Some(json!({})));
    }
    let parsed = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a trust store amx can read", path.display()))?;
    Ok(Some(parsed))
}

/// Put the store back: readable by nobody else, and in one move.
///
/// The vendor reads this file whenever it likes, including from another
/// agent's pane, so it must never catch a half-written one.
fn write(path: &Path, store: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(store).context("writing the trust store")?;
    text.push('\n');

    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let part = path.with_file_name(format!("{name}.amx-{}", std::process::id()));
    match staged(&part, text.as_bytes()).and_then(|()| {
        std::fs::rename(&part, path).with_context(|| format!("putting {} in place", path.display()))
    }) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// The half of the write that happens beside the file rather than to it.
fn staged(part: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(part)
        .with_context(|| format!("creating {}", part.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", part.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A repository with a tree of amx's own in it, neither of them a real
    /// git repository: what this module knows about a tree is its shape.
    fn a_tree(dir: &TempDir) -> (PathBuf, PathBuf) {
        let repo = dir.path().join("app");
        let tree = repo.join(".amx/worktrees/fix-login-a1b");
        std::fs::create_dir_all(&tree).unwrap();
        (repo, tree)
    }

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn read_back(store: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(store).unwrap()).unwrap()
    }

    /// Somebody's own store, with two repositories and an account in it.
    fn a_persons_store() -> Value {
        json!({
            "oauthAccount": { "accountUuid": "9a1e" },
            "numStartups": 412,
            "projects": {
                "/src/other": {
                    "hasTrustDialogAccepted": true,
                    "lastCost": 12.5
                }
            }
        })
    }

    #[test]
    fn trust_knows_the_vendor_whose_screen_it_can_answer() {
        assert!(is_vendor("claude"));
        assert!(is_vendor("claude --verbose"));
        assert!(!is_vendor("mock-claude"));
        assert!(!is_vendor("codex"));
    }

    #[test]
    fn trust_follows_the_vendors_own_config_directory() {
        assert_eq!(
            store_in(&env(&[("HOME", "/home/dev")])),
            Some(PathBuf::from("/home/dev/.claude.json"))
        );
        assert_eq!(
            store_in(&env(&[("HOME", "/home/dev"), (CONFIG_DIR, "/cfg")])),
            Some(PathBuf::from("/cfg/.claude.json")),
            "the variable moves the whole file, not just a part of it"
        );
        assert_eq!(
            store_in(&env(&[("HOME", "/home/dev"), (CONFIG_DIR, "")])),
            Some(PathBuf::from("/home/dev/.claude.json")),
            "an empty variable reads as unset"
        );
        assert_eq!(
            store_in(&env(&[])),
            None,
            "and nowhere to look is not a path"
        );
    }

    #[test]
    fn trust_writes_the_entry_the_vendor_reads() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join("home/.claude.json");

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());

        let written = read_back(&store);
        assert_eq!(written[PROJECTS][key_for(&tree)][ACCEPTED], json!(true));
        assert!(trusted(&written, &tree));
        assert!(
            !trusted(&written, &repo),
            "the tree amx cut, and not the repository it sits in"
        );

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&store).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the person's own file, {mode:o}");
    }

    #[test]
    fn trust_is_only_ever_written_for_a_tree_amx_made() {
        let dir = TempDir::new().unwrap();
        let (repo, _) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        std::fs::write(&store, "{}\n").unwrap();

        for elsewhere in [
            repo.clone(),
            repo.join(".amx/worktrees"),
            dir.path().join("somebody-elses-checkout"),
        ] {
            let refused = seed(&store, &elsewhere, None, 1).unwrap_err();
            assert!(
                format!("{refused:#}").contains("not a tree amx made"),
                "{}: {refused:#}",
                elsewhere.display()
            );
        }
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            "{}\n",
            "and nothing was written on the way to refusing"
        );
    }

    #[test]
    fn trust_leaves_every_other_key_in_the_file_alone() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        // The tree already has an entry of the vendor's own making.
        before[PROJECTS][key_for(&tree)] = json!({ "lastSessionId": "2f7d", "allowedTools": [] });
        std::fs::write(&store, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());

        let after = read_back(&store);
        assert_eq!(after["oauthAccount"]["accountUuid"], "9a1e");
        assert_eq!(after["numStartups"], 412);
        assert_eq!(after[PROJECTS]["/src/other"]["lastCost"], 12.5);
        let entry = &after[PROJECTS][key_for(&tree)];
        assert_eq!(entry["lastSessionId"], "2f7d", "{entry}");
        assert_eq!(entry[ACCEPTED], json!(true), "{entry}");
    }

    #[test]
    fn trust_says_nothing_when_the_repository_already_covers_the_tree() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        before[PROJECTS][key_for(&repo)] = json!({ ACCEPTED: true });
        let bytes = serde_json::to_string_pretty(&before).unwrap();
        std::fs::write(&store, &bytes).unwrap();

        assert!(
            !seed(&store, &tree, Some(&repo), 1).unwrap(),
            "the vendor resolves the tree to the repository, and that is trusted"
        );
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            bytes,
            "so the file is not opened for writing at all"
        );
    }

    #[test]
    fn trust_takes_the_lock_the_vendor_takes_and_gives_it_back() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let lock = lock_beside(&store);

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());
        assert!(!lock.exists(), "the vendor is left free to write again");

        // Held by a claude that is saving its own session.
        std::fs::create_dir_all(&lock).unwrap();
        let second = repo.join(".amx/worktrees/port-importer-c3d");
        std::fs::create_dir_all(&second).unwrap();

        let refused = seed_within(&store, &second, Some(&repo), 2, Duration::ZERO).unwrap_err();
        assert!(
            format!("{refused:#}").contains("being written"),
            "{refused:#}"
        );
        assert!(
            !trusted(&read_back(&store), &second),
            "a write behind the vendor's back would lose one of the two"
        );
        assert!(lock.exists(), "and somebody else's lock is still theirs");
    }

    #[test]
    fn trust_sweeps_a_lock_nobody_is_holding_any_more() {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join(".claude.json");
        let lock = lock_beside(&store);
        std::fs::create_dir_all(&lock).unwrap();

        assert!(
            Held::take(&store, Duration::ZERO, Duration::from_secs(10))
                .unwrap()
                .is_none(),
            "a lock somebody refreshed a moment ago is somebody's"
        );
        let held = Held::take(&store, Duration::ZERO, Duration::ZERO).unwrap();
        assert!(
            held.is_some(),
            "and one nobody has touched was left behind by a claude that died"
        );

        drop(held);
        assert!(!lock.exists());
    }

    /// The lock a claude that died long ago left behind: present, and old.
    fn left_behind(lock: &Path) {
        std::fs::create_dir_all(lock).unwrap();
        let past = nix::sys::time::TimeSpec::new(1, 0);
        nix::sys::stat::utimensat(
            nix::fcntl::AT_FDCWD,
            lock,
            &past,
            &past,
            nix::sys::stat::UtimensatFlags::FollowSymlink,
        )
        .unwrap();
    }

    #[test]
    fn trust_two_sweeps_of_one_stale_lock_never_both_hold() {
        // A lock left by a dead claude, and two spawns arriving at once.
        // Judged stale and then swept are two acts, and the lock the sweep
        // met was sometimes the fresh one the other taker had just made —
        // after which both takers held, and two whole-document rewrites of
        // the store lost one writer's changes.
        let dir = TempDir::new().unwrap();
        let store = dir.path().join(".claude.json");
        let lock = lock_beside(&store);

        for round in 0..1000 {
            left_behind(&lock);
            let starting = std::sync::Barrier::new(2);
            let held: Vec<Option<Held>> = std::thread::scope(|racers| {
                let takers: Vec<_> = (0..2)
                    .map(|_| {
                        racers.spawn(|| {
                            starting.wait();
                            Held::take(&store, Duration::ZERO, STALE).unwrap()
                        })
                    })
                    .collect();
                takers
                    .into_iter()
                    .map(|taker| taker.join().unwrap())
                    .collect()
            });
            let holders = held.iter().filter(|held| held.is_some()).count();
            assert_eq!(holders, 1, "round {round}: one stale lock, one taker");
            drop(held);
            let _ = std::fs::remove_dir_all(&lock);
        }
    }

    #[test]
    fn trust_twice_is_trust_once() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());
        let after_first = std::fs::read_to_string(&store).unwrap();

        assert!(!seed(&store, &tree, Some(&repo), 2).unwrap());
        assert_eq!(std::fs::read_to_string(&store).unwrap(), after_first);
    }

    #[test]
    fn trust_refuses_a_store_it_cannot_read_rather_than_replacing_it() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");

        for broken in [
            "{ \"projects\": {},,, }",
            "{ \"projects\": [\"a list is not what the vendor writes\"] }",
        ] {
            std::fs::write(&store, broken).unwrap();
            let refused = seed(&store, &tree, Some(&repo), 1).unwrap_err();
            assert!(
                format!("{refused:#}").contains("trust store amx can read"),
                "{refused:#}"
            );
            assert_eq!(
                std::fs::read_to_string(&store).unwrap(),
                broken,
                "somebody's editing mistake is not amx's to overwrite"
            );
        }
    }

    #[test]
    fn trust_copies_the_file_aside_before_it_changes_anything_and_only_once() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        // Hand formatting a round trip would not reproduce.
        let before = "{\n    \"numStartups\":   412\n}\n";
        std::fs::write(&store, before).unwrap();

        assert!(seed(&store, &tree, Some(&repo), 1_700_000_000).unwrap());
        let copy = install::latest_backup(&store).unwrap().expect("a copy");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), before);

        let second = repo.join(".amx/worktrees/port-importer-c3d");
        std::fs::create_dir_all(&second).unwrap();
        assert!(seed(&store, &second, Some(&repo), 1_700_000_001).unwrap());
        assert_eq!(
            install::latest_backup(&store).unwrap(),
            Some(copy),
            "the copy worth keeping is the one from before amx ever wrote here"
        );
        let after = read_back(&store);
        assert!(trusted(&after, &tree) && trusted(&after, &second));
    }

    #[test]
    fn trust_creates_a_store_the_vendor_has_not_written_yet() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join("fresh/home/.claude.json");

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());
        assert!(trusted(&read_back(&store), &tree));
        assert_eq!(
            install::latest_backup(&store).unwrap(),
            None,
            "there was nothing to copy aside"
        );
    }

    #[test]
    fn trust_leaves_nothing_beside_the_store_it_wrote() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());
        let beside: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".claude.json."))
            .collect();
        assert!(
            beside.is_empty(),
            "a half-written store was left: {beside:?}"
        );
    }
}
