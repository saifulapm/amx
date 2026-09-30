//! `amx send`: paste a message into a running agent's pane and submit it.
//!
//! After typing, the verb waits up to [`CONFIRM`] for the prompt to be taken:
//! the vendor's own prompt event, or for a vendor without hooks, a reading of
//! the pane that saw a turn begin. The refusal helpers here are shared with
//! `result`, `answer` and `interrupt`.
//!
//! - A waiting agent is refused: typed text would answer its question. The
//!   question and its choices go to stdout and the exit code is `BLOCKED`.
//! - A message containing bracketed-paste delimiters is refused; see
//!   [`ends_its_own_paste`].
//! - The send is appended to the event log and `seq` is bumped before anything
//!   is typed, so a concurrent `result` never takes the previous turn's answer
//!   for this one.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive::{self, View};
use crate::store::{Agent, Event, Kind, Meta, Phase, State};
use crate::tmux::{PaneId, Server};
use crate::vendor::{Capability, Hooks, Moment};
use crate::verbs::{answer, print_question};
use crate::{complain, exit, paths, registry, spawn, store, warn};

/// The event amx records for a message it sent.
pub const SEND: &str = "send";

/// Whether `event` is the vendor reporting that it took a prompt: its
/// `Prompted` or `Taken` moment.
///
/// This says the vendor has the message, which is what a send waits for. It
/// does not say the message was answered; see [`queued`].
fn submitted(hooks: Option<&Hooks>, event: &Event) -> bool {
    matches!(
        hooks.and_then(|hooks| hooks.moment(&event.kind)),
        Some(Moment::Prompted | Moment::Taken)
    )
}

/// How long a send waits for the agent to take the message.
const CONFIRM: Duration = Duration::from_secs(5);

/// How often the event log is polled while waiting.
const POLL: Duration = Duration::from_millis(50);

/// How often the pane is read for a vendor without hooks. A tmux capture costs
/// far more than reading the log, so it gets a slower clock.
const LOOK: Duration = Duration::from_millis(250);

/// Run the verb against the machine.
///
/// A `--file` that cannot be read is a usage error, reported before any record
/// is touched.
pub fn from_env(id: &str, text: Option<&str>, file: Option<&Path>) -> Result<i32> {
    let text = match file {
        Some(path) => match crate::cli::text_of(path) {
            Ok(text) => text,
            Err(refusal) => {
                warn!("amx send: {refusal}");
                return Ok(exit::USAGE);
            }
        },
        None => text.unwrap_or_default().to_string(),
    };

    let root = paths::state_root()?;
    let to_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut out = std::io::stdout().lock();
    run(&root, id, &text, to_terminal, &mut out)
}

/// The verb, with the state directory named.
pub fn run(
    root: &Path,
    id: &str,
    text: &str,
    to_terminal: bool,
    out: &mut impl Write,
) -> Result<i32> {
    let view = derive::view(root, id, store::now())?;
    // Checked before the phase: a parked record still reads whatever the agent
    // was doing when its pane was taken.
    if view.verdict.evidence == derive::Evidence::LetGo {
        complain!("amx: {}", was_let_go(id));
        return Ok(exit::FAILURE);
    }

    let phase = view.phase();
    match phase {
        Phase::Waiting => return waiting_on_a_question(&view, to_terminal, out),
        phase if phase.is_terminal() => return Ok(nothing_more_is_coming(id, phase)),
        _ => {}
    }

    let agent = Agent::open(root, id)?;
    // The confirmation is looked for only in what is appended after this
    // offset, so the wait never re-reads the whole log.
    let from = std::fs::metadata(agent.events_path()).map_or(0, |log| log.len());

    let server = Server::from_socket(view.meta.socket.clone());
    match delivered(&agent, &server, &view.meta.pane, text)? {
        Delivered::Sent => {}
        Delivered::Refused(why) => {
            complain!("amx: {why}");
            return Ok(exit::FAILURE);
        }
        Delivered::Waiting(state) => {
            return waiting_on_a_question(
                &View {
                    state: *state,
                    ..view
                },
                to_terminal,
                out,
            );
        }
    }

    // A working agent takes the message only when its current turn ends.
    if phase == Phase::Working {
        warn!("amx: {id} is working; the message is queued behind the turn it is on");
        return Ok(exit::OK);
    }

    if took_it(root, &view.meta, &agent, from, CONFIRM)? {
        return Ok(exit::OK);
    }
    complain!(
        "amx: {id} did not start working within {}s; the message may not have reached it",
        CONFIRM.as_secs()
    );
    Ok(exit::FAILURE)
}

/// Record the message, then paste it into the pane and press Enter.
///
/// The event and `seq` are written under the writer lock before anything is
/// typed, so a reader in another process knows the visible answer belongs to
/// the previous turn. The view sends through here too.
///
/// The write uses `observe`, which leaves the record's freshness alone: amx
/// sent the message, the agent said nothing. Bumping it would make readers
/// trust the pre-send record over the pane for [`derive::FRESH`] seconds.
pub fn deliver(agent: &Agent, server: &Server, pane: &PaneId, text: &str) -> Result<()> {
    match delivered(agent, server, pane, text)? {
        Delivered::Sent => Ok(()),
        Delivered::Refused(why) => bail!(why),
        Delivered::Waiting(_) => bail!(
            "{} is waiting on a question; nothing was typed at it",
            agent.id()
        ),
    }
}

