//! Answering a vendor's folder-trust screen for a linked worktree.
//!
//! A vendor asks once per new folder whether it is trusted, and draws that
//! question before any hook fires, so an agent that meets it waits until
//! somebody attaches. A linked worktree cut from a repository the person
//! already works in needs no decision, so amx answers for it. [`answers_for`]
//! names each vendor's answer:
//!
//! - [`Answer::Store`]: an entry in the vendor's own trust file, written by
//!   [`seed`] and kept after the run (claude).
//! - [`Answer::Flag`]: a flag on the vendor's argv that lasts one run and
//!   writes nothing (pi's `--approve`, added by `spawn`).
//!
//! Both need the config's `trust` key. The store is written only for a linked
//! worktree, whether amx cut it or something else did (`workflow run` passes
//! its own trees with `--no-worktree`). The repository itself and plain
//! directories are refused.
//!
//! The store, as of claude 2.1.237 (re-check on every vendor bump):
//!
//! - It is `$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`.
//! - The entry is `projects["<dir>"].hasTrustDialogAccepted: true`.
//! - A linked worktree inherits the trust of its repository, so [`seed`] writes
//!   nothing when the repository is already trusted.
//! - With an untrusted repository, a tree carrying a `.claude/settings.json`
//!   that pre-approves tools still gets a permissions prompt; only the
//!   repository's own entry silences it, and amx does not write that.
//!
//! Like `install`, edits keep a backup of the file as it was, carry foreign
//! keys through, and refuse a file that does not parse. Key order is lost in
//! the round trip; the vendor restores its own order on its next write.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::vendor::{Capability, Vendor};
use crate::{install, registry, worktree};

/// The vendor whose trust store amx writes.
const CLAUDE: &str = "claude";

/// The vendor whose trust screen amx answers with a flag.
const PI: &str = "pi";

/// pi's answering flag, and every spelling that already settles trust for a
/// run.
///
/// pi 0.84.4 and 0.85.1 document `--approve, -a` ("Trust project-local files
/// for this run") and `--no-approve, -na`. Any of the four overrides the
/// screen, and none of them writes pi's saved decisions in
/// `~/.pi/agent/trust.json`.
const APPROVE: &str = "--approve";
const AS_GOOD_AS: &[&str] = &[APPROVE, "-a", "--no-approve", "-na"];

/// How amx answers a vendor's folder-trust screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// An entry in the vendor's trust store, written by [`seed`].
    Store,
    /// A flag on the argv of the process amx starts.
    Flag(Flag),
}

/// A folder-trust screen answered on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    /// The flag amx adds.
    pub send: &'static str,
    /// Every spelling that already settles trust for the run, `send`
    /// included. When the argv carries one, amx adds nothing.
    pub settled: &'static [&'static str],
}

/// How long to wait for the vendor's lock, and how often to retry.
const PATIENCE: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_millis(20);

/// Age at which an untouched lock counts as abandoned; the vendor's own
/// threshold.
const STALE: Duration = Duration::from_secs(10);

/// The store's file name and the two keys that decide the screen.
const STORE: &str = ".claude.json";
const PROJECTS: &str = "projects";
const ACCEPTED: &str = "hasTrustDialogAccepted";

/// Moves the store to another directory.
const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// Whether amx can answer the trust screen of `agent`, by either answer.
pub fn is_vendor(agent: &str) -> bool {
    answer_for(agent).is_some()
}

/// Whether answering `agent`'s screen means writing the vendor's trust store.
pub fn writes_a_store(agent: &str) -> bool {
    matches!(answer_for(agent), Some(Answer::Store))
}

/// The argv flag that answers `agent`'s screen, if it is answered that way.
pub fn flag_for(agent: &str) -> Option<Flag> {
    match answer_for(agent)? {
        Answer::Flag(flag) => Some(flag),
        Answer::Store => None,
    }
}

/// How amx answers `vendor`'s folder-trust screen, or `None`.
///
/// The vendor must declare [`Capability::Trust`] and be one whose answer amx
/// knows; a test fails when a vendor claims the capability without one.
pub fn answers_for(vendor: &Vendor) -> Option<Answer> {
    if !vendor.can(Capability::Trust) {
        return None;
    }
    match vendor.name {
        CLAUDE => Some(Answer::Store),
        PI => Some(Answer::Flag(Flag {
            send: APPROVE,
            settled: AS_GOOD_AS,
        })),
        _ => None,
    }
}

