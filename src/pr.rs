//! Whether an agent's branch has a pull request, and how that request is
//! doing.
//!
//! A branch is where an agent's work goes; a pull request is what happens to it
//! afterwards. That is worth a column on the row, because it answers a question
//! the rest of the row cannot: the agent finished, and then what? A number says
//! the work left the machine, and the colour on the number says whether
//! anything is standing in its way.
//!
//! None of this is amx's own knowledge. `gh` and `glab` are what a person
//! already talks to their forge with, both answer in JSON, and both are asked
//! the one question a row needs. A machine with neither installed loses the
//! column and nothing else: every reader here answers with no pull requests
//! rather than with a failure, and a row without a number is the row amx has
//! always drawn.
//!
//! **No reader waits on a forge.** A look reads what the last look wrote down
//! beside the record, and where that is old it sets a fresh look going in a
//! thread nobody joins. The worst a reading costs is a number one look behind
//! the network; the alternative is a list that stops for a second every time it
//! is drawn, on the one surface whose whole promise is that it does not.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::store::Meta;

/// How long an answer from the forge is taken at its word.
///
/// A check goes green somewhere between one minute and twenty, and a person
/// watching a row for it will press nothing to make it move. Long enough that
/// a wall of agents is not a wall of subprocesses, short enough that the
/// number is about now.
pub const FRESH: u64 = 60;

/// What the last look wrote down, beside the record it is about.
const CACHE: &str = "pr.json";

/// One pull request, as much of it as a row has room for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub number: u64,
    pub standing: Standing,
}

impl Pr {
    /// What a row calls it, which is what the forges call it and what a person
    /// types to find it again.
    pub fn label(&self) -> String {
        format!("#{}", self.number)
    }
}

/// Where a pull request has got to, in the one word worth a column.
///
/// Eight answers to four questions — is it in, is anybody looking at it yet,
/// did the checks pass, has a reviewer answered — because the four have an
/// order and what a row shows is the first of them with anything to say.
/// [`fold`] is that order, written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Standing {
    /// It is in.
    Merged,
    /// It was shut without going in.
    Closed,
    /// A draft: it is not asking anybody for anything yet.
    Draft,
    /// A check on it failed.
    Failing,
    /// A reviewer asked for changes.
    Changes,
    /// The checks are still running.
    Running,
    /// A reviewer approved it and nothing is failing.
    Ready,
    /// Open, and waiting on whoever reviews it.
    Open,
}

impl Standing {
    /// What the card says beside the number.
    ///
    /// A row has one colour for this and the colours are five, so two
    /// standings can wear one: the colour answers how it is going, and these
    /// words answer which of the four questions the colour came from — which
    /// is the thing a person opens a card to find out.
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

    /// Whether nothing more is going to happen to it.
    pub fn settled(self) -> bool {
        matches!(self, Standing::Merged | Standing::Closed)
    }
}

/// How the checks on a request went, folded from however many there were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Checks {
    Passing,
    Failing,
    Running,
}

/// Where a pull request has got to, from what the forge said about it.
///
/// The order is what a person scanning a wall of rows wants first. An ending
/// outranks everything, because nothing is being asked of anybody about a
/// request that is over. A draft outranks the rest for the same reason: it is
/// not offered yet, so a red check on it is not news. Then the checks, which
/// are a fact, before the review, which is an opinion — and a failing check
/// before a running one, because one failure decides the build whatever the
/// rest are still doing.
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

/// The requests in the order a row and a card read them: whatever is still
/// live first, and the newest number of those, because a branch that has been
/// through this twice is being read for the attempt that is still going.
fn sorted(mut prs: Vec<Pr>) -> Vec<Pr> {
    prs.sort_by_key(|pr| (pr.standing.settled(), std::cmp::Reverse(pr.number)));
    prs
}

/// The fields amx asks `gh` for, which are the four questions and nothing else.
const GH_FIELDS: &str = "number,state,isDraft,reviewDecision,statusCheckRollup";

/// What `gh pr list --json` said, read into what a row needs.
///
/// Measured against gh 2.97.0 on 2026-08-24: `state` is `OPEN`, `CLOSED` or
/// `MERGED`, `reviewDecision` is `APPROVED`, `CHANGES_REQUESTED`,
/// `REVIEW_REQUIRED` or empty, and `statusCheckRollup` is a flat array holding
/// two shapes at once — a `CheckRun` says how far it has got in `status` and
/// how it went in `conclusion`, a `StatusContext` says only how it went, in
/// `state`.
///
/// Read out of a `Value` rather than into a struct of amx's own: a field the
/// forge renames should cost the one answer that field carried, not the row.
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

