//! The pull request on an agent's branch and its status, asked of `gh` or
//! `glab`.
//!
//! A machine with neither installed gets no pull requests, never an error.
//!
//! - Interactive readers never wait on a forge: they read the answer cached in
//!   `pr.json` beside the record, and the view has [`refresh`] ask about every
//!   stale branch of a repository at once, on a detached thread.
//! - Only [`asked_now`] (for `sweep`) and [`request_head`] (for `new --pr`)
//!   call the forge synchronously.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::store::Meta;

/// Seconds a cached forge answer stays fresh.
pub const FRESH: u64 = 60;

/// The cache file, in the agent's record directory.
const CACHE: &str = "pr.json";

/// Requests one answer about a whole repository carries. A branch a full
/// answer leaves out may have a request beyond it, so it is asked about alone.
const LIMIT: usize = 100;

/// The longest gap between asks about a repository whose forge keeps failing.
const LONGEST: u64 = 16 * 60;

/// Seconds an ended agent's branch with no request goes between asks: a
/// request opened after the work ended is rare, and nothing is waiting on it.
const ENDED: u64 = 10 * 60;

/// The schedule of asks every view shares, beside the agents directory.
pub(crate) const SCHEDULE: &str = "forge.json";

/// One pull request, as much of it as a row shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub number: u64,
    pub standing: Standing,
}

impl Pr {
    /// The row label, `#<number>`.
    pub fn label(&self) -> String {
        format!("#{}", self.number)
    }
}

/// A pull request's status, reduced to one word; [`fold`] sets the precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Standing {
    Merged,
    /// Closed without merging.
    Closed,
    Draft,
    /// A check failed.
    Failing,
    /// A reviewer requested changes.
    Changes,
    /// Checks are still running.
    Running,
    /// Approved, with nothing failing.
    Ready,
    /// Open and awaiting review.
    Open,
}

impl Standing {
    /// The card's word for it. Several standings share a row colour, so the
    /// card names which one it is.
    pub fn says(self) -> &'static str {
        match self {
            Standing::Merged => "merged",
            Standing::Closed => "closed",
            Standing::Draft => "draft",
            Standing::Failing => "checks failing",
            Standing::Changes => "changes requested",
            Standing::Running => "checks running",
            Standing::Ready => "approved",
            Standing::Open => "open",
        }
    }

    /// Whether it is merged or closed.
    pub fn settled(self) -> bool {
        matches!(self, Standing::Merged | Standing::Closed)
    }
}

/// The combined result of a request's checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Checks {
    Passing,
    Failing,
    Running,
}

/// A request's standing from the forge's fields.
///
/// Precedence: merged or closed, then draft, then failing checks, then
/// requested changes, then running checks, then approval.
fn fold(state: &str, draft: bool, review: &str, checks: Option<Checks>) -> Standing {
    if state.eq_ignore_ascii_case("merged") {
        return Standing::Merged;
    }
    if state.eq_ignore_ascii_case("closed") || state.eq_ignore_ascii_case("locked") {
        return Standing::Closed;
    }
    if draft {
        return Standing::Draft;
    }
    if checks == Some(Checks::Failing) {
        return Standing::Failing;
    }
    if review.eq_ignore_ascii_case("changes_requested") {
        return Standing::Changes;
    }
    if checks == Some(Checks::Running) {
        return Standing::Running;
    }
    if review.eq_ignore_ascii_case("approved") {
        return Standing::Ready;
    }
    Standing::Open
}

/// Open requests first, then settled ones, each newest first.
fn sorted(mut prs: Vec<Pr>) -> Vec<Pr> {
    prs.sort_by_key(|pr| (pr.standing.settled(), std::cmp::Reverse(pr.number)));
    prs
}

/// The fields requested from `gh pr list --json`.
const GH_FIELDS: &str = "number,state,isDraft,reviewDecision,statusCheckRollup,headRefOid";

/// Parse `gh pr list --json` output.
///
/// As of gh 2.97.0: `state` is `OPEN`, `CLOSED` or `MERGED`; `reviewDecision`
/// is `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED` or empty;
/// `statusCheckRollup` mixes `CheckRun` entries (`status` and `conclusion`)
/// with `StatusContext` entries (verdict in `state`). Fields are read from a
/// `Value` so a renamed field loses only its own answer.
fn read_gh(said: &str) -> Vec<Pr> {
    let Ok(serde_json::Value::Array(listed)) = serde_json::from_str(said) else {
        return Vec::new();
    };
    listed
        .iter()
        .filter_map(|pr| {
            let number = pr.get("number")?.as_u64()?;
            let checks = rollup(pr.get("statusCheckRollup").and_then(|it| it.as_array()));
            Some(Pr {
                number,
                standing: fold(
                    word(pr, "state"),
                    pr.get("isDraft")
                        .and_then(|it| it.as_bool())
                        .unwrap_or(false),
                    word(pr, "reviewDecision"),
                    checks,
                ),
            })
        })
        .collect()
}

/// Parse `glab mr list --output json` output.
///
/// Written from glab's documentation, not a live run, so every field is
/// optional. `state` is `opened`, `merged`, `closed` or `locked`; a draft is
/// `draft` (newer glab) or `work_in_progress` (older); checks come from the
/// head pipeline's status. The listing has no review decision, so a merge
/// request is never `Ready` or `Changes`.
fn read_glab(said: &str) -> Vec<Pr> {
    let Ok(serde_json::Value::Array(listed)) = serde_json::from_str(said) else {
        return Vec::new();
    };
    listed
        .iter()
        .filter_map(|mr| {
            let number = mr.get("iid")?.as_u64()?;
            let draft = ["draft", "work_in_progress"]
                .iter()
                .any(|key| mr.get(key).and_then(|it| it.as_bool()).unwrap_or(false));
            let pipeline = mr
                .get("head_pipeline")
                .map(|head| word(head, "status"))
                .unwrap_or_default();
            Some(Pr {
                number,
                standing: fold(word(mr, "state"), draft, "", pipeline_of(pipeline)),
            })
        })
        .collect()
}

/// The head commit of each merged request (`headRefOid` for gh, `sha` for
/// glab).
///
/// A branch whose tip equals a merged head has landed even when a squash or
/// rebase merge left its commits on no other branch.
fn merged_heads(said: &str, head: &str) -> Vec<String> {
    let Ok(serde_json::Value::Array(listed)) = serde_json::from_str(said) else {
        return Vec::new();
    };
    listed
        .iter()
        .filter(|pr| word(pr, "state").eq_ignore_ascii_case("merged"))
        .filter_map(|pr| Some(pr.get(head)?.as_str()?.to_string()))
        .collect()
}

/// A string field, or `""` when absent.
fn word<'a>(object: &'a serde_json::Value, key: &str) -> &'a str {
    object.get(key).and_then(|it| it.as_str()).unwrap_or("")
}

/// Combine a request's checks: any failure is `Failing`, any unfinished check
/// is `Running`. No checks at all is `None`, not a pass.
fn rollup(entries: Option<&Vec<serde_json::Value>>) -> Option<Checks> {
    let entries = entries?;
    if entries.is_empty() {
        return None;
    }

    let mut running = false;
    for entry in entries {
        // A CheckRun's verdict is `conclusion` (empty until done); a
        // StatusContext has only `state`.
        let went = match word(entry, "conclusion") {
            "" => word(entry, "state"),
            conclusion => conclusion,
        };
        let done = word(entry, "status");
        if FAILED.iter().any(|bad| went.eq_ignore_ascii_case(bad)) {
            return Some(Checks::Failing);
        }
        let passed = PASSED.iter().any(|good| went.eq_ignore_ascii_case(good));
        // A completed check with an unknown verdict counts as neither.
        running |= !passed && !done.eq_ignore_ascii_case("completed");
    }
    Some(match running {
        true => Checks::Running,
        false => Checks::Passing,
    })
}