/// Outcome of [`delivered`].
enum Delivered {
    Sent,
    /// Nothing typed or recorded, for the reason given.
    Refused(String),
    /// The record turned to waiting since the caller read it. Nothing typed or
    /// recorded; this is the current state.
    Waiting(Box<State>),
}

/// [`deliver`], returning a refusal or a new question instead of an error.
///
/// The caller's reading was taken without the lock, so the record is read
/// again under the writer, which is held until Enter is pressed. `_park` holds
/// the same lock from its check to its kill, so a send either lands first or
/// sees the parked stamp.
fn delivered(agent: &Agent, server: &Server, pane: &PaneId, text: &str) -> Result<Delivered> {
    if ends_its_own_paste(text) {
        bail!(
            "that message carries the end of a bracketed paste; \
             what follows it would be typed at `{}` rather than pasted into it",
            agent.id()
        );
    }

    let id = agent.id();
    let writer = agent.writer()?;
    let state = writer.state()?;
    if state.parked_at > 0 {
        return Ok(Delivered::Refused(was_let_go(id)));
    }
    if state.state == Phase::Waiting {
        return Ok(Delivered::Waiting(Box::new(state)));
    }
    if !state.composer_holds.is_empty() {
        return Ok(Delivered::Refused(held(id, &state.composer_holds)));
    }
    if !server.pane_answers_for(pane, id) {
        return Ok(Delivered::Refused(format!(
            "{id} has no pane any more; run: amx status {id}"
        )));
    }

    // Logged as given; typed with a trailing space if the last word opens a
    // popup.
    let vendor = agent.meta()?.agent.as_deref().and_then(registry::entry);
    writer.append(&Event::new(SEND, serde_json::json!({ "text": text })))?;
    writer.observe(|state| state.seq += 1)?;
    server.paste(pane, &spawn::as_typed(vendor, text))?;
    server.send_keys(pane, &["Enter"]).map(|()| Delivered::Sent)
}

/// The refusal for a composer an interrupt put queued text back into.
///
/// A paste would be appended to that text and submitted with it. The refusal
/// lasts until the vendor's next prompt.
fn held(id: &str, held: &[String]) -> String {
    let held: Vec<String> = held.iter().map(|text| format!("{text:?}")).collect();
    format!(
        "{id}'s composer still holds what the interrupt put back: {}; \
         a send now would go out with it. submit or clear it at the pane: amx attach {id}",
        held.join(", ")
    )
}

/// Whether `text` contains a bracketed-paste start or end sequence.
///
/// tmux wraps the paste in `ESC [ 200 ~` and `ESC [ 201 ~` without escaping
/// the text. An embedded end sequence ends the paste early, and the rest
/// arrives as keystrokes: a newline submits, an arrow moves a menu, a leading
/// slash runs a command. There is no way to escape it, so both sequences are
/// refused.
pub(crate) fn ends_its_own_paste(text: &str) -> bool {
    // `ESC [` and its 8-bit C1 equivalent.
    ["\u{1b}[", "\u{9b}"]
        .iter()
        .any(|csi| text.contains(&format!("{csi}200~")) || text.contains(&format!("{csi}201~")))
}