/// The flags amx gives `glab`, and the shape it answers in.
///
/// Written to what `glab mr list` documents rather than measured against a
/// live one, which is why every field here is read as optional and why a
/// spelling it does not know costs one answer rather than the request: `state`
/// is `opened`, `merged`, `closed` or `locked`, a draft is `draft` in a recent
/// glab and `work_in_progress` in an older one, and the checks are the head
/// pipeline's status. GitLab's review decision is not in this listing at all,
/// so a merge request is never read as approved or as asking for changes.
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

/// A string field of an object, or nothing said at all.
fn word<'a>(object: &'a serde_json::Value, key: &str) -> &'a str {
    object.get(key).and_then(|it| it.as_str()).unwrap_or("")
}

/// How the checks went, from however many of them there were.
///
/// One failure decides the whole build, so it answers at once. Otherwise
/// anything that has not finished leaves the answer running, and a request
/// with no checks configured at all has nothing to say rather than a pass.
fn rollup(entries: Option<&Vec<serde_json::Value>>) -> Option<Checks> {
    let entries = entries?;
    if entries.is_empty() {
        return None;
    }

    let mut running = false;
    for entry in entries {
        // A check run's verdict is its conclusion and is empty until it has
        // one; a status context has no conclusion and says its verdict in
        // `state`. Whichever is there is the verdict.
        let went = match word(entry, "conclusion") {
            "" => word(entry, "state"),
            conclusion => conclusion,
        };
        let done = word(entry, "status");
        if FAILED.iter().any(|bad| went.eq_ignore_ascii_case(bad)) {
            return Some(Checks::Failing);
        }
        let passed = PASSED.iter().any(|good| went.eq_ignore_ascii_case(good));
        // Finished with a verdict nothing here recognises is not a failure to
        // report: it is a check amx has no word for, and the row says nothing
        // about it rather than something wrong.
        running |= !passed && !done.eq_ignore_ascii_case("completed");
    }
    Some(match running {
        true => Checks::Running,
        false => Checks::Passing,
    })
}

/// Every way a check can be over and not have passed.
const FAILED: [&str; 7] = [
    "FAILURE",
    "TIMED_OUT",
    "CANCELLED",
    "ACTION_REQUIRED",
    "STARTUP_FAILURE",
    "STALE",
    "ERROR",
];

/// And every way it can be over and not be in anybody's way. A check that was
/// skipped or that does not vote is a check nobody is waiting on.
const PASSED: [&str; 3] = ["SUCCESS", "NEUTRAL", "SKIPPED"];

/// GitLab's pipeline status as the same three answers.
fn pipeline_of(status: &str) -> Option<Checks> {
    match status.to_ascii_lowercase().as_str() {
        "" => None,
        "failed" | "canceled" | "cancelled" => Some(Checks::Failing),
        "success" | "skipped" | "manual" => Some(Checks::Passing),
        _ => Some(Checks::Running),
    }
}

/// The pull requests on this agent's branch, as the last look wrote them down,
/// with a fresh look set going where what is written down has aged.
pub fn of(meta: &Meta) -> Vec<Pr> {
    let Some((dir, at, branch)) = about(meta) else {
        return Vec::new();
    };
    read(&dir, &at, branch, crate::store::now())
}

/// The same, for a reader that will not be here when a fresh answer arrives.
///
/// A verb that prints once and exits is one of those. The thread a look starts
/// is never joined, and the process is gone before a forge has answered: the
/// subprocess is paid for, killed part way through, and what it was going to
/// write is dropped — sometimes with the file it writes through left behind.
/// So this reads what the last look wrote and starts nothing. It is handed no
/// tree to ask in, which is the whole of the difference.
///
/// The view is the reader that does wait, and it is the one that keeps what is
/// written down worth reading.
pub fn written(meta: &Meta) -> Vec<Pr> {
    let Some((dir, _, branch)) = about(meta) else {
        return Vec::new();
    };
    kept(&dir, branch)
}