/// Check verdicts that count as a failure.
const FAILED: [&str; 7] = [
    "FAILURE",
    "TIMED_OUT",
    "CANCELLED",
    "ACTION_REQUIRED",
    "STARTUP_FAILURE",
    "STALE",
    "ERROR",
];

/// Check verdicts that count as passing, skipped and neutral included.
const PASSED: [&str; 3] = ["SUCCESS", "NEUTRAL", "SKIPPED"];

/// A GitLab pipeline status as [`Checks`].
fn pipeline_of(status: &str) -> Option<Checks> {
    match status.to_ascii_lowercase().as_str() {
        "" => None,
        "failed" | "canceled" | "cancelled" => Some(Checks::Failing),
        "success" | "skipped" | "manual" => Some(Checks::Passing),
        _ => Some(Checks::Running),
    }
}

/// The cached pull requests, with no refresh.
///
/// What every reader shows. The view keeps the cache current through
/// [`refresh`]; a verb that prints and exits would be gone before a forge
/// answered.
pub fn written(meta: &Meta) -> Vec<Pr> {
    let Some((dir, _, branch)) = about(meta) else {
        return Vec::new();
    };
    kept(&dir, branch)
}

/// The record directory, the directory to run the forge in, and the branch.
///
/// `None` for an agent with no branch of amx's (the person's checkout is not
/// its work) or when the record or both directories are gone.
fn about(meta: &Meta) -> Option<(PathBuf, PathBuf, &str)> {
    let branch = meta.branch.as_deref()?;
    let dir = crate::paths::agent_dir(&meta.id).ok()?;
    let at = meta.workdir();
    (dir.is_dir() && at.is_dir()).then(|| (dir, at.to_path_buf(), branch))
}

/// Ask about the branches of `agents` whose written-down requests are stale,
/// one ask per repository when its turn on the schedule has come, on
/// detached threads, so the next reading shows the answers. Never waits on a
/// forge. Each agent comes with whether it has ended.
///
/// What is written down stays shown while an ask runs, so the column does
/// not blink.
pub fn refresh(agents: &[(&Meta, bool)], now: u64) {
    let Some(schedule) = crate::paths::state_root()
        .ok()
        .and_then(|root| crate::paths::schedule_file(&root))
    else {
        return;
    };
    let agents = agents
        .iter()
        .filter_map(|(meta, ended)| asked_about(meta, *ended))
        .collect();
    refresh_with(agents, now, &Forges::installed(), &schedule);
}

/// [`refresh`] over agents already found, each with its repository, with
/// the forge programs and the schedule file given.
fn refresh_with(agents: Vec<(PathBuf, Asked)>, now: u64, forges: &Forges, schedule: &Path) {
    for (repo, agents) in stale(agents, now) {
        ask_again(repo, agents, now, forges.clone(), schedule.to_path_buf());
    }
}

/// An agent whose answer a repository's ask writes down.
#[derive(Debug, Clone)]
struct Asked {
    /// The record directory, where `pr.json` goes.
    dir: PathBuf,
    branch: String,
    /// Whether the agent has ended; see [`ENDED`].
    ended: bool,
}

/// The agent's repository and where its answer goes, for an agent on a
/// branch of amx's whose repository is still there.
fn asked_about(meta: &Meta, ended: bool) -> Option<(PathBuf, Asked)> {
    let (dir, _, branch) = about(meta)?;
    let repo = repository(meta);
    repo.is_dir().then(|| {
        let branch = branch.to_string();
        (repo, Asked { dir, branch, ended })
    })
}

/// The agents whose written-down requests need asking about again, by
/// repository, in the order first met.
fn stale(agents: Vec<(PathBuf, Asked)>, now: u64) -> Vec<(PathBuf, Vec<Asked>)> {
    let mut by_repository: Vec<(PathBuf, Vec<Asked>)> = Vec::new();
    for (repo, asked) in agents {
        if !worth_asking(held(&asked.dir).as_ref(), &asked.branch, asked.ended, now) {
            continue;
        }
        match by_repository.iter_mut().find(|(known, _)| *known == repo) {
            Some((_, agents)) => agents.push(asked),
            None => by_repository.push((repo, vec![asked])),
        }
    }
    by_repository
}

/// Whether what is written down about `branch` is worth asking about again:
/// it is not [`still_good`], and an ended agent's branch with no request was
/// last asked about [`ENDED`] seconds ago or more.
fn worth_asking(held: Option<&Recorded>, branch: &str, ended: bool, now: u64) -> bool {
    if still_good(held, branch, now) {
        return false;
    }
    match held {
        Some(held) if ended && held.branch == branch && held.prs.is_empty() => {
            now.saturating_sub(held.asked) >= ENDED
        }
        _ => true,
    }
}

/// The repository an agent's tree was cut from, read off the tree's path; its
/// directory for an agent amx cut no tree for.
fn repository(meta: &Meta) -> PathBuf {
    meta.worktree
        .as_deref()
        .and_then(crate::worktree::repo_of)
        .unwrap_or_else(|| meta.dir.clone())
}

/// The pull requests on this agent's branch, asking the forge synchronously
/// when the cache is stale.
///
/// For `sweep`, which decides whether to remove an agent on this answer and
/// cannot rely on a cache only the view fills. Settled requests are not asked
/// about again.
pub fn asked_now(meta: &Meta) -> Vec<Pr> {
    let Some((dir, at, branch)) = about(meta) else {
        return Vec::new();
    };
    ask_now(&dir, &at, branch, crate::store::now(), &Forges::installed())
}

/// [`asked_now`] with the record directory and the working directory given.
fn ask_now(dir: &Path, at: &Path, branch: &str, now: u64, forges: &Forges) -> Vec<Pr> {
    let held = held(dir);
    // Asked by name, a `pr-<N>` branch reads as having no request, since its
    // head is a fork's name; what the view found by number stands.
    if still_good(held.as_ref(), branch, now) || numbered(branch).is_some() {
        return theirs(held, branch);
    }
    let Some(looked) = ask_with(at, branch, forges) else {
        return theirs(held, branch);
    };
    let _ = write(dir, branch, looked.prs.clone(), &looked.merged_heads, now);
    looked.prs
}

/// The cached [`merged_heads`] for this agent's branch, however old.
///
/// Needs only the record, since it is read when the tree may already be gone.
pub fn merged_heads_written(meta: &Meta) -> Vec<String> {
    let (Some(branch), Ok(dir)) = (meta.branch.as_deref(), crate::paths::agent_dir(&meta.id))
    else {
        return Vec::new();
    };
    held(&dir)
        .filter(|held| held.branch == branch)
        .map(|held| held.merged_heads)
        .unwrap_or_default()
}

/// The cached pull requests for `branch`, however old.
pub fn kept(dir: &Path, branch: &str) -> Vec<Pr> {
    theirs(held(dir), branch)
}

/// The cached requests when the cache is for `branch` (a rename invalidates
/// it).
fn theirs(held: Option<Recorded>, branch: &str) -> Vec<Pr> {
    held.filter(|held| held.branch == branch)
        .map(|held| held.prs)
        .unwrap_or_default()
}

/// The contents of `pr.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Recorded {
    /// When the forge was asked, in epoch seconds.
    asked: u64,
    branch: String,
    /// One request that does not read, such as a standing a newer amx
    /// wrote, costs that request only.
    #[serde(deserialize_with = "readable")]
    prs: Vec<Pr>,
    /// See [`merged_heads`].
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "readable"
    )]
    merged_heads: Vec<String>,
}

/// The entries of a list that read, or none if it is not a list.
fn readable<'de, D, T>(reading: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Read<T> {
        Good(T),
        Bad(serde::de::IgnoredAny),
    }
    Ok(match Read::<Vec<Read<T>>>::deserialize(reading)? {
        Read::Good(entries) => entries
            .into_iter()
            .filter_map(|entry| match entry {
                Read::Good(entry) => Some(entry),
                Read::Bad(_) => None,
            })
            .collect(),
        Read::Bad(_) => Vec::new(),
    })
}

