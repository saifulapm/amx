//! The pull request on an agent's branch and its status, asked of `gh` or
//! `glab`.
//!
//! A machine with neither installed gets no pull requests, never an error.
//!
//! - Interactive readers never wait on a forge: they read the answer cached in
//!   `pr.json` beside the record and, when it is stale, refresh it on a
//!   detached thread.
//! - Only [`asked_now`] (for `sweep`) and [`request_head`] (for `new --pr`)
//!   call the forge synchronously.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::store::Meta;

/// Seconds a cached forge answer stays fresh.
pub const FRESH: u64 = 60;

/// The cache file, in the agent's record directory.
const CACHE: &str = "pr.json";

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

/// The pull requests on this agent's branch from the cache, starting a
/// background refresh when it is stale.
pub fn of(meta: &Meta) -> Vec<Pr> {
    let Some((dir, at, branch)) = about(meta) else {
        return Vec::new();
    };
    read(&dir, &at, branch, crate::store::now())
}

/// The cached pull requests, with no refresh.
///
/// For verbs that print and exit: a refresh thread would be killed with the
/// process before the forge answered. The view keeps the cache current.
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

/// [`of`] with the record directory and the working directory given.
///
/// Returns the cached answer even when stale, so the column does not blink
/// while a refresh runs.
pub fn read(dir: &Path, at: &Path, branch: &str, now: u64) -> Vec<Pr> {
    let held = held(dir);
    if !still_good(held.as_ref(), branch, now) {
        ask_again(dir.to_path_buf(), at.to_path_buf(), branch.to_string());
    }
    theirs(held, branch)
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
    ask_now(&dir, &at, branch, crate::store::now())
}

/// [`asked_now`] with the record directory and the working directory given.
fn ask_now(dir: &Path, at: &Path, branch: &str, now: u64) -> Vec<Pr> {
    let held = held(dir);
    if still_good(held.as_ref(), branch, now) {
        return theirs(held, branch);
    }
    let looked = ask(at, branch);
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
    prs: Vec<Pr>,
    /// See [`merged_heads`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    merged_heads: Vec<String>,
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

/// Record directories with a refresh in flight, so each agent has at most one.
static ASKING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Refresh the cache on a detached thread that is never joined.
fn ask_again(dir: PathBuf, at: PathBuf, branch: String) {
    {
        let Ok(mut asking) = ASKING.lock() else {
            return;
        };
        if !asking.insert(dir.clone()) {
            return;
        }
    }

    let asking = dir.clone();
    let done = std::thread::Builder::new()
        .name("amx-pr".to_string())
        .spawn(move || {
            let looked = ask(&at, &branch);
            let _ = write(
                &asking,
                &branch,
                looked.prs,
                &looked.merged_heads,
                crate::store::now(),
            );
            forget(&asking);
        });
    if done.is_err() {
        forget(&dir);
    }
}

fn forget(dir: &Path) {
    if let Ok(mut asking) = ASKING.lock() {
        asking.remove(dir);
    }
}

/// One forge answer.
#[derive(Debug, Default)]
struct Looked {
    prs: Vec<Pr>,
    merged_heads: Vec<String>,
}

/// Ask `gh`, then `glab`, about `branch`. Neither answering is no requests.
fn ask(at: &Path, branch: &str) -> Looked {
    if let Some(said) = run(
        at,
        "gh",
        &[
            "pr", "list", "--head", branch, "--state", "all", "--limit", "5", "--json", GH_FIELDS,
        ],
    ) {
        return Looked {
            prs: sorted(read_gh(&said)),
            merged_heads: merged_heads(&said, "headRefOid"),
        };
    }
    if let Some(said) = run(
        at,
        "glab",
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
        return Looked {
            prs: sorted(read_glab(&said)),
            merged_heads: merged_heads(&said, "sha"),
        };
    }
    Looked::default()
}

/// Run a forge command and return its stdout, or `None` if it is missing or
/// fails.
fn run(at: &Path, program: &str, args: &[&str]) -> Option<String> {
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
    serde_json::from_slice(&out.stdout)
        .with_context(|| format!("reading what gh said about #{number}"))
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

    #[test]
    fn a_look_reads_what_the_last_look_wrote_down() {
        let dir = TempDir::new().unwrap();
        let prs = vec![Pr {
            number: 12,
            standing: Standing::Failing,
        }];
        write(dir.path(), "amx/fix-login-a1b", prs.clone(), &[], 1_000).unwrap();

        assert_eq!(
            read(dir.path(), dir.path(), "amx/fix-login-a1b", 1_030),
            prs,
            "a fresh answer is the answer, and no forge is asked at all"
        );
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
            ask_now(dir.path(), dir.path(), "amx/fix-login-a1b", 90_000),
            over,
            "a request that is over stays over, so no forge is asked about it"
        );

        let going = vec![Pr {
            number: 12,
            standing: Standing::Open,
        }];
        write(dir.path(), "amx/fix-login-a1b", going, &[], 1_000).unwrap();
        assert_eq!(
            ask_now(dir.path(), dir.path(), "amx/fix-login-a1b", 90_000),
            Vec::new(),
            "and a stale one is asked about here and now, rather than in a \
             thread this reader will not be around for: there is no forge in a \
             temporary directory, and its answer is the answer"
        );
        assert_eq!(
            held(dir.path()).unwrap().asked,
            90_000,
            "and what it said is written down for whoever reads next"
        );
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
            of(&meta).is_empty(),
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
            ask(dir.path(), "amx/fix-login-a1b").prs.is_empty(),
            "and neither is a directory the forge has nothing to say about"
        );
    }

    #[test]
    fn hardening_a_forge_runs_nothing_the_tree_it_reads_names() {
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