/// Where a look about an agent goes: the record it is written down beside, the
/// tree a forge would be asked from, and the branch it is about.
///
/// There is nowhere for an agent amx did not cut a branch for: what a row would
/// be labelling then is whatever the person's own checkout happens to be on,
/// which is not this agent's work. A record that is not on the disk and a tree
/// that has been removed are the same answer, because both leave nowhere to run
/// the question in.
fn about(meta: &Meta) -> Option<(PathBuf, PathBuf, &str)> {
    let branch = meta.branch.as_deref()?;
    let dir = crate::paths::agent_dir(&meta.id).ok()?;
    let at = match &meta.worktree {
        Some(tree) if tree.is_dir() => tree.clone(),
        _ => meta.dir.clone(),
    };
    (dir.is_dir() && at.is_dir()).then_some((dir, at, branch))
}

/// The same, with the record's directory and the repository named.
///
/// What is written down comes back whether or not it is fresh: a number a
/// minute old is the answer until a better one arrives, and hiding it while
/// the better one is fetched would blink the column on every reading.
pub fn read(dir: &Path, at: &Path, branch: &str, now: u64) -> Vec<Pr> {
    let held = held(dir);
    if !still_good(held.as_ref(), branch, now) {
        ask_again(dir.to_path_buf(), at.to_path_buf(), branch.to_string());
    }
    theirs(held, branch)
}

/// What the last look wrote about this branch, however long ago it was written.
pub fn kept(dir: &Path, branch: &str) -> Vec<Pr> {
    theirs(held(dir), branch)
}

/// The requests in a look, where the look is about the branch being asked
/// after. One from before a rename answers a question nobody is asking now.
fn theirs(held: Option<Recorded>, branch: &str) -> Vec<Pr> {
    held.filter(|held| held.branch == branch)
        .map(|held| held.prs)
        .unwrap_or_default()
}

/// The last answer about one branch, as it is written down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Recorded {
    /// When the forge was asked, which is what says whether this is still
    /// worth believing.
    asked: u64,
    /// And which branch it was asked about. A record from before a rename
    /// answers a question nobody is asking now.
    branch: String,
    prs: Vec<Pr>,
}

/// Whether what is written down is this branch's and still stands.
///
/// Recent enough, or about a branch whose every request is over. A merged
/// request stays merged, and a wall of finished agents would otherwise put a
/// subprocess and a network call behind every one of them once a minute for as
/// long as the view is open, to be told the same thing every time. A branch
/// with nothing on it is asked about again: a request can be opened later, and
/// that is the whole point of the column for an agent that has finished.
fn still_good(held: Option<&Recorded>, branch: &str, now: u64) -> bool {
    let held = match held {
        Some(held) if held.branch == branch => held,
        _ => return false,
    };
    let over = !held.prs.is_empty() && held.prs.iter().all(|pr| pr.standing.settled());
    over || now.saturating_sub(held.asked) < FRESH
}

/// What the last look wrote. A file that is not there, or that nothing can
/// read, is a look that has not happened yet.
fn held(dir: &Path) -> Option<Recorded> {
    let said = std::fs::read_to_string(dir.join(CACHE)).ok()?;
    serde_json::from_str(&said).ok()
}

/// Write down what the forge said, whole, for whoever reads next.
///
/// Into a file beside it and then renamed, so a reader that arrives mid-write
/// sees the answer before or the answer after and never half of either.
fn write(dir: &Path, branch: &str, prs: Vec<Pr>, asked: u64) -> std::io::Result<()> {
    let recorded = Recorded {
        asked,
        branch: branch.to_string(),
        prs,
    };
    let said = serde_json::to_string(&recorded)?;
    let beside = dir.join(format!("{CACHE}.new"));
    std::fs::write(&beside, said)?;
    let _ = crate::paths::keep_to_the_owner(&beside, crate::paths::FILE_MODE);
    std::fs::rename(&beside, dir.join(CACHE))
}

/// The branches a look is already out for. One question at a time per agent:
/// a reading every second must not put a subprocess behind every one of them.
static ASKING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Ask the forge again, with nobody waiting for the answer.
///
/// The thread is never joined. A view is open for hours and will have the
/// answer on the next reading; a verb that exits first leaves the question
/// unanswered, which costs the column and nothing else.
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
            let prs = ask(&at, &branch);
            let _ = write(&asking, &branch, prs, crate::store::now());
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

/// Whichever forge answers about this branch, in the order a machine is likely
/// to have them. Neither of them installed is no pull requests, which is the
/// same answer as a branch nobody has opened one for.
fn ask(at: &Path, branch: &str) -> Vec<Pr> {
    if let Some(said) = run(
        at,
        "gh",
        &[
            "pr", "list", "--head", branch, "--state", "all", "--limit", "5", "--json", GH_FIELDS,
        ],
    ) {
        return sorted(read_gh(&said));
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
        return sorted(read_glab(&said));
    }
    Vec::new()
}