/// Whether the cache is for `branch` and needs no refresh: it is fresh, or
/// every request on it is settled.
///
/// An empty list is always refreshed, since a request may be opened later.
fn still_good(held: Option<&Recorded>, branch: &str, now: u64) -> bool {
    let held = match held {
        Some(held) if held.branch == branch => held,
        _ => return false,
    };
    let over = !held.prs.is_empty() && held.prs.iter().all(|pr| pr.standing.settled());
    over || now.saturating_sub(held.asked) < FRESH
}

/// The cache, or `None` when it is missing or unreadable.
fn held(dir: &Path) -> Option<Recorded> {
    let said = std::fs::read_to_string(dir.join(CACHE)).ok()?;
    serde_json::from_str(&said).ok()
}

/// Write the cache atomically.
fn write(
    dir: &Path,
    branch: &str,
    prs: Vec<Pr>,
    merged_heads: &[String],
    asked: u64,
) -> Result<()> {
    let recorded = Recorded {
        asked,
        branch: branch.to_string(),
        prs,
        merged_heads: merged_heads.to_vec(),
    };
    let said = serde_json::to_string(&recorded)?;
    crate::store::write_atomic(&dir.join(CACHE), said.as_bytes())
}

/// Repositories with an ask in flight, so each has at most one.
static ASKING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Ask about `repo` on a detached thread that is never joined, when its turn
/// on the schedule has come, and note on the schedule how the ask went.
fn ask_again(repo: PathBuf, agents: Vec<Asked>, now: u64, forges: Forges, schedule: PathBuf) {
    {
        let Ok(mut asking) = ASKING.lock() else {
            return;
        };
        if !asking.insert(repo.clone()) {
            return;
        }
    }
    if !claim(&schedule, &repo, now) {
        forget(&repo);
        return;
    }

    let asking = repo.clone();
    let done = std::thread::Builder::new()
        .name("amx-pr".to_string())
        .spawn(move || {
            let answered = look_over(&asking, &agents, &forges, now);
            settle(&schedule, &asking, answered);
            forget(&asking);
        });
    if done.is_err() {
        forget(&repo);
    }
}

/// When a repository was last asked about, and how many asks in a row no
/// forge answered: its entry on the schedule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Tried {
    asked: u64,
    #[serde(default)]
    failures: u32,
}

impl Tried {
    /// Seconds from this ask to the next: a minute, and from the second
    /// failure in a row on twice the gap before, up to [`LONGEST`].
    fn gap(self) -> u64 {
        (FRESH << self.failures.saturating_sub(1).min(4)).min(LONGEST)
    }

    fn due(self, now: u64) -> bool {
        now.saturating_sub(self.asked) >= self.gap()
    }
}

/// The schedule, one entry per repository. An entry that does not read
/// costs that repository its place, which reads as due, and not the others.
fn scheduled(file: &Path) -> BTreeMap<PathBuf, Tried> {
    let Ok(said) = std::fs::read_to_string(file) else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(entries)) = serde_json::from_str(&said) else {
        return BTreeMap::new();
    };
    entries
        .into_iter()
        .filter_map(|(repo, tried)| {
            Some((PathBuf::from(repo), serde_json::from_value(tried).ok()?))
        })
        .collect()
}

/// Change the schedule under its lock, so two views never both take one
/// turn, and write it back when it changed. `None` when the lock cannot be
/// had, which asks nothing.
fn rescheduled<T>(
    file: &Path,
    change: impl FnOnce(&mut BTreeMap<PathBuf, Tried>) -> T,
) -> Option<T> {
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(crate::paths::FILE_MODE)
        .open(file.with_extension("lock"))
        .ok()?;
    let _held = nix::fcntl::Flock::lock(lock, nix::fcntl::FlockArg::LockExclusive).ok()?;
    let mut tried = scheduled(file);
    let before = tried.clone();
    let answer = change(&mut tried);
    if tried != before {
        let said = serde_json::to_string(&tried).ok()?;
        crate::store::write_atomic(file, said.as_bytes()).ok()?;
    }
    Some(answer)
}

/// Take `repo`'s turn at `now` if it has come: whether to ask.
fn claim(file: &Path, repo: &Path, now: u64) -> bool {
    rescheduled(file, |tried| {
        let entry = tried.entry(repo.to_path_buf()).or_default();
        let due = entry.due(now);
        if due {
            entry.asked = now;
        }
        due
    })
    .unwrap_or(false)
}

/// Note whether the ask about `repo` was answered.
fn settle(file: &Path, repo: &Path, answered: bool) {
    rescheduled(file, |tried| {
        let entry = tried.entry(repo.to_path_buf()).or_default();
        entry.failures = match answered {
            true => 0,
            false => entry.failures.saturating_add(1),
        };
    });
}

fn forget(repo: &Path) {
    if let Ok(mut asking) = ASKING.lock() {
        asking.remove(repo);
    }
}

/// Ask about every agent's branch in `repo` at once and write each its own
/// answer, as asked at `now`. A branch a full answer leaves out is asked
/// about alone. A forge that does not answer leaves everything written down
/// as it is. Whether a forge answered.
fn look_over(repo: &Path, agents: &[Asked], forges: &Forges, now: u64) -> bool {
    let Some(listing) = ask_the_repository(repo, forges) else {
        return false;
    };
    for agent in agents {
        let looked = listing.of(&agent.branch).or_else(|| {
            // Asked alone by name, a `pr-<N>` branch would read as having no
            // request; what is written down stands instead.
            numbered(&agent.branch)
                .is_none()
                .then(|| ask_with(repo, &agent.branch, forges))
                .flatten()
        });
        if let Some(looked) = looked {
            let _ = write(
                &agent.dir,
                &agent.branch,
                looked.prs,
                &looked.merged_heads,
                now,
            );
        }
    }
    true
}

/// The forge programs, named so tests can put fakes in their place.
#[derive(Debug, Clone)]
struct Forges {
    gh: PathBuf,
    glab: PathBuf,
}

impl Forges {
    /// The `gh` and `glab` on PATH.
    fn installed() -> Forges {
        Forges {
            gh: PathBuf::from("gh"),
            glab: PathBuf::from("glab"),
        }
    }
}

/// The forge an answer came from, which names its fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forge {
    Gh,
    Glab,
}

impl Forge {
    /// The fields naming a request's head branch, head commit and number.
    fn fields(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Forge::Gh => ("headRefName", "headRefOid", "number"),
            Forge::Glab => ("source_branch", "sha", "iid"),
        }
    }
}

/// One forge's answer about a whole repository: the requests it listed, at
/// most [`LIMIT`] of them.
#[derive(Debug)]
struct Listing {
    forge: Forge,
    listed: Vec<serde_json::Value>,
}

impl Listing {
    /// The requests on `branch`: those whose head is it, and for a `pr-<N>`
    /// branch (see [`numbered`]) request N, whose head is a fork's name.
    ///
    /// `None` when the answer is full and holds none of them, since one may
    /// stand beyond it.
    fn of(&self, branch: &str) -> Option<Looked> {
        let (head, commit, number) = self.forge.fields();
        let request = numbered(branch);
        let mine: Vec<serde_json::Value> = self
            .listed
            .iter()
            .filter(|listed| {
                word(listed, head) == branch
                    || request.is_some() && listed.get(number).and_then(|it| it.as_u64()) == request
            })
            .cloned()
            .collect();
        if mine.is_empty() && self.listed.len() >= LIMIT {
            return None;
        }
        let said = serde_json::Value::Array(mine).to_string();
        Some(Looked {
            prs: sorted(match self.forge {
                Forge::Gh => read_gh(&said),
                Forge::Glab => read_glab(&said),
            }),
            merged_heads: merged_heads(&said, commit),
        })
    }
}