/// Wait up to `patience` for a prompt submission logged after byte `from`.
///
/// For a vendor without hooks nothing else reads the pane while the send
/// waits, so this takes a reading each round; the reading logs
/// [`derive::READ_PROMPT`] when it sees a turn begin.
fn took_it(root: &Path, meta: &Meta, agent: &Agent, from: u64, patience: Duration) -> Result<bool> {
    let deadline = Instant::now() + patience;
    let looking = only_a_reader_will_say(meta);
    let hooks = crate::vendor::hooks_for(meta.agent.as_deref().unwrap_or_default());
    loop {
        if looking {
            // A failed reading is just a missed look; the deadline covers it.
            let _ = derive::view(root, agent.id(), store::now());
        }
        if submissions(hooks.as_ref(), &events_from(agent, from)?) > 0 {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(match looking {
            true => LOOK,
            false => POLL,
        });
    }
}

/// The events appended to `agent`'s log from byte `from` on.
fn events_from(agent: &Agent, from: u64) -> Result<Vec<Event>> {
    use std::io::{Read, Seek, SeekFrom};
    let path = agent.events_path();
    let mut log = match std::fs::File::open(&path) {
        Ok(log) => log,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut tail = Vec::new();
    log.seek(SeekFrom::Start(from))
        .and_then(|_| log.read_to_end(&mut tail))
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(String::from_utf8_lossy(&tail)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Whether only a pane reading can confirm the send: the vendor has no hooks.
///
/// A command with no registry entry counts as a vendor with hooks, as in
/// `doctor`: it is usually a wrapper around one.
fn only_a_reader_will_say(meta: &Meta) -> bool {
    crate::registry::entry(meta.agent.as_deref().unwrap_or_default())
        .is_some_and(|vendor| !vendor.can(Capability::Hooks))
}

/// How many prompts the agent took: the vendor's own prompt events plus
/// [`derive::READ_PROMPT`] from pane readings.
///
/// Subagent events share the log and are not counted.
fn submissions(hooks: Option<&Hooks>, events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| {
            (submitted(hooks, event) || event.kind == derive::READ_PROMPT)
                && event.payload["agent_id"].is_null()
        })
        .count()
}

/// The text of every send the vendor still holds, oldest first, reading the
/// log with the record's own vendor's `hooks`.
///
/// Turn edges decide it. pi reports `Taken` when it starts on a message.
/// claude 2.1.278 fires `UserPromptSubmit` as soon as a mid-turn message is
/// queued, then folds it into the running turn with one `Stop` and no second
/// prompt event. So a prompt reported while a turn runs is held, and the end
/// of that turn answers it. A missed `Stop` errs towards "still queued"; a
/// reader's [`derive::READ_TURN_END`] clears it.
pub fn queued(hooks: Option<&Hooks>, events: &[Event]) -> Vec<String> {
    unanswered(hooks, events)
        .filter_map(|event| event.payload["text"].as_str().map(str::to_string))
        .collect()
}

/// [`queued`] for `meta`'s agent, minus messages its transcript says the
/// vendor took off its queue after they were sent.
///
/// claude fires no hook when it absorbs a queued message into the running
/// turn; the transcript's `queue-operation` line is the only record.
pub fn still_queued(meta: &Meta, events: &[Event]) -> Vec<String> {
    let hooks = crate::vendor::hooks_for(meta.agent.as_deref().unwrap_or_default());
    let pending: Vec<&Event> = unanswered(hooks.as_ref(), events).collect();
    if pending.is_empty() {
        return Vec::new();
    }
    let earliest = pending
        .iter()
        .map(|event| event.at)
        .min()
        .unwrap_or_default();
    let mut taken = match (
        &meta.transcript,
        crate::conversation::format_of(meta.agent.as_deref().unwrap_or_default()),
    ) {
        (Some(path), Some(format)) => crate::conversation::unqueued_since(format, path, earliest),
        _ => Vec::new(),
    };
    pending
        .into_iter()
        .filter_map(|event| {
            let text = event.payload["text"].as_str()?;
            let at = taken
                .iter()
                .position(|(when, words)| *when >= event.at && words.trim() == text.trim());
            match at {
                Some(at) => {
                    taken.remove(at);
                    None
                }
                None => Some(text.to_string()),
            }
        })
        .collect()
}

/// The `send` events on the log after the last turn edge.
fn unanswered<'a>(
    hooks: Option<&Hooks>,
    events: &'a [Event],
) -> impl Iterator<Item = &'a Event> + use<'a> {
    let mut running = false;
    let mut answered = 0;
    for (at, event) in events.iter().enumerate() {
        // Subagent events share the log.
        if !event.payload["agent_id"].is_null() {
            continue;
        }
        // Some(true) for a turn start, Some(false) for a turn end. Either
        // answers everything sent before it.
        let edge = match hooks.and_then(|hooks| hooks.moment(&event.kind)) {
            Some(Moment::Taken) => Some(true),
            // A prompt reported mid-turn is a message the vendor queued.
            Some(Moment::Prompted) if running => None,
            Some(Moment::Prompted) => Some(true),
            Some(Moment::Ended) => Some(false),
            _ if event.kind == derive::READ_PROMPT => Some(true),
            _ if event.kind == derive::READ_TURN_END => Some(false),
            // claude writes no `Stop` for an interrupted turn.
            _ if event.kind == crate::verbs::interrupt::INTERRUPT => Some(false),
            _ => None,
        };
        if let Some(began) = edge {
            running = began;
            answered = at + 1;
        }
    }
    events[answered..].iter().filter(|event| event.kind == SEND)
}

/// Exit `BLOCKED`, printing the pending question and its numbered choices on
/// stdout and the `amx answer` command on stderr.
///
/// A question whose text was never captured still blocks; stdout is empty.
pub fn waiting_on_a_question(view: &View, to_terminal: bool, out: &mut impl Write) -> Result<i32> {
    let id = view.id();
    if let Some(question) = &view.state.question {
        print_question(question, &view.state.options, to_terminal, out)?;
    }
    warn!(
        "amx: {id} is waiting on a question. answer it with `{}`",
        how_to_answer(view)
    );
    Ok(exit::BLOCKED)
}

/// The choices under a question, numbered from 1 as `amx answer` takes them.
///
/// Every surface prints choices through this.
pub fn numbered(options: &[String]) -> impl Iterator<Item = String> + '_ {
    options
        .iter()
        .enumerate()
        .map(|(at, label)| format!("{}. {label}", at + 1))
}

/// The `amx answer` command for this question, with the keys it accepts.
///
/// Every key offered must be one `answer` accepts for this screen.
pub fn how_to_answer(view: &View) -> String {
    format!(
        "amx answer {} {}",
        view.id(),
        takes(view.kind(), &view.state)
    )
}

/// The keys the current screen accepts, in usage-line form.
///
/// An unnumbered list takes a walk. A list amx numbered from the cursor mark
/// takes its digits and `esc`. A vendor question takes a digit, plus free
/// words unless it has previews. Anything else takes one key.
fn takes(kind: Option<Kind>, state: &State) -> String {
    if answer::unnumbered(kind, state) {
        return "<down enter|up enter|esc>".to_string();
    }
    let digits = answer::digits(state.options.len());
    if state.walked {
        return format!("<{digits}|esc>");
    }
    match kind {
        Some(Kind::Question) if answer::previewed(state.pending()) => {
            format!("<{digits}|enter|esc>")
        }
        Some(Kind::Question) => format!("<{digits}|\"words of your own\">"),
        _ => format!("<y|n|{digits}|enter|esc>"),
    }
}

/// Exit `FAILURE` for an agent that has ended, saying what to do next.
pub fn nothing_more_is_coming(id: &str, phase: Phase) -> i32 {
    complain!("amx: {id} is {phase}. {}", remedy(id, phase));
    exit::FAILURE
}

/// The refusal for a parked agent: its pane is gone, the session is intact,
/// and `amx resume` brings it back. See [`crate::verbs::park`].
fn was_let_go(id: &str) -> String {
    format!("{id} is parked; amx let its pane go. run: amx resume {id}")
}

/// What to do about an agent that ended in `phase`, as `ls` suggests it.
fn remedy(id: &str, phase: Phase) -> String {
    match phase {
        Phase::Stopped => format!("run: amx resume {id}"),
        Phase::Failed => format!("it ended badly; run: amx status {id}"),
        Phase::Done => "its command has ended".to_string(),
        _ => format!("run: amx status {id}"),
    }
}

/// Write text amx did not author, with the newline a terminal expects.
pub fn line(text: &str, out: &mut impl Write) -> Result<()> {
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

/// Vendor text for stdout: verbatim down a pipe, sanitized on a terminal so
/// escape sequences cannot drive it. Line breaks survive either way.
pub fn rendered(text: &str, to_terminal: bool) -> String {
    match to_terminal {
        true => crate::tmux::sanitize(text),
        false => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Ask, Choice};
    use crate::tmux::Socket;
    use serde_json::json;

    fn events(kinds: &[&str]) -> Vec<Event> {
        kinds
            .iter()
            .map(|kind| Event::new(*kind, json!({})))
            .collect()
    }

    /// A view of an agent waiting on a question.
    fn asking(question: Option<&str>, options: &[&str], kind: Option<Kind>) -> View {
        View {
            meta: Meta {
                role: None,
                parent: None,
                depth: 0,
                id: "fix-login-a1b".to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                model: None,
                effort: None,
                dir: std::path::PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: 1,
            },
            state: State {
                state: Phase::Waiting,
                question: question.map(str::to_string),
                options: options.iter().map(|label| label.to_string()).collect(),
                kind,
                ..State::default()
            },
            verdict: Verdict {
                phase: Phase::Waiting,
                evidence: Evidence::Hooks,
                rule: None,
                age: 3,
                worked: 3,
            },
            doing: None,
        }
    }

    #[test]
    fn hardening_a_message_may_not_end_its_own_paste() {
        assert!(!ends_its_own_paste(
            "fix the login bug\nand the tests with it"
        ));
        assert!(!ends_its_own_paste(
            "the escape \u{1b}[2J on its own is text"
        ));

        // Text after the end sequence would be typed, not pasted.
        assert!(ends_its_own_paste("done\u{1b}[201~/exit\r"));
        assert!(
            ends_its_own_paste("done\u{9b}201~/exit\r"),
            "including the one character an 8-bit terminal takes for ESC ["
        );
        assert!(
            ends_its_own_paste("\u{1b}[200~ another paste inside this one"),
            "and the end amx did not write is as bad as the start"
        );
    }

    #[test]
    fn hardening_a_message_amx_refuses_never_reaches_the_record() {
        // The record is written before typing, so the paste check must come
        // first or a refused send would still be logged and counted.
        let root = tempfile::TempDir::new().unwrap();
        let agent = Agent::create(root.path(), &asking(None, &[], None).meta).unwrap();
        let server = Server::from_socket(Socket::Name("amx-no-such-server".to_string()));

        let refused = deliver(
            &agent,
            &server,
            &PaneId::new("%1").unwrap(),
            "harmless\u{1b}[201~\u{1b}[B\r",
        )
        .unwrap_err();

        let said = format!("{refused:#}");
        assert!(said.contains("paste"), "{said}");
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
    }

    /// claude's prompt-submitted hook event.
    const SUBMITTED: &str = "UserPromptSubmit";

    #[test]
    fn send_counts_the_prompts_the_agent_itself_submitted() {
        let claude = Some(&crate::vendor::claude::HOOKS);
        assert_eq!(submissions(claude, &events(&[])), 0);
        assert_eq!(
            submissions(
                claude,
                &events(&["SessionStart", SUBMITTED, "Stop", SUBMITTED])
            ),
            2
        );

        // A reader's READ_PROMPT counts as a prompt; READ_TURN_END does not.
        assert_eq!(
            submissions(
                claude,
                &events(&[derive::READ_TURN_END, derive::READ_PROMPT])
            ),
            1,
            "and the other edge of a turn is not a prompt"
        );

        // Subagent prompts share the log and do not count.
        let mixed = vec![
            Event::new(SUBMITTED, json!({})),
            Event::new(SUBMITTED, json!({ "agent_id": "sub-1" })),
        ];
        assert_eq!(submissions(claude, &mixed), 1);
    }

    #[test]
    fn send_waits_only_on_what_the_log_gained_after_the_send() {
        let root = tempfile::TempDir::new().unwrap();
        let mut meta = asking(None, &[], None).meta;
        meta.agent = Some("claude".to_string());
        let agent = Agent::create(root.path(), &meta).unwrap();
        let log = |event: Event| agent.writer().unwrap().append(&event).unwrap();
        log(Event::new(SUBMITTED, json!({})));

        let from = std::fs::metadata(agent.events_path()).unwrap().len();
        assert!(events_from(&agent, from).unwrap().is_empty());
        assert!(
            !took_it(root.path(), &meta, &agent, from, Duration::ZERO).unwrap(),
            "a prompt from before the send is not its confirmation"
        );

        log(Event::new(SEND, json!({ "text": "and the linter" })));
        log(Event::new(SUBMITTED, json!({})));
        assert_eq!(events_from(&agent, from).unwrap().len(), 2);
        assert!(took_it(root.path(), &meta, &agent, from, Duration::ZERO).unwrap());
    }

    #[test]
    fn send_counts_a_prompt_only_in_the_records_own_vendors_words() {
        // Events are read in the record's own vendor's vocabulary.
        let pi = crate::vendor::pi::VENDOR.hooks;
        assert_eq!(
            submissions(pi.as_ref(), &events(&["agent_start", "message_start"])),
            2
        );
        assert_eq!(submissions(pi.as_ref(), &events(&[SUBMITTED])), 0);
        assert_eq!(submissions(None, &events(&[SUBMITTED])), 0);
    }

    #[test]
    fn send_lists_what_was_sent_after_the_last_prompt_was_taken() {
        let sent = |text: &str| Event::new(SEND, json!({ "text": text }));
        let claude = crate::vendor::claude::VENDOR.hooks;
        let pi = crate::vendor::pi::VENDOR.hooks;

        assert!(queued(claude.as_ref(), &[]).is_empty());
        // Nothing taken yet: everything sent is queued.
        assert_eq!(queued(claude.as_ref(), &[sent("carry on")]), ["carry on"]);
        // Two more sent after a prompt was taken, oldest first.
        assert_eq!(
            queued(
                claude.as_ref(),
                &[
                    sent("carry on"),
                    Event::new(SUBMITTED, json!({})),
                    sent("and the linter"),
                    sent("then the docs"),
                ]
            ),
            ["and the linter", "then the docs"]
        );
        // pi's `Taken` for a message steered into a running turn.
        assert_eq!(
            queued(
                pi.as_ref(),
                &[
                    sent("carry on"),
                    Event::new("message_start", json!({ "role": "user" }))
                ]
            ),
            Vec::<String>::new()
        );
        // A reader's READ_PROMPT is the same edge.
        assert_eq!(
            queued(
                claude.as_ref(),
                &[sent("carry on"), Event::new(derive::READ_PROMPT, json!({}))]
            ),
            Vec::<String>::new()
        );
        // A subagent's prompt does not take this agent's message.
        assert_eq!(
            queued(
                claude.as_ref(),
                &[
                    sent("carry on"),
                    Event::new(SUBMITTED, json!({ "agent_id": "sub-1" })),
                ]
            ),
            ["carry on"]
        );
    }

    #[test]
    fn send_goes_on_listing_a_message_the_vendor_is_only_holding() {
        let sent = |text: &str| Event::new(SEND, json!({ "text": text }));
        let claude = crate::vendor::claude::VENDOR.hooks;
        let pi = crate::vendor::pi::VENDOR.hooks;
        let ended = || Event::new("Stop", json!({}));

        // claude 2.1.278 fires UserPromptSubmit as soon as a mid-turn message
        // is queued, and fires no second prompt event when it takes it up. A
        // prompt event mid-turn means the message is held.
        let holding = vec![
            Event::new(SUBMITTED, json!({})),
            sent("and the linter"),
            Event::new(SUBMITTED, json!({})),
        ];
        assert_eq!(queued(claude.as_ref(), &holding), ["and the linter"]);

        // The end of the turn answers it.
        let mut answered = holding.clone();
        answered.push(ended());
        assert!(queued(claude.as_ref(), &answered).is_empty());

        // With no turn running, the next prompt starts a turn of its own.
        let mut again = answered.clone();
        again.extend([sent("then the docs"), Event::new(SUBMITTED, json!({}))]);
        assert!(queued(claude.as_ref(), &again).is_empty());

        // A reader's READ_TURN_END also ends the turn, covering a missed Stop.
        let mut watched = holding.clone();
        watched.push(Event::new(derive::READ_TURN_END, json!({})));
        assert!(queued(claude.as_ref(), &watched).is_empty());

        // An interrupt ends the turn; claude writes no Stop for it.
        let cut = vec![
            Event::new(SUBMITTED, json!({})),
            Event::new(crate::verbs::interrupt::INTERRUPT, json!({})),
            sent("change of direction"),
            Event::new(SUBMITTED, json!({})),
        ];
        assert!(queued(claude.as_ref(), &cut).is_empty());

        // pi's `Taken` means it started on the message, whenever it arrives.
        let steered = vec![
            Event::new("agent_start", json!({})),
            sent("and the linter"),
            Event::new("message_start", json!({ "role": "user" })),
        ];
        assert!(queued(pi.as_ref(), &steered).is_empty());
    }

    #[test]
    fn send_reads_what_is_queued_in_the_records_own_vendors_words() {
        let sent = |text: &str| Event::new(SEND, json!({ "text": text }));
        let second = crate::vendor::second::HOOKS;

        // claude's event name means nothing on a `second` record.
        assert_eq!(
            queued(
                Some(&second),
                &[sent("carry on"), Event::new(SUBMITTED, json!({}))]
            ),
            ["carry on"]
        );
        // Its own prompt event does.
        assert!(
            queued(
                Some(&second),
                &[sent("carry on"), Event::new("told", json!({}))]
            )
            .is_empty()
        );
    }

    #[test]
    fn a_message_claude_folded_into_the_running_turn_is_no_longer_queued() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        // 2026-09-29T20:32:04Z is 1790713924.
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"queue-operation\",\"operation\":\"remove\",\"timestamp\":\"2026-09-29T20:00:00.000Z\",\"content\":\"check the tests\",\"reason\":\"absorbed_mid_turn\"}\n",
                "{\"type\":\"queue-operation\",\"operation\":\"enqueue\",\"timestamp\":\"2026-09-29T20:27:34.348Z\",\"content\":\"why MEM_PROJECT?\"}\n",
                "{\"type\":\"queue-operation\",\"operation\":\"remove\",\"timestamp\":\"2026-09-29T20:32:04.768Z\",\"content\":\"why MEM_PROJECT?\",\"reason\":\"absorbed_mid_turn\"}\n",
            ),
        )
        .unwrap();
        let mut meta = asking(None, &[], None).meta;
        meta.agent = Some("claude".to_string());
        meta.transcript = Some(transcript);
        let sent = |text: &str, at: u64| {
            let mut event = Event::new(SEND, json!({ "text": text }));
            event.at = at;
            event
        };
        let events = [
            Event::new("UserPromptSubmit", json!({})),
            sent("why MEM_PROJECT? ", 1_790_713_654),
            Event::new("UserPromptSubmit", json!({})),
            sent("check the tests", 1_790_713_700),
            Event::new("UserPromptSubmit", json!({})),
        ];

        // The first was absorbed after it was sent. The second matches only an
        // absorption from before it was sent, so it is still queued.
        assert_eq!(still_queued(&meta, &events), ["check the tests"]);
        meta.transcript = None;
        assert_eq!(
            still_queued(&meta, &events),
            ["why MEM_PROJECT? ", "check the tests"]
        );
    }

    #[test]
    fn send_asks_the_vendor_whether_anything_else_will_ever_say_so() {
        let asked = |agent: Option<&str>| {
            let mut meta = asking(None, &[], None).meta;
            meta.agent = agent.map(str::to_string);
            only_a_reader_will_say(&meta)
        };

        // claude reports through its hooks, pi through amx's extension.
        assert!(!asked(Some("claude")), "claude says it itself");
        assert!(!asked(Some("pi")), "and so does pi, through its extension");
        // An unknown command, or a record without the field, counts as
        // reporting.
        assert!(!asked(Some("some-tool --flag")));
        assert!(!asked(None));
    }

    #[test]
    fn send_puts_a_pending_question_where_the_answer_would_have_gone() {
        let mut out = Vec::new();
        let code = waiting_on_a_question(
            &asking(
                Some("Claude needs your permission to use Bash"),
                &["Yes", "No"],
                None,
            ),
            false,
            &mut out,
        )
        .unwrap();

        assert_eq!(code, exit::BLOCKED);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Claude needs your permission to use Bash\n1. Yes\n2. No\n",
            "the choices are what the answer has to be one of"
        );
    }

    #[test]
    fn a_question_nobody_captured_still_blocks() {
        let mut out = Vec::new();
        let code = waiting_on_a_question(&asking(None, &[], None), false, &mut out).unwrap();
        assert_eq!(code, exit::BLOCKED);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn surfaces_the_offer_says_what_this_kind_of_question_will_take() {
        // A permission box and the trust screen take one key, never words.
        for kind in [None, Some(Kind::Permission), Some(Kind::Trust)] {
            let offered = how_to_answer(&asking(Some("Proceed?"), &["Yes", "No"], kind));
            assert!(offered.contains("y|n|1-2"), "{offered}");
            assert!(!offered.contains("words"), "{offered}");
        }

        let menu = how_to_answer(&asking(
            Some("Which fixture?"),
            &["the sqlite one", "the docker one"],
            Some(Kind::Question),
        ));
        assert!(menu.contains("words of your own"), "{menu}");
        assert!(menu.starts_with("amx answer fix-login-a1b"), "{menu}");
    }

    #[test]
    fn surfaces_the_offer_names_the_keys_that_move_the_screen_it_was_read_off() {
        // The digit range matches the number of choices read.
        let box_of_two = how_to_answer(&asking(Some("Proceed?"), &["Yes", "No"], None));
        assert!(box_of_two.contains("1-2"), "{box_of_two}");
        assert!(!box_of_two.contains("1-9"), "{box_of_two}");

        // With no choices read, offer `1-9`.
        let unread = how_to_answer(&asking(Some("Proceed?"), &[], Some(Kind::Permission)));
        assert!(unread.contains("1-9"), "{unread}");

        // claude 2.1.259's folder-trust gate numbers no rows and opens on the
        // exit row, so only a walk is offered.
        let gate = how_to_answer(&asking(Some("Quick safety check"), &[], Some(Kind::Trust)));
        assert_eq!(gate, "amx answer fix-login-a1b <down enter|up enter|esc>");

        // A trust screen with numbered rows takes the usual keys.
        let numbered = how_to_answer(&asking(
            Some("Trust this folder?"),
            &["Yes", "No"],
            Some(Kind::Trust),
        ));
        assert!(numbered.contains("y|n|1-2|enter|esc"), "{numbered}");
    }

    #[test]
    fn surfaces_a_list_amx_numbered_itself_is_offered_the_digits_amx_wrote() {
        // pi draws blocking lists with a cursor arrow and no numbers. amx
        // numbers the rows itself and `answer` walks to them, so only those
        // digits and `esc` are offered; the selector ignores `y`, `n` and
        // `enter`.
        let mut gate = asking(
            Some("Trust project folder? /srv/app"),
            &[
                "Trust",
                "Trust parent folder (/srv)",
                "Trust (this session only)",
                "Do not trust",
                "Do not trust (this session only)",
            ],
            Some(Kind::Trust),
        );
        gate.state.walked = true;
        assert_eq!(how_to_answer(&gate), "amx answer fix-login-a1b <1-5|esc>");

        // pi's tool gate is a question and takes the same keys.
        let mut dialog = asking(
            Some("Allow pi to run `rm -rf build`?"),
            &["Allow once", "Allow always", "Deny"],
            Some(Kind::Question),
        );
        dialog.state.walked = true;
        assert_eq!(how_to_answer(&dialog), "amx answer fix-login-a1b <1-3|esc>");
    }

    #[test]
    fn surfaces_a_question_drawn_beside_a_preview_is_offered_no_words() {
        // claude 2.1.240 draws no free-text row beside previews, and `answer`
        // refuses words there.
        let mut view = asking(Some("Which layout?"), &[], Some(Kind::Question));
        view.state.asks_all(vec![Ask {
            header: None,
            text: "Which layout?".to_string(),
            options: vec![
                Choice {
                    label: "Stacked".to_string(),
                    description: None,
                    preview: Some("| TITLE |".to_string()),
                },
                Choice {
                    label: "Inline".to_string(),
                    description: None,
                    preview: Some("| TITLE - subtitle |".to_string()),
                },
            ],
            multi: false,
            answer: None,
        }]);

        let offered = how_to_answer(&view);
        assert_eq!(offered, "amx answer fix-login-a1b <1-2|enter|esc>");
    }

    #[test]
    fn surfaces_the_choices_are_numbered_the_way_the_screen_numbers_them() {
        let options = ["the sqlite one".to_string(), "the docker one".to_string()];
        assert_eq!(
            numbered(&options).collect::<Vec<_>>(),
            ["1. the sqlite one", "2. the docker one"]
        );
        assert_eq!(numbered(&[]).count(), 0);
    }

    #[test]
    fn send_says_so_when_amx_has_let_the_agents_pane_go() {
        // A parked agent reads idle, so the phase check alone would let the
        // send through. The record must stay untouched.
        let root = tempfile::TempDir::new().unwrap();
        let meta = Meta {
            parent: None,
            depth: 0,
            socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
            pane: PaneId::new("%404").unwrap(),
            ..asking(None, &[], None).meta
        };
        let agent = Agent::create(root.path(), &meta).unwrap();
        agent
            .writer()
            .unwrap()
            .observe(|state| {
                state.state = Phase::Idle;
                state.parked_at = 4_600;
            })
            .unwrap();

        let mut out = Vec::new();
        let code = run(root.path(), &meta.id, "and now the linter", false, &mut out).unwrap();
        assert_eq!(code, exit::FAILURE);
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");

        // The refusal names the command that brings it back.
        let said = was_let_go(&meta.id);
        assert!(said.contains("parked"), "{said}");
        assert!(said.contains("amx resume fix-login-a1b"), "{said}");
    }

    #[test]
    fn every_ending_says_what_to_do_about_it() {
        for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
            assert_eq!(
                nothing_more_is_coming("fix-login-a1b", phase),
                exit::FAILURE
            );
            assert!(
                !remedy("fix-login-a1b", phase).is_empty(),
                "{phase} says nothing"
            );
        }
        assert!(remedy("fix-login-a1b", Phase::Stopped).contains("amx resume fix-login-a1b"));
    }

    #[test]
    fn vendor_text_down_a_pipe_is_the_bytes_the_vendor_wrote() {
        let said = "done\u{1b}]0;PWNED\u{7}\n\tindented\n";
        assert_eq!(rendered(said, false), said);
    }

    #[test]
    fn vendor_text_on_a_terminal_cannot_drive_the_terminal_it_prints_into() {
        let shown = rendered("done\u{1b}]0;PWNED\u{7}\nand more\n", true);
        assert!(shown.contains("done"), "{shown:?}");
        assert!(shown.contains("]0;PWNED"), "still readable: {shown:?}");
        assert!(
            shown.contains("\nand more"),
            "the line breaks are the agent's"
        );
        assert_eq!(
            shown
                .chars()
                .filter(|c| c.is_control() && *c != '\n')
                .count(),
            0,
            "and inert: {shown:?}"
        );
    }

    #[test]
    fn a_line_ends_in_one_newline_however_it_arrived() {
        let mut out = Vec::new();
        line("no newline", &mut out).unwrap();
        line("has one\n", &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "no newline\nhas one\n");
    }

    /// A `cat` pane on a private tmux server, in a session named as
    /// [`crate::spawn::place`] names one so it answers for `fix-login-a1b`.
    /// Killed on drop.
    struct Listening {
        server: Server,
        pane: PaneId,
    }

    impl Listening {
        fn new(test: &str) -> Listening {
            let name = format!("amx-test-send-{test}-{}", std::process::id());
            let server = Server::named(&name).with_conf("/dev/null");
            let (_, pane) = server
                .new_session(&crate::tmux::Spawn {
                    name: Some(&format!("{}fix-login-a1b", crate::tmux::SESSION_PREFIX)),
                    command: &["cat"],
                    ..crate::tmux::Spawn::default()
                })
                .expect("a pane for it");
            Listening { server, pane }
        }

        /// An agent recorded on this pane, with `change` applied to its state.
        fn agent(&self, root: &Path, change: impl FnOnce(&mut State)) -> Agent {
            let meta = Meta {
                socket: self.server.socket().clone(),
                pane: self.pane.clone(),
                ..asking(None, &[], None).meta
            };
            let agent = Agent::create(root, &meta).unwrap();
            agent.writer().unwrap().observe(change).unwrap();
            agent
        }

        /// Whether anything was typed at the pane.
        fn typed_at(&self) -> bool {
            let screen = self.server.capture(&self.pane).unwrap_or_default();
            !screen.trim().is_empty()
        }
    }

    impl Drop for Listening {
        fn drop(&mut self) {
            let _ = self.server.kill();
        }
    }

    #[test]
    fn send_refuses_a_record_parked_since_it_was_read() {
        // Parked between the caller's reading and the paste: the record under
        // the writer lock wins.
        let root = tempfile::TempDir::new().unwrap();
        let pane = Listening::new("parked");
        let agent = pane.agent(root.path(), |state| {
            state.state = Phase::Idle;
            state.parked_at = 4_600;
        });

        let refused = deliver(&agent, &pane.server, &pane.pane, "and the linter").unwrap_err();
        assert!(format!("{refused:#}").contains("parked"), "{refused:#}");
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!pane.typed_at(), "and nothing is typed");
    }

    #[test]
    fn send_refuses_a_pane_that_no_longer_answers_for_the_agent() {
        let root = tempfile::TempDir::new().unwrap();
        let meta = Meta {
            socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
            pane: PaneId::new("%404").unwrap(),
            ..asking(None, &[], None).meta
        };
        let agent = Agent::create(root.path(), &meta).unwrap();
        let server = Server::from_socket(meta.socket.clone());

        assert!(deliver(&agent, &server, &meta.pane, "and the linter").is_err());
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
    }

    #[test]
    fn send_to_a_record_now_waiting_presses_nothing_and_hands_back_the_question() {
        // A question appeared between the caller's reading and the paste.
        let root = tempfile::TempDir::new().unwrap();
        let pane = Listening::new("waiting");
        let agent = pane.agent(root.path(), |state| {
            state.state = Phase::Waiting;
            state.question = Some("Run the migration?".to_string());
        });

        match delivered(&agent, &pane.server, &pane.pane, "and the linter").unwrap() {
            Delivered::Waiting(state) => {
                assert_eq!(state.question.as_deref(), Some("Run the migration?"))
            }
            _ => panic!("the record is waiting"),
        }
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!pane.typed_at(), "and no key is pressed");
    }

    #[test]
    fn send_refuses_while_the_composer_holds_what_an_interrupt_put_back() {
        // pi puts queued messages back in its composer when a turn is
        // cancelled; a paste would be submitted with them.
        let root = tempfile::TempDir::new().unwrap();
        let pane = Listening::new("holds");
        let agent = pane.agent(root.path(), |state| {
            state.state = Phase::Idle;
            state.composer_holds = vec!["and the linter".to_string()];
        });

        let refused = deliver(&agent, &pane.server, &pane.pane, "and the tests").unwrap_err();
        assert!(
            format!("{refused:#}").contains("and the linter"),
            "{refused:#}"
        );
        assert_eq!(agent.state().unwrap().seq, 0, "no send is counted");
        assert!(agent.events().unwrap().is_empty(), "and none is logged");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!pane.typed_at(), "and nothing is typed");
    }

    #[test]
    fn send_types_at_a_pane_that_answers_for_an_idle_agent() {
        let root = tempfile::TempDir::new().unwrap();
        let pane = Listening::new("idle");
        let agent = pane.agent(root.path(), |state| state.state = Phase::Idle);

        deliver(&agent, &pane.server, &pane.pane, "and the linter").unwrap();
        assert_eq!(agent.state().unwrap().seq, 1);
        assert_eq!(agent.events().unwrap().len(), 1);
    }
}