fn answer_for(agent: &str) -> Option<Answer> {
    registry::entry(agent).and_then(answers_for)
}

/// The store path for an agent that will run with `env`.
///
/// Read from the agent's environment, since the agent reads the file. An empty
/// variable counts as unset.
pub fn store_in(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let set = |key: &str| env.get(key).filter(|value| !value.is_empty());
    set(CONFIG_DIR)
        .or_else(|| set("HOME"))
        .map(|dir| Path::new(dir).join(STORE))
}

/// The key the vendor looks `dir` up by: the path with symlinks resolved, or
/// `dir` as given when it does not resolve.
pub fn key_for(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Whether `store` already trusts `dir`.
pub fn trusted(store: &Value, dir: &Path) -> bool {
    store[PROJECTS][key_for(dir)][ACCEPTED] == Value::Bool(true)
}

/// Mark `dir` trusted, leaving every other key as it was. Returns whether
/// anything changed.
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

/// Answer the trust screen for `tree`. Returns whether the store was written.
///
/// `inherits` is the repository the tree belongs to; when it is already
/// trusted, the tree is covered and nothing is written.
pub fn seed(store: &Path, tree: &Path, inherits: Option<&Path>, now: u64) -> Result<bool> {
    seed_within(store, tree, inherits, now, PATIENCE)
}

fn seed_within(
    store: &Path,
    tree: &Path,
    inherits: Option<&Path>,
    now: u64,
    patience: Duration,
) -> Result<bool> {
    // An amx tree is known by its path; any other is asked of git.
    if !worktree::is_amx_tree(tree) && !worktree::is_linked(tree) {
        bail!(
            "{} is not a linked worktree, so its trust is not amx's to answer",
            tree.display()
        );
    }

    // Checked before taking the lock: usually the repository is trusted and
    // there is nothing to write.
    if covers(store, tree, inherits)? {
        return Ok(false);
    }

    let Some(held) = Held::take(store, patience, STALE)? else {
        bail!(
            "{} is being written by {CLAUDE}, so amx left it alone",
            store.display()
        );
    };

    // Re-read under the lock: the vendor may have rewritten it meanwhile.
    let existing = read(store)?;
    let mut document = existing.clone().unwrap_or_else(|| json!({}));
    if covered(&document, tree, inherits) {
        return Ok(false);
    }
    if !merge(&mut document, tree) {
        bail!("{} is not a trust store amx can read", store.display());
    }

    // A preempted sweep can hand this lock to another taker; writing on a lock
    // no longer held would lose that holder's changes.
    if !held.holds() {
        bail!(
            "{} is being written by {CLAUDE}, so amx left it alone",
            store.display()
        );
    }

    back_up(store, now, existing.is_some())?;
    write(store, &document)?;
    Ok(true)
}

/// Remove `tree`'s entry from the store. Returns whether there was one.
///
/// The vendor adds an entry for every directory it starts in, so without this
/// the store keeps a key for every tree amx ever cut. Only the tree's own key
/// is removed; the repository's entry is the person's.
pub fn forget_tree(store: &Path, tree: &Path, now: u64) -> Result<bool> {
    if !worktree::is_amx_tree(tree) {
        bail!(
            "{} is not a tree amx made, so its entry is not amx's to remove",
            tree.display()
        );
    }
    let key = key_for(tree);

    // Checked before taking the lock: usually there is nothing to remove.
    if !read(store)?.is_some_and(|looked| names(&looked, &key)) {
        return Ok(false);
    }

    let Some(held) = Held::take(store, PATIENCE, STALE)? else {
        bail!(
            "{} is being written by {CLAUDE}, so amx left it alone",
            store.display()
        );
    };

    // Re-read under the lock: the vendor may have rewritten it meanwhile.
    let Some(mut document) = read(store)? else {
        return Ok(false);
    };
    if !names(&document, &key) {
        return Ok(false);
    }
    document[PROJECTS]
        .as_object_mut()
        .expect("an object")
        .remove(&key);

    // A preempted sweep can hand this lock to another taker; writing on a lock
    // no longer held would lose that holder's changes.
    if !held.holds() {
        bail!(
            "{} is being written by {CLAUDE}, so amx left it alone",
            store.display()
        );
    }

    back_up(store, now, true)?;
    write(store, &document)?;
    Ok(true)
}

/// The amx trees the store still names that no longer exist on disk, sorted.
///
/// Only keys shaped like an amx tree count. A missing store names nothing; an
/// unreadable one is an error.
pub fn stale_trees(store: &Path) -> Result<Vec<PathBuf>> {
    let Some(document) = read(store)? else {
        return Ok(Vec::new());
    };
    let Some(projects) = document[PROJECTS].as_object() else {
        return Ok(Vec::new());
    };
    let mut gone: Vec<PathBuf> = projects
        .keys()
        .map(PathBuf::from)
        .filter(|key| worktree::is_amx_tree(key) && !key.exists())
        .collect();
    gone.sort();
    Ok(gone)
}

/// Whether `store` has a project entry under `key`.
fn names(store: &Value, key: &str) -> bool {
    store[PROJECTS]
        .as_object()
        .is_some_and(|projects| projects.contains_key(key))
}

/// Whether the store trusts `dir`, directly or through the repository
/// `inherits` names.
///
/// Read-only: takes no lock and creates no file. A missing store covers
/// nothing; an unreadable one is an error.
pub fn covers(store: &Path, dir: &Path, inherits: Option<&Path>) -> Result<bool> {
    Ok(covered(
        &read(store)?.unwrap_or_else(|| json!({})),
        dir,
        inherits,
    ))
}

fn covered(store: &Value, tree: &Path, inherits: Option<&Path>) -> bool {
    trusted(store, tree) || inherits.is_some_and(|repo| trusted(store, repo))
}

/// The vendor's write lock on the store, taken the way the vendor takes it.
///
/// claude guards each write with a `<store>.lock` directory: whoever creates
/// it holds it, and one untouched for [`STALE`] was left by a claude that
/// died. amx takes the same lock, since another claude rewrites the whole
/// document when it saves.
struct Held {
    path: PathBuf,
    token: String,
}

/// The file in the lock directory naming its holder. The vendor only checks
/// that the directory exists.
const OWNER: &str = "owner";

/// The outcome of marking a lock directory.
enum Marked {
    /// The marker landed; the lock is this taker's.
    Held(Held),
    /// No directory at the path: a sweep has it aside and will put back a live
    /// one.
    Aside,
    /// The directory already has a marker, or would not take one.
    Theirs,
}

impl Held {
    /// Take the lock, or `None` when someone else holds it past `patience`.
    fn take(store: &Path, patience: Duration, stale: Duration) -> Result<Option<Held>> {
        let path = lock_beside(store);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let waiting = Instant::now();
        // Set once this taker creates the directory and until a marker lands.
        // Giving up then would leave a lock at the vendor's path that nobody
        // holds and only the stale rule can clear.
        let mut unnamed = false;
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => unnamed = true,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e).with_context(|| format!("taking {}", path.display())),
            }
            if unnamed {
                match Held::mark(&path) {
                    Marked::Held(held) => return Ok(Some(held)),
                    // A sweep has our directory aside and will put it back;
                    // retry at once without counting the wait.
                    Marked::Aside => continue,
                    // A preempted sweep restored its caught lock over our
                    // create. That holder owns the path; wait like any taker.
                    Marked::Theirs => unnamed = false,
                }
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

    /// Mark the directory at `path` as this taker's.
    ///
    /// The marker is created with `create_new`, so when a sweep's putback races
    /// this taker's create, exactly one directory ends up marked.
    fn mark(path: &Path) -> Marked {
        // Unique per process by pid and within it by count.
        static TAKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nth = TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let token = format!("{}-{nth}", std::process::id());

        let marker = path.join(OWNER);
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker);
        let mut file = match opened {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Marked::Aside,
            Err(_) => return Marked::Theirs,
        };
        if file.write_all(token.as_bytes()).is_err() {
            // A partial marker would block every later taker until the stale
            // sweep; remove it and stand aside.
            drop(file);
            let _ = std::fs::remove_file(&marker);
            return Marked::Theirs;
        }
        Marked::Held(Held {
            path: path.to_path_buf(),
            token,
        })
    }

    /// Whether the lock is still this taker's. A preempted sweep can concede a
    /// live lock to a third taker; the marker shows that before the write.
    fn holds(&self) -> bool {
        std::fs::read_to_string(self.path.join(OWNER)).is_ok_and(|named| named == self.token)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // Release only a lock this taker still holds.
        if self.holds() {
            let _ = std::fs::remove_file(self.path.join(OWNER));
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

/// Remove a lock that looks abandoned without ever removing a live one.
///
/// Between judging a lock stale and removing it, another taker can sweep it
/// and create a fresh one. So the lock is first renamed aside to a name unique
/// to this sweep and judged there: an abandoned one is deleted, a live one is
/// renamed back for the taker waiting to mark it.
fn sweep(path: &Path, stale: Duration) {
    // Unique per process by pid and within it by count.
    static SWEPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nth = SWEPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let aside = path.with_file_name(format!("{name}.stale-{}-{nth}", std::process::id()));
    if std::fs::rename(path, &aside).is_err() {
        // Someone else swept or released it first.
        return;
    }
    if !abandoned(&aside, stale) && std::fs::rename(&aside, path).is_ok() {
        return;
    }
    // Abandoned, or a putback that lost the path to a newer create; either way
    // what is aside is nobody's.
    let _ = std::fs::remove_dir_all(&aside);
}

/// The vendor's lock path for `store`.
fn lock_beside(store: &Path) -> PathBuf {
    let name = store.file_name().unwrap_or_default().to_string_lossy();
    store.with_file_name(format!("{name}.lock"))
}

/// Whether `lock` has gone unmodified for `stale`. The vendor touches its lock
/// while it holds it, so age is enough.
fn abandoned(lock: &Path, stale: Duration) -> bool {
    std::fs::metadata(lock)
        .and_then(|found| found.modified())
        .is_ok_and(|touched| {
            SystemTime::now()
                .duration_since(touched)
                .is_ok_and(|since| since >= stale)
        })
}

/// Whether `store` has the vendor's shape as far as `dir`'s entry: an object
/// whose `projects` and entry, where present, are objects.
fn readable(store: &Value, dir: &Path) -> bool {
    store.is_object()
        && store.get(PROJECTS).is_none_or(Value::is_object)
        && store[PROJECTS]
            .get(key_for(dir))
            .is_none_or(Value::is_object)
}

/// Copy the store aside before the first change amx ever makes to it.
///
/// One backup in total, since the copy worth keeping is the file before amx
/// touched it.
fn back_up(store: &Path, now: u64, exists: bool) -> Result<()> {
    if !exists || install::latest_backup(store)?.is_some() {
        return Ok(());
    }
    // The backup name `install` reads back.
    let name = store.file_name().unwrap_or_default().to_string_lossy();
    let backup = store.with_file_name(format!("{name}.amx-backup-{now}"));
    std::fs::copy(store, &backup)
        .with_context(|| format!("copying {} to {}", store.display(), backup.display()))?;
    Ok(())
}

/// Read the store: `None` when missing, an error when unreadable. An empty
/// file reads as `{}`.
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

/// Write the store atomically with owner-only permissions.
///
/// The vendor may read it at any moment, so it must never see a partial file.
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

/// Write the temporary file beside the store.
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
    use crate::vendor::second::SECOND;
    use tempfile::TempDir;

    /// A repository path with an amx tree under it; neither is a git
    /// repository.
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

    /// Run git with no user or system config and a fixed identity.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "amx tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "amx tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A person's store, with an account and another project in it.
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
    fn trust_knows_the_vendors_whose_screen_it_can_answer() {
        assert!(is_vendor("claude"));
        assert!(is_vendor("claude --verbose"));
        assert!(is_vendor("pi"), "answered on the argv, but answered");
        assert!(!is_vendor("mock-claude"));
        assert!(!is_vendor("codex"));
    }

    #[test]
    fn trust_tells_the_store_write_apart_from_the_screen_it_can_answer() {
        // `new` asks whether to write claude's file; `doctor` asks whether amx
        // can answer the screen at all. pi answers yes to the second only.
        assert!(writes_a_store("claude") && is_vendor("claude"));
        assert!(!writes_a_store("pi") && is_vendor("pi"));
        assert!(!writes_a_store("mock-claude") && !is_vendor("mock-claude"));

        assert_eq!(flag_for("claude"), None, "claude's answer is the store");
        assert_eq!(flag_for("pi").map(|flag| flag.send), Some("--approve"));
        assert_eq!(flag_for("mock-claude"), None);
    }

    #[test]
    fn trust_answers_for_a_vendor_that_says_it_has_a_screen_and_no_other() {
        // The capability decides, not the vendor's name.
        let measured = registry::entry(CLAUDE).expect("the vendor amx measured");
        assert_eq!(answers_for(measured), Some(Answer::Store));
        assert_eq!(
            answers_for(&Vendor {
                capabilities: &[Capability::Hooks],
                ..*measured
            }),
            None
        );
        assert_eq!(answers_for(&SECOND), None);
    }

    #[test]
    fn trust_names_an_answer_for_every_vendor_that_claims_the_screen() {
        // A vendor that claims the capability needs a known answer, or its
        // agents sit on the screen. This fails when one is added without one.
        let answered: Vec<(&str, Option<Answer>)> = registry::entries()
            .iter()
            .filter(|vendor| vendor.can(Capability::Trust))
            .map(|vendor| (vendor.name, answers_for(vendor)))
            .collect();
        assert_eq!(
            answered,
            [
                ("claude", Some(Answer::Store)),
                (
                    "pi",
                    Some(Answer::Flag(Flag {
                        send: "--approve",
                        settled: &["--approve", "-a", "--no-approve", "-na"],
                    }))
                ),
            ]
        );
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
    fn trust_is_never_written_for_a_directory_derived_from_nothing() {
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
                format!("{refused:#}").contains("not a linked worktree"),
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
    fn trust_says_whether_a_directory_is_covered_without_writing_anything() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");

        assert!(
            !covers(&store, &tree, Some(&repo)).unwrap(),
            "a store the vendor has not written yet covers nothing"
        );
        assert!(!store.exists(), "and was not made on the way to saying so");

        let mut theirs = a_persons_store();
        theirs[PROJECTS][key_for(&repo)] = json!({ ACCEPTED: true });
        let bytes = serde_json::to_string_pretty(&theirs).unwrap();
        std::fs::write(&store, &bytes).unwrap();

        assert!(
            covers(&store, &tree, Some(&repo)).unwrap(),
            "by the repository's entry"
        );
        assert!(!covers(&store, &tree, None).unwrap(), "and by nothing else");
        assert!(covers(&store, &repo, None).unwrap());
        assert_eq!(std::fs::read_to_string(&store).unwrap(), bytes);
        assert!(!lock_beside(&store).exists(), "a look takes no lock");

        std::fs::write(&store, "{ not json at all }").unwrap();
        assert!(
            covers(&store, &repo, None).is_err(),
            "a store amx cannot read is not a no"
        );
    }

    #[test]
    fn trust_is_written_for_a_linked_worktree_somebody_else_cut() {
        // Any linked worktree qualifies, whoever cut it (`workflow run` cuts
        // its own). The checkout it belongs to is still refused.
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let theirs = dir.path().join("workflow/plan/t1");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                &theirs.to_string_lossy(),
                "-b",
                "t1",
            ],
        );
        let store = dir.path().join(".claude.json");
        std::fs::write(&store, "{}\n").unwrap();

        assert!(seed(&store, &theirs, Some(&repo), 1).unwrap());
        let written = read_back(&store);
        assert!(trusted(&written, &theirs), "{written}");
        assert!(!trusted(&written, &repo), "{written}");

        let refused = seed(&store, &repo, None, 1).unwrap_err();
        assert!(
            format!("{refused:#}").contains("not a linked worktree"),
            "{refused:#}"
        );
    }

    #[test]
    fn trust_leaves_every_other_key_in_the_file_alone() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        // The tree already has an entry the vendor wrote.
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
    fn trust_forgets_the_tree_and_leaves_every_other_key_alone() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        before[PROJECTS][key_for(&repo)] = json!({ ACCEPTED: true });
        before[PROJECTS][key_for(&tree)] = json!({ ACCEPTED: true, "lastSessionId": "2f7d" });
        std::fs::write(&store, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        assert!(forget_tree(&store, &tree, 1).unwrap());

        let after = read_back(&store);
        assert_eq!(after[PROJECTS].get(key_for(&tree)), None, "{after}");
        assert!(
            trusted(&after, &repo),
            "the repository's entry is the person's consent, not amx's: {after}"
        );
        assert_eq!(after[PROJECTS]["/src/other"]["lastCost"], 12.5);
        assert_eq!(after["numStartups"], 412);
        assert_eq!(after["oauthAccount"]["accountUuid"], "9a1e");
    }

    #[test]
    fn trust_copies_the_file_aside_before_it_forgets_anything() {
        let dir = TempDir::new().unwrap();
        let (_, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        before[PROJECTS][key_for(&tree)] = json!({ ACCEPTED: true });
        let bytes = serde_json::to_string_pretty(&before).unwrap();
        std::fs::write(&store, &bytes).unwrap();

        assert!(forget_tree(&store, &tree, 1_700_000_000).unwrap());

        let copy = install::latest_backup(&store).unwrap().expect("a copy");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), bytes);
        assert!(
            trusted(&read_back(&copy), &tree),
            "the file as it was, key and all"
        );
    }

    #[test]
    fn trust_is_only_ever_forgotten_for_a_tree_amx_made() {
        let dir = TempDir::new().unwrap();
        let (repo, _) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        before[PROJECTS][key_for(&repo)] = json!({ ACCEPTED: true });
        let bytes = serde_json::to_string_pretty(&before).unwrap();
        std::fs::write(&store, &bytes).unwrap();

        let refused = forget_tree(&store, &repo, 1).unwrap_err();
        assert!(
            format!("{refused:#}").contains("not a tree amx made"),
            "{refused:#}"
        );
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            bytes,
            "and nothing was written on the way to refusing"
        );
    }

    #[test]
    fn trust_forgets_nothing_where_the_store_never_named_the_tree() {
        let dir = TempDir::new().unwrap();
        let (_, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let bytes = serde_json::to_string_pretty(&a_persons_store()).unwrap();
        std::fs::write(&store, &bytes).unwrap();

        assert!(!forget_tree(&store, &tree, 1).unwrap());
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            bytes,
            "a file with nothing of amx's in it is not opened for writing"
        );
        assert_eq!(install::latest_backup(&store).unwrap(), None);

        let never_written = dir.path().join("fresh/home/.claude.json");
        assert!(!forget_tree(&never_written, &tree, 1).unwrap());
        assert!(!never_written.exists(), "nor made");
    }

    #[test]
    fn trust_lists_the_trees_the_store_names_and_the_disk_has_not_got() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let gone = repo.join(".amx/worktrees/port-importer-c3d");
        let older = repo.join(".amx/worktrees/fix-auth-b2c");
        let mut before = a_persons_store();
        before[PROJECTS][key_for(&repo)] = json!({ ACCEPTED: true });
        // Still on disk, so its agent may be running.
        before[PROJECTS][key_for(&tree)] = json!({ ACCEPTED: true });
        for stopped in [&gone, &older] {
            before[PROJECTS][stopped.to_string_lossy().into_owned()] = json!({ ACCEPTED: true });
        }
        std::fs::write(&store, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        assert_eq!(
            stale_trees(&store).unwrap(),
            vec![older, gone],
            "the trees amx cut and nothing else, sorted"
        );
    }

    #[test]
    fn trust_finds_nothing_stale_in_a_store_that_is_not_there_and_refuses_one_it_cannot_read() {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join(".claude.json");
        assert_eq!(stale_trees(&store).unwrap(), Vec::<PathBuf>::new());

        std::fs::write(
            &store,
            serde_json::to_string_pretty(&a_persons_store()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            stale_trees(&store).unwrap(),
            Vec::<PathBuf>::new(),
            "a store with none of amx's keys in it"
        );

        std::fs::write(&store, "{ \"projects\": {},,, }").unwrap();
        let refused = stale_trees(&store).unwrap_err();
        assert!(
            format!("{refused:#}").contains("trust store amx can read"),
            "a file amx cannot read is not a file with nothing to prune: {refused:#}"
        );
    }

    #[test]
    fn trust_leaves_a_store_the_vendor_is_writing_alone_when_it_forgets() {
        let dir = TempDir::new().unwrap();
        let (_, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let mut before = a_persons_store();
        before[PROJECTS][key_for(&tree)] = json!({ ACCEPTED: true });
        std::fs::write(&store, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        // A claude holding the lock while it saves.
        let lock = lock_beside(&store);
        std::fs::create_dir_all(&lock).unwrap();

        let refused = forget_tree(&store, &tree, 1).unwrap_err();
        assert!(
            format!("{refused:#}").contains("being written"),
            "{refused:#}"
        );
        assert!(
            trusted(&read_back(&store), &tree),
            "a write behind the vendor's back would lose one of the two"
        );
        assert!(lock.exists(), "and somebody else's lock is still theirs");
    }

    #[test]
    fn trust_takes_the_lock_the_vendor_takes_and_gives_it_back() {
        let dir = TempDir::new().unwrap();
        let (repo, tree) = a_tree(&dir);
        let store = dir.path().join(".claude.json");
        let lock = lock_beside(&store);

        assert!(seed(&store, &tree, Some(&repo), 1).unwrap());
        assert!(!lock.exists(), "the vendor is left free to write again");

        // A claude holding the lock while it saves.
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

    /// Create `lock` with an old timestamp, as a claude that died leaves it.
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
        // A stale lock and two takers at once. Judging a lock stale and
        // sweeping it are separate steps, and a sweep could remove the fresh
        // lock the other taker had just made, leaving both holding it.
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
    fn trust_a_sweep_preempted_over_a_live_lock_never_makes_two_holders() {
        // B holds a fresh lock. A judged the lock B replaced abandoned, was
        // preempted, and now sweeps B's live lock (rename aside, check, put
        // back). C spins on take. C's create can land while B's lock is aside,
        // and A's putback then replaces C's directory. Without the owner
        // marker both B and C would believe they hold the lock.
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = TempDir::new().unwrap();
        let store = dir.path().join(".claude.json");
        let lock = lock_beside(&store);

        let b = Held::take(&store, Duration::ZERO, STALE).unwrap().unwrap();

        let landed = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let c = std::thread::scope(|racers| {
            let taker = racers.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    if let Some(held) = Held::take(&store, Duration::ZERO, STALE).unwrap() {
                        landed.store(true, Ordering::Relaxed);
                        return Some(held);
                    }
                }
                None
            });
            let patience = Instant::now();
            while !landed.load(Ordering::Relaxed) && patience.elapsed() < Duration::from_secs(10) {
                sweep(&lock, STALE);
            }
            stop.store(true, Ordering::Relaxed);
            taker.join().unwrap()
        });

        let c = c.expect("ten seconds of sweeps and C never met the aside window");
        assert!(c.holds(), "the lock C took is C's");
        assert!(
            !b.holds(),
            "and B must find that out before it writes the store"
        );
    }

    #[test]
    fn trust_a_marker_that_cannot_land_says_which_of_the_two_it_met() {
        // A marker can fail to land because a sweep has this taker's fresh
        // directory aside, or because someone else marked it. Reading the
        // first as the second would abandon a directory only this taker can
        // mark, and its fresh timestamp keeps the stale rule off it for 10 s.
        let dir = TempDir::new().unwrap();
        let store = dir.path().join(".claude.json");
        let lock = lock_beside(&store);
        let aside = lock.with_file_name(".claude.json.lock.swept");

        // The directory created, and a sweep holding it aside.
        std::fs::create_dir_all(&lock).unwrap();
        std::fs::rename(&lock, &aside).unwrap();
        assert!(
            matches!(Held::mark(&lock), Marked::Aside),
            "nothing at the path is a sweep holding a directory, not a lock \
             somebody else took"
        );

        // Put back, and still this taker's to mark.
        std::fs::rename(&aside, &lock).unwrap();
        let held = Held::mark(&lock);
        assert!(
            matches!(held, Marked::Held(_)),
            "a directory nobody has named is there to be named"
        );

        // Once marked, the directory is its taker's.
        assert!(
            matches!(Held::mark(&lock), Marked::Theirs),
            "the path is whoever's marker is in it"
        );
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