/// The request number a `pr-<N>` branch was cut for by `new --pr`.
fn numbered(branch: &str) -> Option<u64> {
    let digits = branch.strip_prefix("pr-")?;
    digits
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| digits.parse().ok())
        .flatten()
}

/// Ask `gh`, then `glab`, about every request in the repository at `at`, the
/// newest [`LIMIT`] of them. `None` when neither answers with a list.
fn ask_the_repository(at: &Path, forges: &Forges) -> Option<Listing> {
    let limit = LIMIT.to_string();
    let fields = format!("{GH_FIELDS},headRefName");
    let asked = [
        (
            Forge::Gh,
            &forges.gh,
            vec![
                "pr", "list", "--state", "all", "--limit", &limit, "--json", &fields,
            ],
        ),
        (
            Forge::Glab,
            &forges.glab,
            vec![
                "mr",
                "list",
                "--all",
                "--per-page",
                &limit,
                "--output",
                "json",
            ],
        ),
    ];
    asked.into_iter().find_map(|(forge, program, args)| {
        let said = run(at, program, &args)?;
        match serde_json::from_str(&said) {
            Ok(serde_json::Value::Array(listed)) => Some(Listing { forge, listed }),
            _ => None,
        }
    })
}

/// One forge answer.
#[derive(Debug)]
struct Looked {
    prs: Vec<Pr>,
    merged_heads: Vec<String>,
}

/// Ask `gh`, then `glab`, about `branch`. `None` when neither answers, which
/// says nothing about the requests.
fn ask(at: &Path, branch: &str) -> Option<Looked> {
    ask_with(at, branch, &Forges::installed())
}

/// [`ask`] with the forge programs given.
fn ask_with(at: &Path, branch: &str, forges: &Forges) -> Option<Looked> {
    if let Some(said) = run(
        at,
        &forges.gh,
        &[
            "pr", "list", "--head", branch, "--state", "all", "--limit", "5", "--json", GH_FIELDS,
        ],
    ) {
        return Some(Looked {
            prs: sorted(read_gh(&said)),
            merged_heads: merged_heads(&said, "headRefOid"),
        });
    }
    if let Some(said) = run(
        at,
        &forges.glab,
        &[
            "mr",
            "list",
            "--source-branch",
            branch,
            "--all",
            "--output",
            "json",
        ],
    ) {
        return Some(Looked {
            prs: sorted(read_glab(&said)),
            merged_heads: merged_heads(&said, "sha"),
        });
    }
    None
}