/// One forge command, with its output as the answer. A command that is not
/// installed, will not run, or says it failed answers with nothing.
fn run(at: &Path, program: &str, args: &[&str]) -> Option<String> {
    let out = command(at, program, args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A forge, pointed at the tree and ready to run.
///
/// It runs in the tree the agent works in, and that tree's own `.git/config`
/// is a list of programs the agent itself can write — `core.fsmonitor` names
/// one git starts before it will look at a file, and a hook runs on the index
/// git refreshes on its way past. `gh` and `glab` both shell out to git, so
/// the same refusal `worktree.rs` writes with `-c` goes here through the
/// environment, which every git underneath inherits and which beats the
/// config files it would otherwise read them from.
fn command(at: &Path, program: &str, args: &[&str]) -> Command {
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
        // Neither forge is being read by a person here, and a pager would hold
        // a thread open waiting for one that is not there.
        .env("GH_PAGER", "cat")
        .env("GLAB_PAGER", "cat")
        .env("NO_COLOR", "1");
    forge
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// What gh 2.97.0 answered about this repository on 2026-08-24, cut to the
    /// fields amx asks for and with the check runs trimmed to one apiece.
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

    /// A request of gh's own shape, with the four answers a row is folded from
    /// written in the forge's own words.
    fn a_request(state: &str, draft: bool, review: &str, checks: &str) -> String {
        format!(
            r#"[{{"number":12,"state":"{state}","isDraft":{draft},
                  "reviewDecision":"{review}","statusCheckRollup":{checks}}}]"#
        )
    }

    /// The rollup of one check run that has finished with this conclusion.
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
        // Nothing is being asked of anybody about a request that is over, so a
        // red check on a merged branch is history rather than news. amx has
        // merged a draft with a failing check on it, which is why this is a
        // rule rather than a hypothetical.
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
        // The checks are a fact and the review is an opinion, and a person
        // scanning a wall wants the fact.
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

        // One failure decides the build whatever the rest are still doing.
        let failed = r#"[{"number":12,"state":"OPEN","isDraft":false,"reviewDecision":"",
          "statusCheckRollup":[
            {"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},
            {"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}]}]"#;
        assert_eq!(one(failed).standing, Standing::Failing);

        // A status context has no conclusion and says its verdict in `state`.
        let context = r#"[{"number":12,"state":"OPEN","isDraft":false,"reviewDecision":"",
          "statusCheckRollup":[{"__typename":"StatusContext","state":"PENDING"}]}]"#;
        assert_eq!(one(context).standing, Standing::Running);

        // And a request nobody has configured a check for has nothing to say
        // about them, which is not the same as passing.
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
            // A row with no number is a row amx cannot label.
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
        write(dir.path(), "amx/fix-login-a1b", prs.clone(), 1_000).unwrap();

        assert_eq!(
            read(dir.path(), dir.path(), "amx/fix-login-a1b", 1_030),
            prs,
            "a fresh answer is the answer, and no forge is asked at all"
        );
        assert!(
            !dir.path().join(format!("{CACHE}.new")).exists(),
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
        write(dir.path(), "amx/fix-login-a1b", prs.clone(), 1_000).unwrap();

        // Old enough that a look would set a fresh one going. A verb that
        // prints this and exits has nowhere to put the answer, so it takes
        // what is written down and leaves the forge alone: there is no tree
        // to ask in here at all.
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

        // A clock that has gone backwards is not a reason to stop reading what
        // is there.
        assert!(still_good(Some(&held), "amx/fix-login-a1b", 900));
    }

    #[test]
    fn a_look_stops_asking_about_a_branch_whose_requests_are_all_over() {
        // A merged request stays merged. A wall of finished agents would
        // otherwise put a network call behind every one of them once a minute,
        // for as long as somebody left the view open, to be told the same
        // thing every time.
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
            id: "fix-login-a1b".to_string(),
            task: "fix the login bug".to_string(),
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
            ask(dir.path(), "amx/fix-login-a1b").is_empty(),
            "and neither is a directory the forge has nothing to say about"
        );
    }

    #[test]
    fn hardening_a_forge_runs_nothing_the_tree_it_reads_names() {
        // Both forges shell out to git, and the tree they run in is the tree
        // the agent writes. `core.fsmonitor` names a program git starts before
        // it will look at a file, and a hook runs on the index it refreshes on
        // the way past; the environment is where an override reaches a git
        // amx is not the one running.
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
}