/// Run a forge command and return its stdout, or `None` if it is missing or
/// fails.
fn run(at: &Path, program: impl AsRef<OsStr>, args: &[&str]) -> Option<String> {
    let out = command(at, program, args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A forge command run in `at`.
///
/// `gh` and `glab` shell out to git in the agent's tree, whose `.git/config`
/// the agent can write. `core.fsmonitor` and `core.hooksPath` would run
/// programs from it, so they are overridden through `GIT_CONFIG_*`, which
/// every git underneath inherits and which beats the config files.
fn command(at: &Path, program: impl AsRef<OsStr>, args: &[&str]) -> Command {
    let mut forge = Command::new(program);
    forge
        .current_dir(at)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_1", "/dev/null")
        // A pager would wait for a reader that is not there.
        .env("GH_PAGER", "cat")
        .env("GLAB_PAGER", "cat")
        .env("NO_COLOR", "1");
    forge
}

/// A pull request's head, as `new --pr` needs it.
///
/// `cross` is set for a branch in another fork, whose name (often `main` or
/// `patch-1`) may not be free to use locally.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PrHead {
    #[serde(rename = "headRefName")]
    pub branch: String,
    #[serde(rename = "headRefOid")]
    pub commit: String,
    #[serde(rename = "isCrossRepository")]
    pub cross: bool,
}

/// The fields requested from `gh pr view --json` for a [`PrHead`].
const HEAD_FIELDS: &str = "headRefName,headRefOid,isCrossRepository";

/// The head of request `number` in `repo`, asked of gh synchronously.
pub fn request_head(repo: &Path, number: u64) -> Result<PrHead> {
    head_from(repo, number, Path::new("gh"))
}

/// [`request_head`] with the gh binary given, so tests can use a fake.
fn head_from(repo: &Path, number: u64, gh: &Path) -> Result<PrHead> {
    let numbered = number.to_string();
    let out = command(repo, gh, &["pr", "view", &numbered, "--json", HEAD_FIELDS])
        .output()
        .map_err(|trouble| match trouble.kind() {
            std::io::ErrorKind::NotFound => anyhow!("gh is not on the PATH"),
            _ => anyhow!("running gh: {trouble}"),
        })?;
    if !out.status.success() {
        // Every failure here means the number is not a request in this
        // repository; naming the repository catches a wrong checkout.
        bail!("no pull request #{number} in {}", repo.display());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("reading gh output for #{number}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// A fake gh in `dir` that prints `said` for request 7 and fails for any
    /// other number. Passed by path so tests never run the real gh.
    fn a_fake_gh(dir: &Path, said: &str) -> PathBuf {
        let gh = dir.join("gh");
        std::fs::write(
            &gh,
            format!("#!/bin/sh\n[ \"$3\" = 7 ] || exit 1\ncat <<'SAID'\n{said}\nSAID\n"),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        wait_until_runnable(&gh);
        gh
    }

    /// Wait until a just-written program can be executed.
    ///
    /// Another test thread's fork can briefly hold the file's write handle,
    /// and exec then fails with ETXTBSY.
    fn wait_until_runnable(program: &Path) {
        for _ in 0..200 {
            match Command::new(program)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Err(busy) if busy.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                _ => return,
            }
        }
        panic!("{} was busy for a second", program.display());
    }

    /// Real gh 2.97.0 output for amx's own repository, trimmed to one check.
    const A_MERGED_ONE: &str = r#"[
      {"headRefName":"ci-green-check","isDraft":false,"number":5,
       "reviewDecision":"","state":"MERGED",
       "statusCheckRollup":[{"__typename":"CheckRun","conclusion":"SUCCESS",
         "name":"ci (ubuntu-latest)","status":"COMPLETED"}]}
    ]"#;

    fn one(said: &str) -> Pr {
        let read = read_gh(said);
        assert_eq!(read.len(), 1, "{read:?}");
        read.into_iter().next().expect("one request")
    }

    /// A one-request gh listing with the given fields.
    fn a_request(state: &str, draft: bool, review: &str, checks: &str) -> String {
        format!(
            r#"[{{"number":12,"state":"{state}","isDraft":{draft},
                  "reviewDecision":"{review}","statusCheckRollup":{checks}}}]"#
        )
    }

    /// A rollup of one completed check run.
    fn a_check(conclusion: &str) -> String {
        format!(r#"[{{"__typename":"CheckRun","status":"COMPLETED","conclusion":"{conclusion}"}}]"#)
    }

    #[test]
    fn a_request_is_a_number_and_the_word_its_colour_comes_from() {
        let merged = one(A_MERGED_ONE);
        assert_eq!(merged.number, 5);
        assert_eq!(merged.standing, Standing::Merged);
        assert_eq!(merged.label(), "#5", "which is what a row calls it");
    }

    #[test]
    fn a_request_that_ended_says_so_over_everything_else() {
        // A merged or closed request outranks draft, review and checks.
        for (state, want) in [("MERGED", Standing::Merged), ("CLOSED", Standing::Closed)] {
            let ended = one(&a_request(
                state,
                true,
                "CHANGES_REQUESTED",
                &a_check("FAILURE"),
            ));
            assert_eq!(ended.standing, want);
        }
    }

    #[test]
    fn a_request_nobody_is_looking_at_yet_is_a_draft() {
        let draft = one(&a_request("OPEN", true, "", &a_check("FAILURE")));
        assert_eq!(
            draft.standing,
            Standing::Draft,
            "a check on a draft is not asking anybody for anything"
        );
    }

    #[test]
    fn a_request_says_the_checks_before_it_says_the_review() {
        let failing = one(&a_request("OPEN", false, "APPROVED", &a_check("FAILURE")));
        assert_eq!(failing.standing, Standing::Failing);

        let asked = one(&a_request(
            "OPEN",
            false,
            "CHANGES_REQUESTED",
            &a_check("SUCCESS"),
        ));
        assert_eq!(asked.standing, Standing::Changes);

        let ready = one(&a_request("OPEN", false, "APPROVED", &a_check("SUCCESS")));
        assert_eq!(ready.standing, Standing::Ready);

        let waiting = one(&a_request(
            "OPEN",
            false,
            "REVIEW_REQUIRED",
            &a_check("SUCCESS"),
        ));
        assert_eq!(
            waiting.standing,
            Standing::Open,
            "green and unread is the ordinary state of a request"
        );
    }

    #[test]
    fn a_request_is_running_while_any_check_has_not_finished() {
        let mixed = r#"[{"number":12,"state":"OPEN","isDraft":false,"reviewDecision":"",
          "statusCheckRollup":[
            {"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}]}]"#;
        assert_eq!(one(mixed).standing, Standing::Running);

        // One failure wins over checks still running.
        let failed = r#"[{"number":12,"state":"OPEN","isDraft":false,"reviewDecision":"",
          "statusCheckRollup":[
            {"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},
            {"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}]}]"#;
        assert_eq!(one(failed).standing, Standing::Failing);

        // A StatusContext has its verdict in `state`.
        let context = r#"[{"number":12,"state":"OPEN","isDraft":false,"reviewDecision":"",
          "statusCheckRollup":[{"__typename":"StatusContext","state":"PENDING"}]}]"#;
        assert_eq!(one(context).standing, Standing::Running);

        // No checks configured is not a pass.
        let none = one(&a_request("OPEN", false, "", "[]"));
        assert_eq!(none.standing, Standing::Open);
    }

    #[test]
    fn a_request_amx_cannot_read_costs_the_column_and_nothing_else() {
        for said in [
            "",
            "null",
            "not json at all",
            r#"{"message":"gh had something else to say"}"#,
            // No number, so nothing to label.
            r#"[{"state":"OPEN"}]"#,
        ] {
            assert!(read_gh(said).is_empty(), "{said:?}");
            assert!(read_glab(said).is_empty(), "{said:?}");
        }
    }

    #[test]
    fn a_gitlab_request_is_read_from_the_words_gitlab_uses() {
        let listed = r#"[
          {"iid":7,"state":"opened","draft":false,"head_pipeline":{"status":"failed"}},
          {"iid":8,"state":"merged","draft":false},
          {"iid":9,"state":"opened","work_in_progress":true},
          {"iid":10,"state":"opened","draft":false,"head_pipeline":{"status":"running"}}
        ]"#;
        let read = read_glab(listed);
        assert_eq!(
            read.iter().map(|mr| mr.standing).collect::<Vec<_>>(),
            [
                Standing::Failing,
                Standing::Merged,
                Standing::Draft,
                Standing::Running
            ]
        );
        assert_eq!(read[0].label(), "#7", "and gitlab's number is its iid");
    }

    #[test]
    fn a_branch_read_twice_shows_the_attempt_that_is_still_going() {
        let read = sorted(vec![
            Pr {
                number: 9,
                standing: Standing::Merged,
            },
            Pr {
                number: 4,
                standing: Standing::Open,
            },
            Pr {
                number: 7,
                standing: Standing::Closed,
            },
        ]);
        assert_eq!(
            read.iter().map(|pr| pr.number).collect::<Vec<_>>(),
            [4, 9, 7],
            "what is still live comes first, and the newest ending after it"
        );
    }

    /// A fake forge in a directory of its own: a `gh` that logs each call and
    /// prints `listing` when asked about the whole repository and `alone`
    /// when asked about one branch, and no `glab`.
    struct Fake {
        dir: TempDir,
        forges: Forges,
    }

    impl Fake {
        fn new(listing: &str, alone: &str) -> Fake {
            let dir = TempDir::new().unwrap();
            std::fs::write(dir.path().join("listing"), listing).unwrap();
            std::fs::write(dir.path().join("alone"), alone).unwrap();
            let at = dir.path().display();
            let gh = dir.path().join("gh");
            std::fs::write(
                &gh,
                format!(
                    "#!/bin/sh\n\
                     printf '%s\\n' \"$*\" >> '{at}/calls'\n\
                     [ -f '{at}/fails' ] && exit 1\n\
                     case \" $* \" in *' --head '*) cat '{at}/alone' ;; *) cat '{at}/listing' ;; esac\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
            wait_until_runnable(&gh);
            let forges = Forges {
                gh,
                glab: dir.path().join("no-glab"),
            };
            Fake { dir, forges }
        }

        /// From now on every call fails.
        fn fails(&self) {
            std::fs::write(self.dir.path().join("fails"), "").unwrap();
        }

        /// The calls made so far, one line of arguments each, leaving out
        /// the bare run that checked the script could be run.
        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .filter(|call| !call.is_empty())
                .map(str::to_string)
                .collect()
        }

        /// The schedule this forge's asks go through.
        fn schedule(&self) -> PathBuf {
            self.dir.path().join(SCHEDULE)
        }

        /// One [`refresh_with`] at `now` over agents still going, waited out.
        fn refresh(&self, agents: &[(&Path, &Path, &str)], now: u64) {
            self.refresh_as(agents, false, now);
        }

        /// [`Fake::refresh`] over agents that have `ended` or not.
        fn refresh_as(&self, agents: &[(&Path, &Path, &str)], ended: bool, now: u64) {
            let asked = agents
                .iter()
                .map(|(repo, dir, branch)| {
                    let asked = Asked {
                        dir: dir.to_path_buf(),
                        branch: branch.to_string(),
                        ended,
                    };
                    (repo.to_path_buf(), asked)
                })
                .collect();
            refresh_with(asked, now, &self.forges, &self.schedule());
            for (repo, _, _) in agents {
                asked_by_now(repo);
            }
        }
    }

    /// A gh listing of open requests, each `(number, head branch)`, with the
    /// head commit `sha-<number>`.
    fn listed(requests: &[(u64, &str)]) -> String {
        listed_as(requests, "OPEN")
    }

    /// [`listed`] with every request in `state`.
    fn listed_as(requests: &[(u64, &str)], state: &str) -> String {
        let entries: Vec<serde_json::Value> = requests
            .iter()
            .map(|(number, head)| {
                serde_json::json!({
                    "number": number,
                    "state": state,
                    "isDraft": false,
                    "reviewDecision": "",
                    "statusCheckRollup": [],
                    "headRefName": head,
                    "headRefOid": format!("sha-{number}"),
                })
            })
            .collect();
        serde_json::Value::Array(entries).to_string()
    }

    /// Record directories for `branches`, each its own.
    fn records(branches: usize) -> Vec<TempDir> {
        (0..branches).map(|_| TempDir::new().unwrap()).collect()
    }

    fn numbers(dir: &Path, branch: &str) -> Vec<u64> {
        kept(dir, branch).iter().map(|pr| pr.number).collect()
    }

    #[test]
    fn a_look_reads_what_the_last_look_wrote_down() {
        let dir = TempDir::new().unwrap();
        let prs = vec![Pr {
            number: 12,
            standing: Standing::Failing,
        }];
        write(dir.path(), "amx/fix-login-a1b", prs.clone(), &[], 1_000).unwrap();

        let repo = TempDir::new().unwrap();
        let forge = Fake::new("[]", "[]");
        forge.refresh(&[(repo.path(), dir.path(), "amx/fix-login-a1b")], 1_030);
        assert_eq!(
            forge.calls(),
            Vec::<String>::new(),
            "a fresh answer is the answer, and no forge is asked at all"
        );
        assert_eq!(kept(dir.path(), "amx/fix-login-a1b"), prs);
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            left,
            [CACHE],
            "and the file it was written through is not left lying about"
        );
    }

    #[test]
    fn a_look_nobody_will_be_here_for_reads_the_file_and_asks_nothing() {
        let dir = TempDir::new().unwrap();
        let prs = vec![Pr {
            number: 12,
            standing: Standing::Failing,
        }];
        write(dir.path(), "amx/fix-login-a1b", prs.clone(), &[], 1_000).unwrap();

        // Stale, but `kept` returns the cache without asking a forge.
        assert!(!still_good(
            held(dir.path()).as_ref(),
            "amx/fix-login-a1b",
            9_000
        ));
        assert_eq!(kept(dir.path(), "amx/fix-login-a1b"), prs);
        assert_eq!(
            kept(dir.path(), "amx/port-importer-b2c"),
            Vec::new(),
            "and an answer about another branch is not this branch's answer"
        );
    }

    #[test]
    fn a_reader_that_can_wait_asks_the_forge_where_what_is_written_down_is_stale() {
        let dir = TempDir::new().unwrap();
        let over = vec![Pr {
            number: 12,
            standing: Standing::Merged,
        }];
        write(dir.path(), "amx/fix-login-a1b", over.clone(), &[], 1_000).unwrap();
        assert_eq!(
            ask_now(
                dir.path(),
                dir.path(),
                "amx/fix-login-a1b",
                90_000,
                &Forges::installed()
            ),
            over,
            "a request that is over stays over, so no forge is asked about it"
        );

        let going = vec![Pr {
            number: 12,
            standing: Standing::Open,
        }];
        write(
            dir.path(),
            "amx/fix-login-a1b",
            going.clone(),
            &["aaa".to_string()],
            1_000,
        )
        .unwrap();
        let before = std::fs::read(dir.path().join(CACHE)).unwrap();
        assert_eq!(
            ask_now(
                dir.path(),
                dir.path(),
                "amx/fix-login-a1b",
                90_000,
                &Forges::installed()
            ),
            going,
            "and a stale one is asked about here and now, rather than in a \
             thread this reader will not be around for: no forge answers in a \
             temporary directory, so what was written down still stands"
        );
        assert_eq!(
            std::fs::read(dir.path().join(CACHE)).unwrap(),
            before,
            "and is still written down, merged heads and all"
        );
    }

    /// Wait until no ask is in flight about `repo`.
    fn asked_by_now(repo: &Path) {
        for _ in 0..1_000 {
            if !ASKING.lock().unwrap().contains(repo) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the ask about {} never finished", repo.display());
    }

    #[test]
    fn a_reading_asks_about_a_whole_repository_once_and_writes_each_branch_its_own() {
        let repo = TempDir::new().unwrap();
        let dirs = records(5);
        let branches = [
            "amx/a-a1b",
            "amx/b-b2c",
            "amx/c-c3d",
            "amx/d-d4e",
            "amx/e-e5f",
        ];
        let requests: Vec<(u64, &str)> = (11..).zip(branches).collect();
        let forge = Fake::new(&listed(&requests), "[]");
        let agents: Vec<(&Path, &Path, &str)> = dirs
            .iter()
            .zip(branches)
            .map(|(dir, branch)| (repo.path(), dir.path(), branch))
            .collect();

        for now in [1_000, 1_001, 1_030, 1_045] {
            forge.refresh(&agents, now);
        }
        assert_eq!(forge.calls().len(), 1, "{:?}", forge.calls());
        assert!(
            forge.calls()[0].starts_with("pr list --state all --limit 100"),
            "{:?}",
            forge.calls()
        );
        for ((dir, branch), number) in dirs.iter().zip(branches).zip(11..) {
            assert_eq!(numbers(dir.path(), branch), [number], "{branch}");
        }

        // Once the answers are a minute old, one more ask.
        forge.refresh(&agents, 1_000 + FRESH);
        assert_eq!(forge.calls().len(), 2, "{:?}", forge.calls());
    }

    #[test]
    fn two_repositories_are_asked_about_once_each() {
        let (one, two) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let dirs = records(6);
        let branches = [
            "amx/a-a1b",
            "amx/b-b2c",
            "amx/c-c3d",
            "amx/d-d4e",
            "amx/e-e5f",
            "amx/f-f6a",
        ];
        let requests: Vec<(u64, &str)> = (11..).zip(branches).collect();
        let forge = Fake::new(&listed(&requests), "[]");
        let agents: Vec<(&Path, &Path, &str)> = dirs
            .iter()
            .zip(branches)
            .enumerate()
            .map(|(at, (dir, branch))| match at < 3 {
                true => (one.path(), dir.path(), branch),
                false => (two.path(), dir.path(), branch),
            })
            .collect();

        for now in [1_000, 1_020, 1_045] {
            forge.refresh(&agents, now);
        }
        assert_eq!(forge.calls().len(), 2, "{:?}", forge.calls());
        for ((dir, branch), number) in dirs.iter().zip(branches).zip(11..) {
            assert_eq!(numbers(dir.path(), branch), [number], "{branch}");
        }
    }

    #[test]
    fn a_branch_is_written_only_its_own_requests_and_their_merged_heads() {
        let repo = TempDir::new().unwrap();
        let dirs = records(3);
        let mut requests: Vec<serde_json::Value> =
            serde_json::from_str(&listed_as(&[(4, "amx/a-a1b")], "MERGED")).unwrap();
        requests.extend(
            serde_json::from_str::<Vec<serde_json::Value>>(&listed(&[
                (9, "amx/a-a1b"),
                (12, "main"),
                (13, "amx/elsewhere-z9z"),
            ]))
            .unwrap(),
        );
        let forge = Fake::new(&serde_json::Value::Array(requests).to_string(), "[]");
        forge.refresh(
            &[
                (repo.path(), dirs[0].path(), "amx/a-a1b"),
                (repo.path(), dirs[1].path(), "pr-12"),
                (repo.path(), dirs[2].path(), "amx/c-c3d"),
            ],
            1_000,
        );

        assert_eq!(numbers(dirs[0].path(), "amx/a-a1b"), [9, 4]);
        assert_eq!(
            held(dirs[0].path()).unwrap().merged_heads,
            ["sha-4"],
            "the merged head of its own request and nobody else's"
        );
        assert_eq!(
            numbers(dirs[1].path(), "pr-12"),
            [12],
            "a request from a fork is found by its number, since its head is \
             the fork's own name"
        );
        assert_eq!(numbers(dirs[2].path(), "amx/c-c3d"), Vec::<u64>::new());
        assert!(
            held(dirs[2].path()).is_some(),
            "and a branch with no request is written down as having none"
        );
    }

    #[test]
    fn a_branch_a_full_answer_leaves_out_is_asked_about_alone() {
        let repo = TempDir::new().unwrap();
        let dirs = records(3);
        let others: Vec<(u64, String)> = (1000..1000 + LIMIT as u64)
            .map(|number| (number, format!("amx/other-{number}")))
            .collect();
        let others: Vec<(u64, &str)> = others
            .iter()
            .map(|(number, head)| (*number, head.as_str()))
            .collect();
        let forge = Fake::new(&listed(&others), &listed(&[(7, "amx/old-a1b")]));
        write(
            dirs[1].path(),
            "pr-5",
            vec![Pr {
                number: 5,
                standing: Standing::Open,
            }],
            &[],
            1,
        )
        .unwrap();
        forge.refresh(
            &[
                (repo.path(), dirs[0].path(), "amx/old-a1b"),
                (repo.path(), dirs[1].path(), "pr-5"),
                (repo.path(), dirs[2].path(), "amx/other-1000"),
            ],
            1_000,
        );

        assert_eq!(numbers(dirs[0].path(), "amx/old-a1b"), [7]);
        assert_eq!(
            numbers(dirs[1].path(), "pr-5"),
            [5],
            "a fork's request asked about by name is not found, so what is \
             written down stands"
        );
        assert_eq!(numbers(dirs[2].path(), "amx/other-1000"), [1000]);
        let calls = forge.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[1].contains("--head amx/old-a1b"), "{calls:?}");
    }

    #[test]
    fn sweep_keeps_a_fork_request_the_view_found_by_its_number() {
        let dir = TempDir::new().unwrap();
        let open = vec![Pr {
            number: 12,
            standing: Standing::Open,
        }];
        write(dir.path(), "pr-12", open.clone(), &[], 1_000).unwrap();
        // Asked by name, the forge knows no request on pr-12: its head is the
        // fork's own name.
        let forge = Fake::new("[]", "[]");
        assert_eq!(
            ask_now(dir.path(), dir.path(), "pr-12", 90_000, &forge.forges),
            open
        );
        assert_eq!(kept(dir.path(), "pr-12"), open, "and it stays written down");
    }

    #[test]
    fn a_forge_that_fails_is_asked_at_doubling_gaps_until_it_answers() {
        let repo = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let forge = Fake::new(&listed(&[(7, "amx/fix-login-a1b")]), "[]");
        forge.fails();
        let agents = [(repo.path(), dir.path(), "amx/fix-login-a1b")];

        let mut at = 1_000;
        forge.refresh(&agents, at);
        assert_eq!(forge.calls().len(), 1);
        for (asks, gap) in (2..).zip([60, 120, 240, 480, 960, 960, 960]) {
            forge.refresh(&agents, at + gap - 1);
            assert_eq!(forge.calls().len(), asks - 1, "not before {gap} s");
            at += gap;
            forge.refresh(&agents, at);
            assert_eq!(forge.calls().len(), asks, "but at {gap} s");
        }
        assert!(
            kept(dir.path(), "amx/fix-login-a1b").is_empty(),
            "nothing came of the failures"
        );

        // The first answer puts the gap back to a minute.
        std::fs::remove_file(forge.dir.path().join("fails")).unwrap();
        at += 960;
        forge.refresh(&agents, at);
        let answered = forge.calls().len();
        assert_eq!(numbers(dir.path(), "amx/fix-login-a1b"), [7]);
        forge.refresh(&agents, at + 59);
        assert_eq!(forge.calls().len(), answered);
        forge.refresh(&agents, at + 60);
        assert_eq!(forge.calls().len(), answered + 1);
    }

    #[test]
    fn an_ended_agent_with_no_request_is_asked_about_every_ten_minutes() {
        let repo = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let forge = Fake::new("[]", "[]");
        write(dir.path(), "amx/fix-login-a1b", Vec::new(), &[], 1_000).unwrap();
        let agents = [(repo.path(), dir.path(), "amx/fix-login-a1b")];

        for now in [1_060, 1_300, 1_599] {
            forge.refresh_as(&agents, true, now);
        }
        assert_eq!(forge.calls().len(), 0, "{:?}", forge.calls());
        forge.refresh_as(&agents, true, 1_600);
        assert_eq!(forge.calls().len(), 1);

        // One still going is asked about once a minute.
        let going = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        write(dir.path(), "amx/port-importer-b2c", Vec::new(), &[], 1_000).unwrap();
        forge.refresh_as(
            &[(going.path(), dir.path(), "amx/port-importer-b2c")],
            false,
            1_060,
        );
        assert_eq!(forge.calls().len(), 2);
    }

    #[test]
    fn a_turn_another_view_took_is_not_taken_again() {
        let repo = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let forge = Fake::new(&listed(&[(7, "amx/fix-login-a1b")]), "[]");
        forge.refresh(&[(repo.path(), dir.path(), "amx/fix-login-a1b")], 1_000);
        assert_eq!(forge.calls().len(), 1);

        // Another view reads the same schedule.
        assert!(!claim(&forge.schedule(), repo.path(), 1_030));
        assert!(claim(&forge.schedule(), repo.path(), 1_060));
        assert!(
            !claim(&forge.schedule(), repo.path(), 1_060),
            "and a turn is taken once"
        );
    }

    #[test]
    fn a_schedule_entry_that_does_not_read_costs_that_repository_and_not_the_others() {
        let at = TempDir::new().unwrap();
        let file = at.path().join(SCHEDULE);
        std::fs::write(
            &file,
            r#"{"/srv/one":{"asked":"soon"},"/srv/two":{"asked":1000,"failures":2},"/srv/three":7}"#,
        )
        .unwrap();
        assert_eq!(
            scheduled(&file),
            BTreeMap::from([(
                PathBuf::from("/srv/two"),
                Tried {
                    asked: 1_000,
                    failures: 2,
                },
            )])
        );
        assert!(
            claim(&file, Path::new("/srv/one"), 1_010),
            "the one that does not read is due"
        );
        assert!(
            !claim(&file, Path::new("/srv/two"), 1_010),
            "and the one that does keeps its gap"
        );
        assert_eq!(scheduled(&file)[Path::new("/srv/two")].failures, 2);

        std::fs::write(&file, "half a doc").unwrap();
        assert!(
            scheduled(&file).is_empty(),
            "and so is a file nothing reads"
        );
    }

    #[test]
    fn a_forge_that_cannot_be_asked_leaves_the_requests_written_down() {
        let repo = TempDir::new().unwrap();
        let forge = Fake::new("[]", "[]");
        forge.fails();
        let dir = TempDir::new().unwrap();
        let going = vec![Pr {
            number: 12,
            standing: Standing::Open,
        }];
        write(
            dir.path(),
            "amx/fix-login-a1b",
            going.clone(),
            &["aaa".to_string()],
            1_000,
        )
        .unwrap();
        let before = std::fs::read(dir.path().join(CACHE)).unwrap();

        // 70 s on, stale: a forge that does not answer is asked.
        let agents = [(repo.path(), dir.path(), "amx/fix-login-a1b")];
        forge.refresh(&agents, 1_070);
        assert_eq!(forge.calls().len(), 1);
        assert_eq!(kept(dir.path(), "amx/fix-login-a1b"), going);
        assert_eq!(
            std::fs::read(dir.path().join(CACHE)).unwrap(),
            before,
            "the badge and the merged heads stay as they were"
        );
        forge.refresh(&agents, 1_071);
        assert_eq!(
            forge.calls().len(),
            1,
            "and the next reading does not ask again so soon"
        );
        forge.refresh(&agents, 1_070 + FRESH);
        assert_eq!(forge.calls().len(), 2, "until the freshness has passed");
    }

    #[test]
    fn a_request_that_does_not_read_costs_that_request_and_not_the_others() {
        let dir = TempDir::new().unwrap();
        for said in [
            r#"{"asked":1000,"branch":"amx/fix-login-a1b","prs":[{"number":3,"standing":"open"},{"number":4,"standing":"weird"}]}"#,
            r#"{"asked":1000,"branch":"amx/fix-login-a1b","prs":[{"number":3,"standing":"open"},{"number":"five","standing":"open"}]}"#,
            r#"{"asked":1000,"branch":"amx/fix-login-a1b","prs":[{"number":3,"standing":"open"}],"merged_heads":"abc"}"#,
        ] {
            std::fs::write(dir.path().join(CACHE), said).unwrap();
            assert_eq!(
                kept(dir.path(), "amx/fix-login-a1b"),
                [Pr {
                    number: 3,
                    standing: Standing::Open,
                }],
                "{said}"
            );
        }
    }

    #[test]
    fn a_forge_that_cannot_be_asked_about_a_new_branch_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let forge = Fake::new("[]", "[]");
        forge.fails();
        forge.refresh(&[(repo.path(), dir.path(), "amx/fix-login-a1b")], 1_000);
        assert_eq!(forge.calls().len(), 1);
        assert!(!dir.path().join(CACHE).exists());
        assert_eq!(
            ask_now(
                dir.path(),
                dir.path(),
                "amx/fix-login-a1b",
                1_000,
                &Forges::installed()
            ),
            Vec::new()
        );
        assert!(!dir.path().join(CACHE).exists());
    }

    #[test]
    fn a_look_at_nothing_written_down_answers_with_no_requests() {
        let dir = TempDir::new().unwrap();
        assert_eq!(held(dir.path()), None);

        std::fs::write(dir.path().join(CACHE), "half a doc").unwrap();
        assert_eq!(held(dir.path()), None, "and so is a file nothing can read");
    }

    #[test]
    fn a_look_asks_again_when_what_is_written_down_is_old_or_somebody_elses() {
        let held = Recorded {
            asked: 1_000,
            branch: "amx/fix-login-a1b".to_string(),
            prs: Vec::new(),
            merged_heads: Vec::new(),
        };
        assert!(still_good(
            Some(&held),
            "amx/fix-login-a1b",
            1_000 + FRESH - 1
        ));
        assert!(
            !still_good(Some(&held), "amx/fix-login-a1b", 1_000 + FRESH),
            "past the freshness it is worth asking again"
        );
        assert!(
            !still_good(Some(&held), "amx/port-importer-b2c", 1_010),
            "and an answer about another branch is not this branch's answer"
        );
        assert!(!still_good(None, "amx/fix-login-a1b", 1_010));

        // A clock that went backwards keeps the cache.
        assert!(still_good(Some(&held), "amx/fix-login-a1b", 900));
    }

    #[test]
    fn a_look_stops_asking_about_a_branch_whose_requests_are_all_over() {
        // Settled requests do not change, so the view stops polling for them.
        let over = Recorded {
            asked: 1_000,
            branch: "amx/fix-login-a1b".to_string(),
            prs: vec![
                Pr {
                    number: 12,
                    standing: Standing::Merged,
                },
                Pr {
                    number: 9,
                    standing: Standing::Closed,
                },
            ],
            merged_heads: Vec::new(),
        };
        assert!(still_good(Some(&over), "amx/fix-login-a1b", 90_000));

        let mut going = over.clone();
        going.prs[0].standing = Standing::Ready;
        assert!(
            !still_good(Some(&going), "amx/fix-login-a1b", 90_000),
            "one that is still going is asked about until it is not"
        );

        let none = Recorded {
            prs: Vec::new(),
            ..over
        };
        assert!(
            !still_good(Some(&none), "amx/fix-login-a1b", 90_000),
            "and a branch nobody has opened one for is asked about again, \
             because opening one later is what the column is for"
        );
    }

    #[test]
    fn a_look_hands_back_nothing_for_an_agent_with_no_branch_of_its_own() {
        let meta = Meta {
            role: None,
            parent: None,
            depth: 0,
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: crate::tmux::Socket::Name("amx".to_string()),
            pane: crate::tmux::PaneId::new("%1").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        };
        assert!(
            asked_about(&meta, false).is_none(),
            "and nothing asks about it"
        );
        assert!(
            written(&meta).is_empty(),
            "an agent working in a directory has no branch of amx's making, \
             and the person's own checkout is not this agent's work"
        );
    }

    #[test]
    fn a_machine_with_no_forge_on_it_loses_the_column_and_nothing_else() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            run(dir.path(), "amx-no-forge-by-this-name", &["--version"]),
            None,
            "a program that is not installed is not a failure to report"
        );
        assert!(
            ask(dir.path(), "amx/fix-login-a1b").is_none(),
            "and neither is a directory no forge will answer about: it is no \
             answer, rather than an answer of no requests"
        );
    }

    #[test]
    fn a_forge_runs_nothing_the_tree_it_reads_names() {
        // The forges run git in the agent's tree; only the environment
        // reaches that git.
        let dir = TempDir::new().unwrap();
        let forge = command(dir.path(), "gh", &["pr", "list"]);
        let set: Vec<(String, String)> = forge
            .get_envs()
            .filter_map(|(name, value)| Some((name.to_string_lossy().into_owned(), value?)))
            .map(|(name, value)| (name, value.to_string_lossy().into_owned()))
            .collect();
        let named = |key: &str| {
            set.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };

        assert_eq!(named("GIT_CONFIG_NOSYSTEM"), Some("1"));
        assert_eq!(named("GIT_CONFIG_COUNT"), Some("2"));
        let blanked: Vec<(&str, &str)> = (0..2)
            .filter_map(|n| {
                Some((
                    named(&format!("GIT_CONFIG_KEY_{n}"))?,
                    named(&format!("GIT_CONFIG_VALUE_{n}"))?,
                ))
            })
            .collect();
        assert_eq!(
            blanked,
            [("core.fsmonitor", "false"), ("core.hooksPath", "/dev/null")],
            "every key here names a program the agent could have written"
        );
    }

    #[test]
    fn a_requests_head_is_a_branch_a_commit_and_whether_it_came_from_a_fork() {
        let dir = TempDir::new().unwrap();
        let gh = a_fake_gh(
            dir.path(),
            r#"{"headRefName":"fix-login","headRefOid":"4f3f0b2c5b6d1e8a9c0d7e2f1a3b4c5d6e7f8091",
                "isCrossRepository":false}"#,
        );

        let head = head_from(dir.path(), 7, &gh).unwrap();
        assert_eq!(head.branch, "fix-login");
        assert_eq!(head.commit, "4f3f0b2c5b6d1e8a9c0d7e2f1a3b4c5d6e7f8091");
        assert!(!head.cross, "the request is on a branch of this repository");
    }

    #[test]
    fn a_request_from_a_fork_says_so_because_its_branch_name_is_not_free() {
        // A fork's branch name may clash locally, so the caller names the
        // local branch after the number instead.
        let dir = TempDir::new().unwrap();
        let gh = a_fake_gh(
            dir.path(),
            r#"{"headRefName":"main","headRefOid":"4f3f0b2c5b6d1e8a9c0d7e2f1a3b4c5d6e7f8091",
                "isCrossRepository":true}"#,
        );

        let head = head_from(dir.path(), 7, &gh).unwrap();
        assert_eq!(head.branch, "main");
        assert!(head.cross);
    }

    #[test]
    fn a_number_that_is_no_request_is_refused_and_names_the_repository() {
        let dir = TempDir::new().unwrap();
        let gh = a_fake_gh(dir.path(), "{}");

        let refused = head_from(dir.path(), 9, &gh).unwrap_err();
        let said = format!("{refused:#}");
        assert!(said.contains("#9"), "the number that was typed: {said}");
        assert!(
            said.contains(&dir.path().display().to_string()),
            "and where it was looked for, since a person with several \
             checkouts has typed it in the wrong one: {said}"
        );
    }

    #[test]
    fn a_machine_with_no_gh_on_it_says_gh_is_not_on_the_path() {
        // Unlike the column, `new --pr` cannot proceed without gh.
        let dir = TempDir::new().unwrap();

        let refused = head_from(dir.path(), 7, &dir.path().join("gh")).unwrap_err();
        assert!(
            format!("{refused:#}").contains("gh is not on the PATH"),
            "{refused:#}"
        );
    }

    #[test]
    fn a_gh_amx_cannot_read_is_refused_rather_than_read_as_a_head() {
        let dir = TempDir::new().unwrap();
        let gh = a_fake_gh(dir.path(), "not json at all");
        assert!(head_from(dir.path(), 7, &gh).is_err());
    }

    #[test]
    fn a_merged_request_says_the_head_it_went_in_at() {
        let gh = r#"[{"number":12,"state":"MERGED","headRefOid":"aaa"},
                     {"number":9,"state":"CLOSED","headRefOid":"bbb"},
                     {"number":7,"state":"OPEN","headRefOid":"ccc"}]"#;
        assert_eq!(merged_heads(gh, "headRefOid"), ["aaa"]);
        let glab =
            r#"[{"iid":3,"state":"merged","sha":"ddd"},{"iid":4,"state":"opened","sha":"eee"}]"#;
        assert_eq!(merged_heads(glab, "sha"), ["ddd"]);
        assert!(merged_heads("not json", "sha").is_empty());
    }
}
