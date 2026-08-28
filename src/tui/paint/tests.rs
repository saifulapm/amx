//! What the screen says, read back without a terminal anywhere near it.
//!
//! The bands are drawn together and asserted on together, because what these
//! are about is the frame a person is looking at: the surfaces are carved
//! apart so each is readable, and the screen they make is one thing.

use super::card::{ALL_CHROME, body, choices, tail, walks};
use super::empty::WELCOME;
use super::header::{SHORT, SPACED};
use super::help::GROUPS;
use super::input::{ANSWERS, composer_lines};
use super::style::request_colour;
use super::text::fit;
use super::wall::{LIVE, pulse, resting, set, set_for};
use super::*;
use crate::derive::{self, Evidence, Verdict, View};
use crate::furniture::cut;
use crate::pr::{Pr, Standing};
use crate::store::{Kind, Meta, Phase, State};
use crate::theme::Theme;
use crate::tmux::{PaneId, Socket};
use crate::tui::Arm;
use crate::tui::act::{Asking, Composer};
use crate::tui::rows::{Group, Narrow};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::path::PathBuf;
use std::time::Instant;

/// The palette a screen nobody handed a theme is painted in, which is the
/// one every screen built here has and the one these colours are read out
/// of: what the tests are about is which role a thing is painted in, and
/// the values are the theme's business.
fn theme() -> Theme {
    Theme::default()
}

fn view(id: &str, phase: Phase, said: Option<&str>, age: u64) -> View {
    View {
        meta: Meta {
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            dir: PathBuf::from("/srv/app"),
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
            state: phase,
            summary: said.map(str::to_string),
            since: 1,
            last_event: 1,
            ..State::default()
        },
        verdict: Verdict {
            phase,
            evidence: Evidence::Hooks,
            rule: None,
            age,
            // The rows print the worked seconds; most of these tests only
            // care that a number is where the column is, so the helper
            // hands both clocks the same one.
            worked: age,
        },
    }
}

/// Every state there is, so a table of marks cannot quietly miss one.
const EVERY: [Phase; 8] = [
    Phase::Starting,
    Phase::Working,
    Phase::Waiting,
    Phase::Idle,
    Phase::Done,
    Phase::Failed,
    Phase::Stopped,
    Phase::Unknown,
];

/// The view, with a reading in it. The card is read as it is planted,
/// the way the view itself builds one.
fn showing(views: Vec<View>, card: Option<Card>) -> Screen {
    let mut screen = Screen::default();
    screen.list.show(views);
    screen.card = card.map(Card::read);
    screen
}

/// The card a waiting agent's row opens: what it is asking, the choices it
/// offers, and the screen it is asking on.
fn asking(options: &[&str], kind: Option<Kind>) -> Card {
    Card {
        id: "ask-a1b".to_string(),
        phase: Phase::Waiting,
        age: 29,
        question: Some("Which fixture should the port keep?".to_string()),
        options: options.iter().map(|label| (*label).to_string()).collect(),
        kind,
        body: "$ cargo test\nDo you want to proceed?".to_string(),
        changes: false,
        answer: false,
    }
}

/// The same reading, running somewhere else.
fn at(mut view: View, dir: &str) -> View {
    view.meta.dir = PathBuf::from(dir);
    view
}

/// The same reading, on a branch of its own.
fn on_a_branch(mut view: View, branch: &str) -> View {
    view.meta.branch = Some(branch.to_string());
    view
}

/// A forge holding one failing request for the agent that is asking, and
/// two for the one beside it — the second attempt and the first.
fn a_forge(meta: &crate::store::Meta) -> Vec<Pr> {
    match meta.branch.as_deref() {
        Some("amx/ask-a1b") => vec![Pr {
            number: 12,
            standing: Standing::Failing,
        }],
        Some("amx/busy-b2c") => vec![
            Pr {
                number: 40,
                standing: Standing::Open,
            },
            Pr {
                number: 7,
                standing: Standing::Merged,
            },
        ],
        _ => Vec::new(),
    }
}

/// The view over that forge.
fn over_the_forge(views: Vec<View>, card: Option<Card>) -> Screen {
    let mut screen = Screen::default();
    screen.list.asking(a_forge);
    screen.list.show(views);
    screen.card = card.map(Card::read);
    screen
}

/// The view with the agents gathered by where they are running.
fn by_project(views: Vec<View>) -> Screen {
    let mut screen = Screen::default();
    screen.list.turn();
    screen.list.show(views);
    screen
}

/// What a view of this size draws, cell by cell.
fn cells(screen: &Screen, size: (u16, u16)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
    terminal.draw(|frame| draw(frame, screen)).unwrap();
    terminal.backend().buffer().clone()
}

/// The mark on a row, and how the view painted it: a mark carries its
/// colour, and a test that read the text alone could not see it.
fn mark(screen: &Screen, size: (u16, u16), row: u16) -> (String, Color, Modifier) {
    let cell = cells(screen, size)[(2, row)].clone();
    (cell.symbol().to_string(), cell.fg, cell.modifier)
}

/// What a view of this size puts on the screen, line by line.
fn painted(screen: &Screen, size: (u16, u16)) -> Vec<String> {
    let buffer = cells(screen, size);
    (0..size.1)
        .map(|row| {
            (0..size.0)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// What the view puts on a screen of this size, line by line.
fn drawn(views: Vec<View>, card: Option<Card>, size: (u16, u16)) -> Vec<String> {
    painted(&showing(views, card), size)
}

/// What a heading line says in front of the rule that carries it out to
/// the edge: the label, and how many failed under it where any did.
fn heading_of(line: &str) -> &str {
    line.split('─').next().unwrap_or_default().trim()
}

/// And the count it ends in, which is the last thing on the line.
fn counted(line: &str) -> &str {
    line.split_whitespace().next_back().unwrap_or_default()
}

/// The same, once the list has learned the screen's size: the first
/// frame writes the room back the way the loop's draw does, the refit
/// lays the rows out for it, and the second frame is the one a person
/// reads.
fn settled(views: Vec<View>, size: (u16, u16)) -> Vec<String> {
    let mut screen = showing(views, None);
    let _ = painted(&screen, size);
    screen.list.refit();
    painted(&screen, size)
}

/// The two agents a card is opened over, so there is a list to still be
/// drawn behind it.
fn a_fleet() -> Vec<View> {
    vec![
        view("ask-a1b", Phase::Waiting, None, 29),
        view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
    ]
}

#[test]
fn card_floats_a_bordered_box_over_the_still_drawn_list() {
    let screen = drawn(
        a_fleet(),
        Some(asking(
            &["the sqlite one", "the docker one"],
            Some(Kind::Question),
        )),
        (60, 14),
    );

    assert_eq!(heading_of(&screen[3]), "NEEDS INPUT", "{screen:?}");
    assert!(
        screen[4].contains("ask-a1b"),
        "the row the card was opened from is still on the screen: {screen:?}"
    );

    let top = screen
        .iter()
        .position(|line| line.starts_with('╭'))
        .expect("the top of the card");
    assert!(
        screen[top].contains("ask-a1b · waiting 29s"),
        "which agent, what it is doing, and how long since: {:?}",
        screen[top]
    );
    assert!(screen[top].ends_with('╮'), "{:?}", screen[top]);
    assert!(
        screen[top + 1].contains("Which fixture should the port keep?"),
        "{:?}",
        screen[top + 1]
    );
    assert!(
        !screen.iter().any(|line| line.contains("Do you want to")),
        "and the pane it is asking on is not echoed under it: {screen:?}"
    );

    let bottom = screen
        .iter()
        .rposition(|line| line.starts_with('╰'))
        .expect("the foot of the card");
    assert!(screen[bottom].ends_with('╯'), "{:?}", screen[bottom]);
    assert_eq!(
        bottom + 2,
        screen.len(),
        "and the hint row is the one beneath it: {screen:?}"
    );
}

#[test]
fn card_numbers_the_choices_the_question_offers() {
    let screen = drawn(
        a_fleet(),
        Some(asking(
            &["the sqlite one", "the docker one"],
            Some(Kind::Question),
        )),
        (60, 14),
    );
    assert!(
        screen
            .iter()
            .any(|line| line.contains("1. the sqlite one   2. the docker one")),
        "numbered the way every surface numbers them: {screen:?}"
    );
}

/// The same card, with somebody part way through typing the answer to it.
fn answering(card: Card, typed: &str) -> Screen {
    let mut screen = showing(a_fleet(), Some(card));
    let mut composer = Composer::new(Asking::Reply {
        id: "ask-a1b".to_string(),
        question: true,
    });
    composer.text = typed.to_string();
    screen.mode = Mode::Typing(composer);
    screen
}

/// The row of the card the answer is typed on.
fn answer_row(screen: &[String]) -> String {
    screen
        .iter()
        .find(|line| line.contains('❯'))
        .unwrap_or_else(|| panic!("no row to answer on in: {screen:?}"))
        .clone()
}

#[test]
fn card_takes_the_answer_on_a_row_of_the_card_itself() {
    let question = || asking(&["the sqlite one", "the docker one"], Some(Kind::Question));

    let empty = painted(&answering(question(), ""), (60, 14));
    assert!(
        answer_row(&empty).contains("❯ press 1-2, or type an answer"),
        "an empty row says what the question will take: {:?}",
        answer_row(&empty)
    );
    assert_eq!(
        empty[13], ANSWERS,
        "and the row under the card says what its own keys do"
    );

    let typed = painted(&answering(question(), "the docker one"), (60, 14));
    assert!(
        answer_row(&typed).contains("❯ the docker one"),
        "{:?}",
        answer_row(&typed)
    );
    assert!(
        !typed.iter().any(|line| line.contains("type an answer")),
        "what was typed takes the row the invitation had: {typed:?}"
    );
    assert!(
        !typed.iter().any(|line| line.starts_with("answer ask-a1b")),
        "and the line is on the card rather than on a band of its own \
         under it: {typed:?}"
    );
    assert_eq!(
        caret(&answering(question(), "the docker one"), (60, 14)),
        (18, 11),
        "with the terminal's own cursor at the end of what was typed, on \
         a card that is the question block's own size"
    );
}

#[test]
fn card_is_no_taller_than_what_it_has_to_show() {
    // An agent whose answer is one line does not want seven rows of box to
    // say it in, and every row the card leaves is a row of the wall.
    let brief = Card {
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        body: "did what it was asked".to_string(),
        ..asking(&[], None)
    };
    let screen = drawn(a_fleet(), Some(brief), (60, 20));
    let top = screen
        .iter()
        .position(|line| line.starts_with('╭'))
        .expect("the top of the card");
    let bottom = screen
        .iter()
        .rposition(|line| line.starts_with('╰'))
        .expect("the foot of the card");

    assert_eq!(
        bottom - top,
        2,
        "two borders and the one line it has: {screen:?}"
    );
    assert!(
        screen[top + 1].contains("did what it was asked"),
        "{screen:?}"
    );
    assert_eq!(
        screen[top - 1],
        "",
        "with the rows it is not covering behind it: {screen:?}"
    );
}

#[test]
fn card_keeps_the_row_being_typed_on_when_there_is_room_for_little_else() {
    // A card with one row inside its borders. What somebody is typing is
    // what that row is for: the question is on the agent's row behind the
    // card, and the line is nowhere else at all.
    let screen = painted(
        &answering(asking(&["the sqlite one"], Some(Kind::Question)), "the sq"),
        (60, 6),
    );
    assert!(answer_row(&screen).contains("❯ the sq"), "{screen:?}");
    assert_eq!(screen[5], ANSWERS, "with the card's own keys under it");
}

#[test]
fn card_invites_only_the_answers_the_question_will_take() {
    // A permission box has no field for words: they would land on whatever
    // is highlighted, which is an answer nobody chose.
    let box_office = Card {
        kind: Some(Kind::Permission),
        question: Some("Claude needs your permission to use Bash".to_string()),
        ..asking(&["Yes", "No"], None)
    };
    let asked = answer_row(&painted(&answering(box_office, ""), (60, 14)));
    assert!(asked.contains("❯ press 1-2, y or n"), "{asked:?}");
    assert!(
        !asked.contains("type"),
        "a hint that offers what the prompt will refuse is a hint that \
         lies: {asked:?}"
    );

    // And a card nobody is answering has the list's own keys under it.
    let looking = painted(&showing(a_fleet(), Some(asking(&[], None))), (60, 14));
    assert_eq!(
        looking[13],
        "space closes it · enter attach · ctrl+x stop · ? keys"
    );
    assert!(
        !looking.iter().any(|line| line.contains('❯')),
        "with no row to answer on: {looking:?}"
    );
}

#[test]
fn card_packs_the_choices_onto_as_few_rows_as_it_is_wide() {
    let two = ["the sqlite one".to_string(), "the docker one".to_string()];
    assert_eq!(
        choices(&two, 40, false),
        ["1. the sqlite one   2. the docker one"]
    );
    assert_eq!(
        choices(&two, 20, false),
        ["1. the sqlite one", "2. the docker one"],
        "and one to a row where they will not sit together"
    );
    assert_eq!(
        choices(&two, 10, false),
        ["1. the sq…", "2. the do…"],
        "a choice wider than the card is cut, and says it was"
    );
    assert!(choices(&[], 40, false).is_empty());
}

#[test]
fn card_gives_a_question_that_takes_several_a_box_beside_every_choice() {
    let two = ["the sqlite one".to_string(), "the docker one".to_string()];
    assert_eq!(
        choices(&two, 50, true),
        ["1. [ ] the sqlite one   2. [ ] the docker one"],
        "the vendor's own box, between the number and the label"
    );
    assert_eq!(
        choices(&two, 20, true),
        ["1. [ ] the sqlite o…", "2. [ ] the docker o…"],
        "and a narrow card cuts the label rather than the box, because the \
         box is what says the row is one"
    );
}

#[test]
fn glyphs_give_every_state_a_mark_of_its_own() {
    let marks: Vec<&str> = EVERY.iter().map(|phase| resting(*phase)).collect();
    assert_eq!(
        marks
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        EVERY.len(),
        "eight states, eight marks: {marks:?}"
    );
    assert_eq!(resting(Phase::Waiting), "?");
    assert_eq!(resting(Phase::Starting), "◌");
    assert_eq!(resting(Phase::Idle), "○");
    assert_eq!(resting(Phase::Done), "●");
    assert_eq!(resting(Phase::Failed), "✗");
    assert_eq!(resting(Phase::Stopped), "⏹");
    assert_eq!(resting(Phase::Unknown), "~");

    for phase in EVERY.iter().filter(|phase| **phase != Phase::Working) {
        assert!(
            !set().contains(&resting(*phase)),
            "{phase} rests on a mark the pulse passes through"
        );
    }
}

#[test]
fn glyphs_pulse_a_working_row_through_twelve_frames() {
    let set = set();
    let want: Vec<&str> = set.iter().chain(set.iter().rev()).copied().collect();
    let frames: Vec<&str> = (0..12).map(pulse).collect();

    assert_eq!(frames, want, "the set, and then the set backwards");
    assert_eq!(pulse(12), pulse(0), "and round again");
    assert_eq!(
        resting(Phase::Working),
        set[LIVE],
        "and it rests on the vendor's own live glyph"
    );
}

#[test]
fn glyphs_take_the_set_the_terminal_asks_for() {
    assert_eq!(set_for("xterm-ghostty"), ["·", "✢", "✳", "✶", "✻", "✻"]);
    assert_eq!(set_for("tmux-256color"), ["·", "✢", "*", "✶", "✻", "✽"]);
    assert_eq!(
        set_for(""),
        set_for("xterm"),
        "and anything else is the same"
    );
}

#[test]
fn glyphs_leave_the_colour_to_say_how_it_went() {
    // The mark on the one row a view of one agent draws.
    let painted = |phase| {
        let screen = showing(vec![view("agent-a1b", phase, Some("said"), 5)], None);
        mark(&screen, (60, 8), 2)
    };
    let plain = Modifier::empty();

    // The one glyph with weight on it, which is the one the view is
    // opened to find.
    assert_eq!(
        painted(Phase::Waiting),
        ("?".into(), theme().waiting, Modifier::BOLD)
    );
    assert_eq!(
        painted(Phase::Unknown),
        ("~".into(), theme().waiting, plain)
    );
    assert_eq!(painted(Phase::Done), ("●".into(), theme().done, plain));
    assert_eq!(painted(Phase::Failed), ("✗".into(), theme().failed, plain));
    assert_eq!(
        painted(Phase::Stopped),
        ("⏹".into(), theme().stopped, plain)
    );

    // An agent still at work has nothing to say about how it went, so it
    // takes the terminal's own colour and the pulse does the talking. An
    // agent that has finished its turn and is sitting there is quiet.
    assert_eq!(painted(Phase::Starting), ("◌".into(), Color::Reset, plain));
    assert_eq!(
        painted(Phase::Working),
        (pulse(0).into(), Color::Reset, plain)
    );
    assert_eq!(
        painted(Phase::Idle),
        ("○".into(), Color::Reset, Modifier::DIM)
    );
}

#[test]
fn glyphs_draw_a_working_row_a_frame_at_a_time() {
    let at = |beat| {
        let mut screen = showing(
            vec![view("port-import-b2c", Phase::Working, Some("Running"), 3)],
            None,
        );
        screen.beat = beat;
        painted(&screen, (60, 8))[2].clone()
    };

    assert!(
        at(0).starts_with(&format!("  {} port-import-b2c", pulse(0))),
        "{:?}",
        at(0)
    );
    assert_ne!(at(0), at(LIVE), "a working row moves");
}

#[test]
fn view_draws_a_row_for_every_agent_under_a_heading_for_its_group() {
    let screen = drawn(
        vec![
            view("ask-a1b", Phase::Waiting, None, 90),
            view("fix-login-b2c", Phase::Working, Some("Running Bash"), 3),
        ],
        None,
        (60, 10),
    );

    assert!(
        screen[0].ends_with("1 working   2/5 running    1 WAITING"),
        "{:?}",
        screen[0]
    );
    assert_eq!(heading_of(&screen[2]), "NEEDS INPUT");
    assert!(
        screen[3].starts_with("• ? ask-a1b"),
        "a question nobody has been to read carries the mark that says so: \
         {:?}",
        screen[3]
    );
    assert!(screen[3].ends_with("1m"), "{:?}", screen[3]);
    assert_eq!(screen[4], "", "the next group stands off from this one");
    assert_eq!(heading_of(&screen[5]), "WORKING");
    assert!(
        screen[6].starts_with(&format!("  {} fix-login-b2c", pulse(0))),
        "{:?}",
        screen[6]
    );
    assert!(screen[6].contains("Running Bash"), "{:?}", screen[6]);
    assert!(screen[6].ends_with("3s"), "{:?}", screen[6]);
    assert_eq!(
        screen[9], "space card · enter attach · ctrl+x stop · ? keys",
        "and the keys, where they can be read"
    );
}

#[test]
fn view_keeps_a_row_to_one_line_however_much_the_agent_said() {
    let screen = drawn(
        vec![view(
            "fix-login-a1b",
            Phase::Idle,
            Some("I fixed it.\n\nHere is what I changed:\n- the parser"),
            1,
        )],
        None,
        (60, 8),
    );
    assert!(screen[2].contains("I fixed it."), "{:?}", screen[2]);
    assert!(
        !screen.iter().any(|line| line.contains("the parser")),
        "{screen:?}"
    );
}

#[test]
fn view_cuts_what_will_not_fit_rather_than_losing_the_age() {
    let screen = drawn(
        vec![view(
            "fix-login-a1b",
            Phase::Working,
            Some("Editing a file with a very long name indeed, and then some"),
            45,
        )],
        None,
        (40, 8),
    );
    assert!(screen[2].contains('…'), "{:?}", screen[2]);
    assert!(screen[2].ends_with("45s"), "{:?}", screen[2]);
    assert!(screen[2].chars().count() <= 40, "{:?}", screen[2]);
}

#[test]
fn view_says_on_an_armed_row_what_the_next_press_would_do_to_it() {
    let size = (60, 8);
    let mut screen = showing(
        vec![
            view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
            view(
                "port-importer-b2c",
                Phase::Done,
                Some("wrote the tests"),
                90,
            ),
        ],
        None,
    );
    assert!(painted(&screen, size)[2].contains("wrote the parser"));

    screen.arm = Some(Arm {
        ids: vec!["fix-login-a1b".to_string()],
        swept: false,
        at: Instant::now(),
    });
    let drawn = painted(&screen, size);
    assert!(
        drawn[2].contains("ctrl+x again forgets"),
        "the row says it where it was saying what the agent did: {:?}",
        drawn[2]
    );
    assert!(
        !drawn[2].contains("wrote the parser"),
        "in place of the summary rather than beside it: {:?}",
        drawn[2]
    );
    assert!(
        drawn[2].ends_with("1m"),
        "and the columns either side of it are where they were: {:?}",
        drawn[2]
    );
    assert_eq!(
        word_colour(&screen, size, 2, "ctrl+x again forgets"),
        theme().waiting,
        "in the colour of a thing waiting on a person"
    );
    assert!(
        drawn[3].contains("wrote the tests"),
        "and the rows nobody armed say what they always said: {:?}",
        drawn[3]
    );
}

#[test]
fn axis_heads_the_rows_with_the_project_and_gives_each_one_its_state() {
    let screen = painted(
        &by_project(vec![
            at(view("ask-a1b", Phase::Waiting, None, 30), "/src/api"),
            at(
                view("fix-login-b2c", Phase::Done, Some("fixed it"), 30),
                "/src/api",
            ),
            at(view("busy-c3d", Phase::Working, None, 3), "/src/web"),
        ]),
        (60, 10),
    );

    assert_eq!(screen[2], "/src/api");
    assert!(screen[3].contains("ask-a1b"), "{:?}", screen[3]);
    assert!(
        screen[3].contains("waiting"),
        "the heading is a place, so the row says the state: {:?}",
        screen[3]
    );
    assert!(screen[4].contains("done"), "{:?}", screen[4]);
    assert_eq!(screen[5], "", "the next project stands off from this one");
    assert_eq!(screen[6], "/src/web");

    // One column, so the states read down the screen rather than wandering
    // with the length of the name above them. Counted in characters: the
    // marks are not all one byte, and a column is what a person sees.
    let column = |line: &str, word: &str| {
        let at = line.find(word).expect("the state on the row");
        line[..at].chars().count()
    };
    assert_eq!(column(&screen[3], "waiting"), column(&screen[4], "done"));
}

#[test]
fn axis_leaves_the_state_off_a_row_the_heading_over_it_already_says() {
    let screen = painted(
        &showing(vec![view("busy-a1b", Phase::Working, None, 3)], None),
        (60, 8),
    );
    assert_eq!(heading_of(&screen[1]), "WORKING");
    assert!(
        !screen[2].contains("working"),
        "twice on one screen is a column of noise: {:?}",
        screen[2]
    );
}

#[test]
fn axis_says_at_the_top_what_the_list_was_narrowed_to() {
    let mut screen = showing(
        vec![
            view("busy-a1b", Phase::Working, None, 3),
            view("done-b2c", Phase::Done, None, 60),
        ],
        None,
    );
    screen
        .list
        .narrow(vec![Narrow::State(Some("working".to_string()))]);

    let painted = painted(&screen, (60, 8));
    assert!(
        painted[0].ends_with("1 working   1/5 running   s:working   nothing waiting"),
        "{:?}",
        painted[0]
    );
    assert!(painted[2].contains("busy-a1b"), "{:?}", painted[2]);
    assert!(
        !painted.iter().any(|line| line.contains("done-b2c")),
        "a hidden agent is not counted, not drawn and not headed: {painted:?}"
    );
}

#[test]
fn axis_says_nothing_matches_rather_than_claiming_there_are_no_agents() {
    let mut screen = showing(vec![view("busy-a1b", Phase::Working, None, 3)], None);
    screen
        .list
        .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);

    assert_eq!(painted(&screen, (60, 8))[1], "nothing matches a:nobody");
}

#[test]
fn axis_says_a_line_that_narrows_will_narrow_rather_than_start_anything() {
    let mut screen = showing(Vec::new(), None);
    let mut composer = Composer::new(Asking::Task);
    composer.text = "s:waiting".to_string();
    screen.mode = Mode::Typing(composer);

    let painted = painted(&screen, (60, 6));
    assert_eq!(painted[4], "narrow ▸ s:waiting");
    assert!(painted[5].contains("enter narrows it"), "{:?}", painted[5]);
    assert!(
        !painted[5].contains("starts it"),
        "a hint that says the other thing is a hint that lies: {:?}",
        painted[5]
    );
}

/// The background of every cell across one row of the list.
fn behind(screen: &Screen, size: (u16, u16), row: u16) -> Vec<Color> {
    let buffer = cells(screen, size);
    (0..size.0).map(|at| buffer[(at, row)].bg).collect()
}

#[test]
fn a_cursor_on_a_headings_line_is_marked_the_way_a_cursor_on_a_row_is() {
    let mut screen = showing(
        vec![
            view("busy-a1b", Phase::Working, None, 3),
            view("busy-b2c", Phase::Working, None, 5),
        ],
        None,
    );
    let bar = vec![theme().cursor; 60];
    let plain = vec![Color::Reset; 60];

    // The view opens on the first agent, with the heading over it bare.
    assert_eq!(behind(&screen, (60, 8), 2), bar, "the row the cursor is on");
    assert_eq!(behind(&screen, (60, 8), 1), plain, "and not the heading");

    screen.list.up();
    assert_eq!(
        behind(&screen, (60, 8), 1),
        bar,
        "a heading is a line like any other, so the cursor looks the same \
         on it: column zero to the last column, over a label that is a \
         third of that"
    );
    assert_eq!(behind(&screen, (60, 8), 2), plain);
}

#[test]
fn a_headings_bar_is_the_only_thing_that_says_where_the_cursor_is() {
    let painted = painted(
        &showing(
            vec![view("busy-a1b", Phase::Working, Some("Running"), 3)],
            None,
        ),
        (60, 8),
    );
    assert!(
        painted[2].starts_with(&format!("  {} busy-a1b", pulse(0))),
        "a row reads the same whether or not the cursor is on it: {:?}",
        painted[2]
    );
}

#[test]
fn headings_count_their_agents_whether_or_not_the_rows_are_under_them() {
    let mut screen = showing(
        vec![
            view("busy-a1b", Phase::Working, None, 3),
            view("busy-b2c", Phase::Working, None, 5),
        ],
        None,
    );

    assert_eq!(
        counted(&painted(&screen, (60, 8))[1]),
        "2",
        "the margin of a screen is a line of numbers, open or shut"
    );

    screen.list.up();
    screen.list.shut_or_open();
    let painted = painted(&screen, (60, 8));
    assert_eq!(counted(&painted[1]), "2");
    assert!(
        !painted.iter().any(|line| line.contains("busy-a1b")),
        "and shut, the count is all that is standing in for them: {painted:?}"
    );
}

#[test]
fn headings_say_how_many_failed_whether_or_not_the_rows_are_under_them() {
    let mut screen = showing(
        vec![
            view("done-a1b", Phase::Done, Some("did it"), 60),
            view("broke-b2c", Phase::Failed, Some("could not"), 60),
        ],
        None,
    );

    assert_eq!(
        heading_of(&painted(&screen, (60, 8))[1]),
        "COMPLETED · 1 failed",
        "a screenful of headings says how it went without being opened"
    );

    screen.list.up();
    screen.list.shut_or_open();
    assert_eq!(
        heading_of(&painted(&screen, (60, 8))[1]),
        "COMPLETED · 1 failed",
        "shutting a group hides the detail of a failure, never the fact"
    );
}

#[test]
fn card_neutralises_the_question_and_the_choices_it_quotes() {
    // The question is the agent's own words, and a bidirectional override
    // written into them can visually reorder the choices underneath —
    // which are the keys a person is about to press. ratatui drops the
    // control characters on its own; the invisible format characters it
    // keeps have to be neutralised before anything draws them.
    let mut card = asking(&["yes\u{200b}really", "no\u{ad}pe"], Some(Kind::Question));
    card.question = Some("pro\u{ad}ceed\u{202e}?".to_string());
    let screen = drawn(a_fleet(), Some(card), (60, 14)).join("\n");

    for (invisible, name) in [
        ('\u{202e}', "a bidi override"),
        ('\u{200b}', "a zero-width space"),
        ('\u{ad}', "a soft hyphen"),
    ] {
        assert!(
            !screen.contains(invisible),
            "{name} reached the terminal: {screen:?}"
        );
    }
    assert!(screen.contains("pro ceed"), "{screen:?}");
}

#[test]
fn headings_stand_off_from_whatever_is_above_them() {
    // A blank line above every heading, so the groups read as groups
    // instead of one run of rows — and the first of them is stood off from
    // the header the same way, so the list starts where the chrome ends
    // rather than against it.
    let screen = drawn(a_fleet(), None, (60, 12));
    assert!(screen[0].contains("running"), "the header: {screen:?}");
    assert_eq!(screen[2], "", "the space over the list");
    assert_eq!(heading_of(&screen[3]), "NEEDS INPUT", "the first heading");
    assert!(screen[4].contains("ask-a1b"), "{screen:?}");
    assert_eq!(screen[5], "", "a blank line stands the next group off");
    assert_eq!(heading_of(&screen[6]), "WORKING");
    assert!(screen[7].contains("busy-b2c"), "{screen:?}");
}

#[test]
fn the_space_over_the_list_is_the_first_row_a_short_screen_takes_back() {
    // Air is worth a row where there are rows to spare and not where there
    // are none: the header has already given its second row up by then,
    // and this one goes the same way.
    let tall = drawn(a_fleet(), None, (60, SPACED as u16));
    assert_eq!(tall[2], "", "{tall:?}");
    assert_eq!(heading_of(&tall[3]), "NEEDS INPUT", "{tall:?}");

    let short = drawn(a_fleet(), None, (60, SPACED as u16 - 1));
    assert_eq!(heading_of(&short[2]), "NEEDS INPUT", "{short:?}");
    assert!(short[3].contains("ask-a1b"), "{short:?}");
}

#[test]
fn headings_carry_the_weight_on_the_label_and_none_of_it_on_the_rule() {
    // Case and weight are what make a heading here, with no second type
    // size to make it with, and every heading wears them: where the cursor
    // is standing is said by the bar under one line, not by the headings
    // around it putting weight down and picking it up.
    let screen = showing(a_fleet(), None);
    let cells = cells(&screen, (60, 10));
    for row in [2, 5] {
        let label = cells[(1, row)].clone();
        assert!(
            label.modifier.contains(Modifier::BOLD),
            "the heading on row {row} is bold: {:?}",
            label.modifier
        );
    }
    assert_eq!(
        cells[(1, 2)].fg,
        theme().waiting,
        "the group that wants a person is the one carrying colour up here"
    );
    assert_eq!(
        cells[(1, 5)].fg,
        Color::Reset,
        "and the rest of them do not"
    );

    // The rule that carries the label out to its count carries none of the
    // weight, which is what leaves the label the loud thing on the line.
    let rule = cells[(30, 2)].clone();
    assert_eq!(rule.symbol(), "─", "the rule runs out to the count");
    assert!(
        rule.modifier.contains(Modifier::DIM) && !rule.modifier.contains(Modifier::BOLD),
        "{:?}",
        rule.modifier
    );
}

#[test]
fn view_says_when_there_is_nothing_to_show() {
    let screen = drawn(Vec::new(), None, (40, 6));
    assert!(screen[0].starts_with("AMX"), "{:?}", screen[0]);
    assert!(
        screen[0].ends_with("0/5 running   nothing waiting"),
        "{:?}",
        screen[0]
    );
    assert_eq!(screen[1], "no agents");
}

/// A screen with room for the whole header, at the width the mockup was
/// drawn at.
const WIDE: (u16, u16) = (100, 12);

/// The view with a launch profile that says where it is running: the
/// directory is read from the disk when a real view opens, and a test says
/// what the disk would have answered.
fn launching(views: Vec<View>) -> Screen {
    let mut screen = showing(views, None);
    screen.profile.dir = "~/code/amx".to_string();
    screen
}

/// One line of what a view of this size draws.
fn screen_line(screen: &Screen, size: (u16, u16), row: usize) -> String {
    painted(screen, size)[row].clone()
}

/// Which column of a drawn line a word starts at, for the tests that ask
/// what the view painted it in. Columns, not bytes: the separator between
/// two things said on one row is two bytes wide and one column.
fn column_of(line: &str, word: &str) -> u16 {
    let at = line.find(word).expect("the word is on the line");
    line[..at].chars().count() as u16
}

#[test]
fn header_says_where_it_is_and_what_the_fleet_is_over_the_dials() {
    let screen = painted(
        &launching(vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]),
        WIDE,
    );

    assert!(
        screen[0].starts_with("AMX  ~/code/amx"),
        "whose screen this is and where it was opened: {:?}",
        screen[0]
    );
    assert!(
        !screen[0].contains(env!("CARGO_PKG_VERSION")),
        "which version this is says nothing about the fleet: {:?}",
        screen[0]
    );
    assert!(
        screen[0].ends_with("1 working   2/5 running    1 WAITING"),
        "what the fleet is, the gate the next one meets, and the one count \
         that wants somebody at the end of the row: {:?}",
        screen[0]
    );
    assert_eq!(
        screen[1], "└ next  claude   model  default   permission  default   worktree  new",
        "and under it every dial the next agent will be started with"
    );
    assert_eq!(screen[2], "", "a blank row stands the list off from it");
    assert_eq!(
        heading_of(&screen[3]),
        "NEEDS INPUT",
        "and the list starts under that"
    );
}

#[test]
fn header_spends_its_one_colour_on_the_count_that_wants_a_person() {
    let screen = launching(vec![
        view("ask-a1b", Phase::Waiting, None, 30),
        view("ask-b2c", Phase::Waiting, None, 10),
        view("busy-c3d", Phase::Working, Some("Running Bash"), 3),
    ]);
    let drawn = painted(&screen, WIDE);

    assert!(
        drawn[0].ends_with(" 2 WAITING"),
        "the one number the view was opened for, at the end of the row: {:?}",
        drawn[0]
    );
    assert!(
        drawn[0].contains("1 working   3/5 running"),
        "the counts beside it say the rest of the fleet, and say the \
         waiting one nowhere else: {:?}",
        drawn[0]
    );

    // A block rather than a phrase: reverse video in the waiting colour,
    // out to the edge of the row, the space either side of the words
    // included.
    let buffer = cells(&screen, WIDE);
    for column in column_of(&drawn[0], " 2 WAITING")..WIDE.0 {
        let cell = buffer[(column, 0)].clone();
        assert_eq!(cell.fg, theme().waiting, "column {column}: {:?}", drawn[0]);
        assert!(
            cell.modifier.contains(Modifier::REVERSED | Modifier::BOLD),
            "column {column}: {:?}",
            cell.modifier
        );
    }
}

#[test]
fn header_says_nothing_waiting_in_words_where_nobody_is() {
    let screen = launching(vec![view(
        "busy-a1b",
        Phase::Working,
        Some("Running Bash"),
        3,
    )]);
    let drawn = painted(&screen, WIDE);

    assert!(
        drawn[0].ends_with("nothing waiting"),
        "the answer stands where the answer always stands: {:?}",
        drawn[0]
    );

    let buffer = cells(&screen, WIDE);
    let cell = buffer[(column_of(&drawn[0], "nothing waiting"), 0)].clone();
    assert_eq!(
        cell.fg,
        Color::Reset,
        "nothing is asking, so nothing shouts"
    );
    assert!(cell.modifier.contains(Modifier::DIM), "{:?}", cell.modifier);
    assert!(
        !cell.modifier.contains(Modifier::REVERSED),
        "{:?}",
        cell.modifier
    );
}

#[test]
fn header_hangs_the_dials_off_the_row_they_are_under() {
    let screen = launching(Vec::new());
    let drawn = painted(&screen, WIDE);
    assert_eq!(
        drawn[1], "└ next  claude   model  default   permission  default   worktree  new",
        "one glyph in the first column says the row is subordinate to the \
         one above it, without a word of explanation"
    );

    let buffer = cells(&screen, WIDE);
    for label in ["└", "next", "model", "permission", "worktree"] {
        let cell = buffer[(column_of(&drawn[1], label), 1)].clone();
        assert_eq!(cell.fg, Color::Reset, "{label}: {:?}", drawn[1]);
        assert!(
            cell.modifier.contains(Modifier::DIM),
            "{label}: {:?}",
            cell.modifier
        );
    }
    // The values are what somebody reads the row for, so they are the
    // thing on it wearing a colour.
    for value in ["claude", "new"] {
        let cell = buffer[(column_of(&drawn[1], value), 1)].clone();
        assert_eq!(cell.fg, theme().accent, "{value}: {:?}", drawn[1]);
        assert!(
            !cell.modifier.contains(Modifier::DIM),
            "{value}: {:?}",
            cell.modifier
        );
    }
}

#[test]
fn header_drops_the_dial_labels_before_it_cuts_what_they_are_set_to() {
    let screen = launching(Vec::new());
    assert_eq!(
        screen_line(&screen, (60, 12), 1),
        "└ next  claude  ·  default  ·  default  ·  new",
        "the value is the reading; the label is what a person already knows \
         the order of. Only `next` keeps its own, because it is what says \
         which half of the screen the row is about"
    );
}

#[test]
fn header_names_a_dial_that_rests_where_the_vendor_left_it() {
    let mut screen = launching(Vec::new());
    assert_eq!(
        screen_line(&screen, WIDE, 1),
        "└ next  claude   model  default   permission  default   worktree  new",
        "the vendor's own answer said as a value, not a guess at which \
         model claude would have picked"
    );

    // Turned, the value is what it was turned to. The label does not move,
    // so the row a person has learned to read stays the row they read.
    screen.profile.model = "opus".to_string();
    screen.profile.permission = "plan".to_string();
    screen.profile.worktree = false;
    assert_eq!(
        screen_line(&screen, WIDE, 1),
        "└ next  claude   model  opus   permission  plan   worktree  none"
    );

    // An agent the registry never heard of declares no dials, so the row
    // holds the vendor and the one dial that is amx's own.
    screen.profile.agent = "mock-claude".to_string();
    assert_eq!(
        screen_line(&screen, WIDE, 1),
        "└ next  mock-claude   worktree  none"
    );
}

#[test]
fn header_counts_the_fleet_in_the_words_a_filter_takes() {
    let mut screen = launching(vec![
        view("ask-a1b", Phase::Waiting, None, 30),
        view("done-b2c", Phase::Done, Some("did it"), 60),
    ]);
    assert!(
        screen_line(&screen, WIDE, 0).ends_with("1 done   1/5 running    1 WAITING"),
        "the heading over the rows says `needs input`; the counter says \
         the word the list can be narrowed by, and says the waiting one \
         once, in the badge: {:?}",
        screen_line(&screen, WIDE, 0)
    );

    // A narrowing is still read back where it was typed, so a short list
    // says why it is short.
    screen
        .list
        .narrow(vec![Narrow::State(Some("waiting".to_string()))]);
    assert!(
        screen_line(&screen, WIDE, 0).ends_with("1/5 running   s:waiting    1 WAITING"),
        "{:?}",
        screen_line(&screen, WIDE, 0)
    );
}

#[test]
fn header_says_the_gate_the_next_agent_meets_before_it_refuses() {
    let mut screen = launching(vec![
        view("busy-a1b", Phase::Working, None, 3),
        view("busy-b2c", Phase::Working, None, 3),
        view("busy-c3d", Phase::Working, None, 3),
        view("done-d4e", Phase::Done, Some("did it"), 60),
    ]);
    screen.profile.max = 5;
    assert!(
        screen_line(&screen, WIDE, 0).contains("3/5 running"),
        "an agent whose command has ended holds no slot: {:?}",
        screen_line(&screen, WIDE, 0)
    );
}

#[test]
fn header_sheds_the_dir_before_the_name_and_the_vendor_before_a_dial() {
    // Decided here rather than discovered at the edge of a terminal.
    let mut screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);

    let cramped = painted(&screen, (28, 12));
    assert!(
        cramped[0].starts_with("AMX"),
        "the name says what the screen is, and it is three columns: {:?}",
        cramped[0]
    );
    assert!(
        !cramped[0].contains("code/amx"),
        "a path cut to nothing is not a path: {:?}",
        cramped[0]
    );

    // A vendor is a command line, and a command is routinely a long one.
    // It gives way to the dials beside it: a dial cut off the end of the
    // row is a dial nobody can see they have turned.
    screen.profile.agent = "claude --settings /etc/amx/every-hook.json".to_string();
    let long = painted(&screen, (80, 12));
    assert!(long[1].starts_with("└ next  claude --set"), "{:?}", long[1]);
    assert!(
        long[1].contains('…'),
        "and it says it was cut: {:?}",
        long[1]
    );
    assert!(
        long[1].ends_with("permission  default   worktree  new"),
        "{:?}",
        long[1]
    );

    // Narrower still and the labels go first, which buys the vendor ten
    // columns before a dial gives up a character of its value.
    assert_eq!(
        screen_line(&screen, (50, 12), 1),
        "└ next  claude --…  ·  default  ·  default  ·  new"
    );

    // Narrower again and there is no room for all of it either way. What
    // the vendor keeps is a floor: a row that had fitted every dial on
    // the screen by leaving off what runs would be a row about nothing.
    let narrow = screen_line(&screen, (36, 12), 1);
    assert!(narrow.starts_with("└ next  claude …"), "{narrow:?}");
    assert!(
        narrow.ends_with('…'),
        "and the end of the row is what says it was cut: {narrow:?}"
    );
}

#[test]
fn header_sheds_the_counts_before_the_one_that_wants_a_person() {
    // Every group at once, which is more counting than a narrow terminal
    // has room for beside the name. What goes is the counting: the badge
    // is the answer the view was opened to read.
    let screen = launching(vec![
        view("ask-a1b", Phase::Waiting, None, 30),
        view("busy-b2c", Phase::Working, None, 3),
        view("idle-c3d", Phase::Idle, None, 30),
        view("done-d4e", Phase::Done, Some("did it"), 60),
    ]);

    assert!(
        screen_line(&screen, (60, 12), 0).contains("1 working   1 idle   1 done   3/5 running"),
        "{:?}",
        screen_line(&screen, (60, 12), 0)
    );

    let cramped = screen_line(&screen, (40, 12), 0);
    assert!(cramped.starts_with("AMX  ~/code/amx"), "{cramped:?}");
    assert!(cramped.ends_with(" 1 WAITING"), "{cramped:?}");
    assert!(
        !cramped.contains("running"),
        "and the counting is what gave the room up: {cramped:?}"
    );
}

#[test]
fn header_gives_the_row_back_to_the_list_on_a_short_screen() {
    let screen = launching(vec![view("busy-a1b", Phase::Working, None, 3)]);
    let short = painted(&screen, (60, SHORT as u16 - 1));

    assert!(
        short[0].starts_with("AMX  ~/code/amx"),
        "the row that says what there is stays; the dials are one \
         keypress from being read under the composer: {:?}",
        short[0]
    );
    assert!(
        short[0].ends_with("1 working   1/5 running   nothing waiting"),
        "{:?}",
        short[0]
    );
    assert!(!short.iter().any(|line| line.starts_with('└')), "{short:?}");
    assert_eq!(
        heading_of(&short[1]),
        "WORKING",
        "and the list starts a row sooner"
    );
}

/// A screen with room for the bands above and below the list, the space
/// between the header and it, and a group or two under that.
const WALL: (u16, u16) = (80, 12);

#[test]
fn a_wall_nobody_has_put_anything_on_says_so_in_one_line_of_its_own() {
    let screen = drawn(Vec::new(), None, WALL);

    assert_eq!(screen[3], WELCOME, "{screen:?}");
    // Everything under it down to the keys is the empty wall itself: one
    // line where the four groups used to have a sentence each.
    assert!(
        screen[4..screen.len() - 1].iter().all(String::is_empty),
        "one line, and no more: {screen:?}"
    );
    for group in Group::ALL {
        assert!(
            !screen.iter().any(|line| line.contains(group.title())),
            "{} stands over rows, and there are none: {screen:?}",
            group.title()
        );
    }
}

#[test]
fn the_wall_says_it_plainly_where_the_line_of_its_own_will_not_fit() {
    // Said whole or not at all: a sentence cut by the terminal reads as a
    // sentence that ends where the screen does, and this one is a joke as
    // well, which is worse to be handed two thirds of.
    let narrow = drawn(
        Vec::new(),
        None,
        (WELCOME.chars().count() as u16 - 1, WALL.1),
    );
    assert_eq!(narrow[3], "no agents");
    let wide = drawn(Vec::new(), None, (WELCOME.chars().count() as u16, WALL.1));
    assert_eq!(wide[3], WELCOME);
}

#[test]
fn the_wall_has_its_line_to_itself_and_gives_it_up_to_the_first_row() {
    let one = drawn(
        vec![view("done-a1b", Phase::Done, Some("did it"), 60)],
        None,
        WALL,
    );
    assert_eq!(heading_of(&one[3]), "COMPLETED");
    assert!(
        !one.iter().any(|line| line.contains("nobody asking")),
        "one agent and there is something to read off the rows: {one:?}"
    );

    // A fleet somebody narrowed to nothing is not a fleet nobody started,
    // and the view owes them the words they typed rather than a joke.
    let mut screen = showing(Vec::new(), None);
    screen
        .list
        .narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
    assert_eq!(painted(&screen, WALL)[3], "nothing matches a:nobody");

    // And the project axis is a list of places, which nobody arrives at
    // with nothing to arrange.
    let mut screen = showing(Vec::new(), None);
    screen.list.turn();
    assert_eq!(painted(&screen, WALL)[3], "no agents");
}

#[test]
fn view_shows_the_fold_and_what_it_is_holding_back() {
    // A working agent and five finished. On a tall screen every row is
    // drawn and there is no fold at all; on a short one the finished
    // group takes the rows the live group left, and the fold stands on
    // the band's last row saying exactly what did not fit.
    let fleet = || {
        let mut views = vec![view("busy-b2c", Phase::Working, Some("Running Bash"), 3)];
        views.extend((0..5).map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60)));
        views
    };

    let tall = settled(fleet(), (40, 24));
    assert_eq!(tall.iter().filter(|l| l.contains("done-")).count(), 5);
    assert!(!tall.iter().any(|l| l.contains("more")), "{tall:?}");

    let short = settled(fleet(), (40, 10));
    assert_eq!(heading_of(&short[5]), "COMPLETED");
    assert_eq!(short.iter().filter(|l| l.contains("done-")).count(), 2);
    assert!(
        short[8].contains("… 3 more"),
        "the fold stands on the last row the band has: {short:?}"
    );
}

#[test]
fn card_shows_the_question_alone_and_none_of_the_pane_it_is_asked_on() {
    // The pane under a question is the vendor's drawing of the same box
    // the card already says in rows of its own, behind an echo of the
    // prompt: everything on it is noise below the answer line.
    let screen = drawn(
        vec![view("ask-a1b", Phase::Waiting, None, 30)],
        Some(Card {
            question: Some("Claude needs your permission to use Bash".to_string()),
            body: "$ rm -rf build\nDo you want to proceed?\n\n\n".to_string(),
            options: Vec::new(),
            kind: Some(Kind::Permission),
            ..asking(&[], None)
        }),
        (60, 12),
    );

    let all = screen.join("\n");
    assert!(all.contains("ask-a1b · waiting"), "{all}");
    assert!(all.contains("Claude needs your permission"), "{all}");
    assert!(
        !all.contains("Do you want to proceed?"),
        "the question block is the whole of the card: {all}"
    );
    assert_eq!(
        screen[11], "space closes it · enter attach · ctrl+x stop · ? keys",
        "the keys stay on the screen under the card, saying what they do \
         while it is up"
    );
    assert!(
        screen.iter().any(|line| line.contains("ask-a1b")),
        "and the list is still there above it: {all}"
    );

    let top = screen
        .iter()
        .position(|line| line.starts_with('╭'))
        .expect("the top of the card");
    let bottom = screen
        .iter()
        .rposition(|line| line.starts_with('╰'))
        .expect("the foot of the card");
    assert_eq!(
        bottom - top,
        2,
        "and the card is the question's own size, with no window kept \
         for a pane it will not draw: {screen:?}"
    );
}

/// The row the keys are drawn on, which is the last one on the screen.
fn hint_row(screen: &Screen, size: (u16, u16)) -> String {
    painted(screen, size).pop().expect("a row for the keys")
}

/// A fleet with nothing left to finish, so there is a fold to walk onto.
fn all_done() -> Vec<View> {
    (0..5)
        .map(|n| view(&format!("done-{n}"), Phase::Done, Some("did it"), 60))
        .collect()
}

#[test]
fn keymap_hints_are_the_keys_the_line_under_the_cursor_answers_to() {
    let wide = (80, 12);
    let mut screen = showing(a_fleet(), None);

    // The view opens on an agent's row, where those keys reach the agent.
    assert_eq!(
        hint_row(&screen, wide),
        "space card · enter attach · ctrl+x stop · ctrl+s axis · q quit · ? keys"
    );

    // One line up is the heading over it, where the same two keys do
    // something else entirely.
    screen.list.up();
    assert_eq!(
        hint_row(&screen, wide),
        "enter shuts it · ctrl+x clears the group · ctrl+s axis · q quit · ? keys"
    );

    // And a group somebody has shut is opened by the key that shut it.
    screen.list.shut_or_open();
    assert!(
        hint_row(&screen, wide).starts_with("enter opens it"),
        "{:?}",
        hint_row(&screen, wide)
    );
}

#[test]
fn keymap_hints_offer_nothing_the_line_under_the_cursor_cannot_do() {
    let wide = (80, 12);

    // A card is put away by the key that opened it.
    let mut screen = showing(a_fleet(), None);
    screen.card = Some(asking(&[], None).read());
    assert!(
        hint_row(&screen, wide).starts_with("space closes it · enter attach"),
        "{:?}",
        hint_row(&screen, wide)
    );

    // An agent whose command has ended has no window to bring forward and
    // nothing left to stop.
    let mut screen = showing(all_done(), None);
    screen.list.fit(5);
    screen.list.refit();
    let row = hint_row(&screen, wide);
    assert!(row.starts_with("space card · ctrl+x forget"), "{row:?}");
    assert!(!row.contains("attach"), "{row:?}");

    // The fold is not an agent either: what enter does there is give back
    // the rows it is holding.
    for _ in 0..3 {
        screen.list.down();
    }
    assert!(
        hint_row(&screen, wide).starts_with("enter shows them"),
        "{:?}",
        hint_row(&screen, wide)
    );

    // And a wall with nothing on it has no line under the cursor at all.
    let screen = showing(Vec::new(), None);
    assert!(
        hint_row(&screen, wide).starts_with("n starts one"),
        "{:?}",
        hint_row(&screen, wide)
    );
}

#[test]
fn keymap_hints_shed_from_the_far_end_and_never_shed_the_overlay() {
    let screen = showing(a_fleet(), None);
    for width in 12..=80 {
        let row = hint_row(&screen, (width, 12));
        assert!(
            row.chars().count() <= width as usize,
            "a hint cut in half is a key that reads as another one: {row:?}"
        );
        assert!(
            row.ends_with("? keys"),
            "the row that has all of them is the last thing to go: {row:?}"
        );
    }

    // What is shed is what is furthest from it, and what is kept is what
    // the line under the cursor answers to.
    assert_eq!(
        hint_row(&screen, (60, 12)),
        "space card · enter attach · ctrl+x stop · ? keys"
    );
}

#[test]
fn view_says_what_it_could_not_do_where_the_keys_are() {
    let mut screen = showing(Vec::new(), None);
    screen.notice = Some(Notice::Advice(
        "fix-login-a1b has no pane any more".to_string(),
    ));

    let painted = painted(&screen, (60, 6));
    assert_eq!(painted[5], "fix-login-a1b has no pane any more");
}

#[test]
fn glyphs_and_notices_tell_a_failure_from_advice() {
    // The first cell of the row the two of them share.
    let said = |notice| {
        let mut screen = showing(Vec::new(), None);
        screen.notice = Some(notice);
        let cell = cells(&screen, (60, 6))[(0, 5)].clone();
        (cell.fg, cell.modifier)
    };

    assert_eq!(
        said(Notice::Failed("could not stop fix-login-a1b".to_string())),
        (theme().failed, Modifier::empty())
    );
    assert_eq!(
        said(Notice::Advice(
            "fix-login-a1b is done; nothing is listening".to_string()
        )),
        (Color::Reset, Modifier::DIM),
        "a thing that did not happen is not a thing that went wrong"
    );
}

#[test]
fn view_shows_what_an_agent_changed_from_the_top_of_the_patch() {
    let patch = (0..40)
        .map(|n| format!("+ line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let screen = drawn(
        vec![view("fix-login-a1b", Phase::Working, None, 3)],
        Some(Card {
            id: "fix-login-a1b".to_string(),
            phase: Phase::Working,
            age: 3,
            question: None,
            options: Vec::new(),
            kind: None,
            body: patch,
            changes: true,
            answer: false,
        }),
        (60, 14),
    );

    let all = screen.join("\n");
    assert!(all.contains("fix-login-a1b · what it has changed"), "{all}");
    assert!(
        all.contains("+ line 0"),
        "the first of it, not the last: {all}"
    );
    assert!(!all.contains("+ line 39"), "{all}");
}

/// The card over a patch of this many lines, which can be more than any
/// card has rows for.
fn a_long_patch(lines: usize) -> Card {
    Card {
        id: "fix-login-a1b".to_string(),
        phase: Phase::Working,
        age: 3,
        question: None,
        options: Vec::new(),
        kind: None,
        body: (0..lines)
            .map(|n| format!("+ line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        changes: true,
        answer: false,
    }
}

#[test]
fn card_pages_a_patch_from_its_offset_and_says_how_far() {
    let screen = showing(
        vec![view("fix-login-a1b", Phase::Working, None, 3)],
        Some(a_long_patch(40)),
    );
    screen.scroll.away.set(20);

    let all = painted(&screen, (60, 14)).join("\n");
    assert!(all.contains("+ line 20"), "the page it was sent to: {all}");
    assert!(!all.contains("+ line 0"), "{all}");
    assert!(!all.contains("+ line 39"), "{all}");
    assert!(all.contains("↑ 20 more"), "how far from the top: {all}");
    assert_eq!(screen.scroll.away.get(), 20);
}

#[test]
fn card_stops_a_page_at_the_end_of_the_patch() {
    let screen = showing(
        vec![view("fix-login-a1b", Phase::Working, None, 3)],
        Some(a_long_patch(40)),
    );
    screen.scroll.away.set(1000);

    let all = painted(&screen, (60, 14)).join("\n");
    assert!(all.contains("+ line 39"), "the last of it: {all}");
    assert_eq!(
        screen.scroll.away.get(),
        40 - screen.scroll.page.get(),
        "written back as the last page there is: {all}"
    );
}

#[test]
fn card_holds_a_fitting_body_at_its_edge() {
    let screen = showing(
        vec![view("fix-login-a1b", Phase::Working, None, 3)],
        Some(a_long_patch(3)),
    );
    screen.scroll.away.set(5);

    let all = painted(&screen, (60, 14)).join("\n");
    assert!(all.contains("+ line 0"), "{all}");
    assert!(all.contains("+ line 2"), "{all}");
    assert!(!all.contains("more"), "nothing is hidden: {all}");
    assert_eq!(screen.scroll.away.get(), 0, "nothing to page over");
}

#[test]
fn card_pages_a_recorded_answer_down_from_its_top() {
    let answered = || {
        showing(
            vec![view("fix-login-a1b", Phase::Done, None, 3)],
            Some(Card {
                id: "fix-login-a1b".to_string(),
                phase: Phase::Done,
                age: 3,
                question: None,
                options: Vec::new(),
                kind: None,
                body: (0..40)
                    .map(|n| format!("said {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                changes: false,
                answer: true,
            }),
        )
    };

    // An answer reads forward, so the card opens on its first words.
    let opened = painted(&answered(), (60, 14)).join("\n");
    assert!(opened.contains("said 0"), "{opened}");
    assert!(!opened.contains("said 39"), "{opened}");

    // And paged, it stands that many rows below the top.
    let screen = answered();
    screen.scroll.away.set(7);
    let all = painted(&screen, (60, 14)).join("\n");
    assert!(all.contains("said 7"), "seven rows below the top: {all}");
    assert!(
        !all.contains("said 0"),
        "the first words are behind it: {all}"
    );
    assert!(all.contains("↑ 7 more"), "how far from the top: {all}");
    assert_eq!(screen.scroll.away.get(), 7);
}

#[test]
fn card_gives_a_long_answer_its_whole_allowance() {
    // Forty rows of answer on a twenty-row screen: the card grows to
    // everything the height allows rather than the few lines a capture
    // used to fill, and the rest is there to page onto.
    let long: String = (0..40).map(|n| format!("said {n}\n")).collect();
    let card = Card {
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        body: long,
        answer: true,
        ..asking(&[], None)
    };
    let screen = drawn(a_fleet(), Some(card), (60, 20));

    let top = screen
        .iter()
        .position(|line| line.starts_with('╭'))
        .expect("the top of the card");
    let bottom = screen
        .iter()
        .rposition(|line| line.starts_with('╰'))
        .expect("the foot of the card");
    assert_eq!(
        bottom - top,
        9,
        "half the screen, the card's cap: {screen:?}"
    );
    assert!(
        screen[top + 1].contains("said 0"),
        "opened at the answer's first words: {screen:?}"
    );
}

#[test]
fn card_holding_a_question_never_leaves_its_edge() {
    let screen = showing(a_fleet(), Some(asking(&["1. Yes", "2. No"], None)));
    screen.scroll.away.set(5);

    let all = painted(&screen, (60, 14)).join("\n");
    assert!(!all.contains("more"), "{all}");
    assert_eq!(
        screen.scroll.away.get(),
        0,
        "a question block does not page"
    );
}

/// A capture with the vendor's paint on it, which is what costs something
/// to read: the escapes are what the walk is for.
const PAINTED: &str = "\u{1b}[1mwrote the parser\u{1b}[0m\n\u{1b}[32m+ done\u{1b}[0m";

#[test]
fn card_walks_its_body_when_it_is_built_and_never_again_on_a_frame() {
    let mut card = asking(&[], None);
    card.phase = Phase::Working;
    card.question = None;
    card.body = PAINTED.to_string();

    let walked = walks();
    let screen = showing(a_fleet(), Some(card));
    assert_eq!(
        walks(),
        walked + 1,
        "the body is walked out of its escapes where the card is built"
    );

    // A view redraws on every key, every tick and every mouse move. None
    // of them is a reason to read the same capture again.
    for _ in 0..3 {
        let drawn = painted(&screen, (60, 14)).join("\n");
        assert!(drawn.contains("wrote the parser"), "{drawn}");
    }
    assert_eq!(
        walks(),
        walked + 1,
        "and every frame after it draws from that walk"
    );
}

/// A screen with room for the composer to reach its cap and a list above
/// it: ten rows is a third of thirty.
const TALL: (u16, u16) = (60, 30);

/// The view with somebody part way through typing this line.
fn typing(text: &str) -> Screen {
    let mut screen = showing(Vec::new(), None);
    let mut composer = Composer::new(Asking::Task);
    composer.text = text.to_string();
    screen.mode = Mode::Typing(composer);
    screen
}

/// Where the terminal's own cursor was left, which is where the next
/// character somebody types will land.
fn caret(screen: &Screen, size: (u16, u16)) -> (u16, u16) {
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
    terminal.draw(|frame| draw(frame, screen)).unwrap();
    let at = terminal.get_cursor_position().unwrap();
    (at.x, at.y)
}

#[test]
fn composer_an_empty_task_line_names_its_own_prefixes() {
    // Wide enough for the whole sentence; a narrow screen clips it with
    // the ellipsis every other row wears.
    let empty = painted(&typing(""), (110, 30));
    let hint = empty
        .iter()
        .find(|row| row.contains("m:model"))
        .expect("the empty line teaches its prefixes");
    assert!(
        hint.starts_with("task ▸ m:model"),
        "the hint is a placeholder on the line itself, not a row of its \
         own: {hint}"
    );
    for named in [
        "m:model",
        "p:permission",
        "w:on|off",
        "d:directory",
        "agent:command",
        "s:state",
        "a:name",
    ] {
        assert!(hint.contains(named), "{named} is not taught: {hint}");
    }
    assert_eq!(
        empty.iter().filter(|row| row.contains("m:model")).count(),
        1,
        "and only there: the band under the composer is gone"
    );

    let narrow = painted(&typing(""), TALL);
    let clipped = narrow
        .iter()
        .find(|row| row.contains("m:model"))
        .expect("a narrow screen still teaches what fits");
    assert!(clipped.starts_with("task ▸ m:model"), "{clipped}");
    assert!(clipped.trim_end().ends_with('…'), "{clipped}");

    // The next keystroke lands where the prompt ends, over the
    // placeholder, the way a browser draws a field's ghost text.
    assert_eq!(caret(&typing(""), TALL), (7, 27));

    // The first character typed takes the placeholder away: whoever is
    // typing has stopped reading it.
    let typed = painted(&typing("p"), TALL);
    assert!(
        !typed.iter().any(|row| row.contains("m:model")),
        "{typed:?}"
    );

    // A reply goes to an agent already running, where a dial means
    // nothing, so the line would be teaching keys it does not read.
    let mut replying = showing(Vec::new(), None);
    replying.mode = Mode::Typing(Composer::new(Asking::Reply {
        id: "fix-a1b".to_string(),
        question: false,
    }));
    let reply = painted(&replying, TALL);
    assert!(
        !reply.iter().any(|row| row.contains("m:model")),
        "{reply:?}"
    );
}

#[test]
fn composer_wraps_what_will_not_fit_and_starts_a_row_at_every_newline() {
    assert_eq!(composer_lines("abcdef", 3), ["abc", "def"]);
    assert_eq!(
        composer_lines("port the importer\nand its tests", 40),
        ["port the importer", "and its tests"]
    );
    assert_eq!(
        composer_lines("a\n\nb", 8),
        ["a", "", "b"],
        "a paragraph with nothing in it is a row, because the cursor sits \
         on it"
    );
    assert_eq!(composer_lines("", 8), [""]);
}

#[test]
fn composer_grows_a_row_at_a_time_as_the_line_it_holds_does() {
    let one = painted(&typing("port the importer"), TALL);
    assert_eq!(one[27], "task ▸ port the importer");
    assert_eq!(one[26], "", "one line takes one row, at the foot of it all");

    let three = painted(
        &typing("port the importer\nand its tests\nand the docs"),
        TALL,
    );
    assert_eq!(three[25], "task ▸ port the importer");
    assert_eq!(
        three[26], "       and its tests",
        "a row under the first is indented to it, so a task reads as one \
         thing"
    );
    assert_eq!(three[27], "       and the docs");
    assert_eq!(
        caret(&typing("port it\nand test it"), TALL),
        (18, 27),
        "and the cursor is at the end of the last of them"
    );
}

#[test]
fn composer_wrapping_past_the_width_grows_it_the_same_way_a_newline_does() {
    // Twice the room a sixty-column screen leaves beside the prompt.
    let painted = painted(&typing(&"x".repeat(106)), TALL);
    assert_eq!(painted[26], format!("task ▸ {}", "x".repeat(53)));
    assert_eq!(painted[27], format!("       {}", "x".repeat(53)));
}

/// A line long enough to need more rows than any screen will give it.
fn twenty_rows() -> String {
    (1..=20)
        .map(|n| format!("row-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn composer_stops_growing_at_its_cap_and_scrolls_the_line_inside_it() {
    let screen = typing(&twenty_rows());
    let painted = painted(&screen, TALL);

    assert_eq!(
        painted[18], "task ▸ row-11",
        "the prompt is on the top row however far the rest has scrolled: \
         {painted:?}"
    );
    assert_eq!(painted[27], "       row-20", "{painted:?}");
    assert!(
        !painted.iter().any(|line| line.contains("row-10")),
        "and what scrolled past is off the screen: {painted:?}"
    );
    assert_eq!(caret(&screen, TALL), (13, 27));
}

#[test]
fn composer_leaves_the_list_it_was_opened_from_on_the_screen() {
    // A third of eight rows is two, whatever the line is holding, and the
    // agents are what the view is for.
    let painted = painted(&typing(&twenty_rows()), (60, 8));
    assert_eq!(painted[4], "task ▸ row-19");
    assert_eq!(painted[5], "       row-20");
    assert_eq!(
        painted[1], WELCOME,
        "the list is still there above it: {painted:?}"
    );
}

#[test]
fn view_shows_the_line_being_typed_and_what_entering_it_will_do() {
    let mut screen = showing(Vec::new(), None);
    let mut composer = Composer::new(Asking::Task);
    composer.text = "port the importer".to_string();
    screen.mode = Mode::Typing(composer);

    let painted = painted(&screen, (60, 6));
    assert_eq!(painted[3], "task ▸ port the importer");
    assert!(painted[5].contains("enter starts it"), "{:?}", painted[5]);
    assert!(painted[5].contains("alt+enter newline"), "{:?}", painted[5]);
}

#[test]
fn header_says_what_the_next_agent_may_do_without_asking() {
    let mut screen = launching(Vec::new());
    screen.mode = Mode::Typing(Composer::new(Asking::Task));

    let drawn = painted(&screen, (60, 8));
    assert!(
        drawn[5].starts_with("task ▸ m:model"),
        "the empty line carries its placeholder above the dial: {:?}",
        drawn[5]
    );
    assert_eq!(
        drawn[6], "permission: vendor default (shift+tab to cycle)",
        "the layer, not a guess at which mode claude would have picked"
    );
    assert!(drawn[7].contains("enter starts it"), "{:?}", drawn[7]);

    screen.profile.permission = "acceptEdits".to_string();
    assert_eq!(
        painted(&screen, (60, 8))[6],
        "⏵⏵ acceptEdits (shift+tab to cycle)",
        "and a mode in the vendor's own word for it"
    );
}

#[test]
fn header_keeps_the_permission_row_to_the_lines_that_start_an_agent() {
    let row = |screen: &Screen| {
        painted(screen, (60, 8))
            .iter()
            .any(|line| line.contains("shift+tab"))
    };

    // A reply goes to an agent that is already running under whatever it
    // was started with, so the dial has nothing to say about it.
    let mut screen = launching(Vec::new());
    screen.mode = Mode::Typing(Composer::new(Asking::Reply {
        id: "ask-a1b".to_string(),
        question: true,
    }));
    assert!(!row(&screen), "a reply is not a spawn");

    // Nor has it anything to say about a line that narrows the list.
    let mut composer = Composer::new(Asking::Task);
    composer.text = "s:waiting".to_string();
    screen.mode = Mode::Typing(composer);
    assert!(!row(&screen));

    // A vendor amx has no entry for declares no permission dial: there is
    // nothing to say and nothing to turn, so the row is absent rather than
    // empty.
    screen.mode = Mode::Typing(Composer::new(Asking::Task));
    screen.profile.agent = "mock-claude".to_string();
    assert!(!row(&screen));

    // And nothing is being typed at all, which is most of the time.
    let screen = launching(Vec::new());
    assert!(!row(&screen));
}

#[test]
fn header_leaves_the_list_a_row_with_every_other_band_open() {
    // Four bands of chrome at once: the header, a closer look, a line
    // being typed and the row under it. The list is what the view is for,
    // so the closer look gives way rather than the rows it was opened
    // from.
    let mut screen = launching(vec![view("ask-a1b", Phase::Waiting, None, 30)]);
    screen.card = Some(asking(&["the sqlite one"], Some(Kind::Question)).read());
    screen.mode = Mode::Typing(Composer::new(Asking::Task));

    let painted = painted(&screen, (60, 10));
    assert!(
        painted.iter().any(|line| line.contains("ask-a1b")),
        "{painted:?}"
    );
    assert!(
        painted.iter().any(|line| line.contains("shift+tab")),
        "{painted:?}"
    );
    assert!(painted[9].contains("enter starts it"), "{:?}", painted[9]);
}

#[test]
fn view_lists_every_key_when_somebody_asks_for_them() {
    let mut screen = showing(Vec::new(), None);
    screen.mode = Mode::Keys;

    // Tall and wide enough for every key and every heading over them,
    // so each of them has the row to itself and every description is
    // whole.
    let tall = (HELP.len() + GROUPS.len()) as u16 + header_rows(24) + space_rows(24) + 1;
    let painted = painted(&screen, (140, tall)).join("\n");
    for (key, does) in HELP {
        assert!(painted.contains(key), "{key} is missing:\n{painted}");
        assert!(painted.contains(does), "{does} is missing:\n{painted}");
    }
}

/// The overlay on a screen this size, and the rows it was drawn on.
fn overlay(size: (u16, u16)) -> Vec<String> {
    let mut screen = showing(Vec::new(), None);
    screen.mode = Mode::Keys;
    painted(&screen, size)
}

#[test]
fn keymap_stands_the_keys_under_headings_that_say_what_they_are_for() {
    // A screen with room for the groups in two columns, which is the
    // shape they are laid out in wherever the width will take it.
    let painted = overlay((140, 38));

    // Down before across: the second key is under the first rather than
    // beside it, and the second column starts where the first one's share
    // of the width ends.
    assert!(painted[3].starts_with("walk"), "{:?}", painted[3]);
    assert!(painted[4].starts_with(HELP[0].0), "{:?}", painted[4]);
    assert_eq!(
        column_of(&painted[3], "arrange"),
        70,
        "and the next column stands beside the first: {:?}",
        painted[3]
    );

    // A heading over every run of keys, a blank row between two groups,
    // and the groups themselves whole rather than split down the fold.
    assert!(
        painted[9].chars().take(70).all(char::is_whitespace),
        "one group stands off from the next: {:?}",
        painted[9]
    );
    assert!(painted[10].starts_with("look"), "{:?}", painted[10]);
    assert!(painted[17].starts_with("start"), "{:?}", painted[17]);
    assert_eq!(column_of(&painted[12], "dials"), 70, "{:?}", painted[12]);

    let all = painted.join("\n");
    for (key, does) in HELP {
        assert!(key.len() < 12, "{key} is wider than a band's key column");
        assert!(all.contains(key), "{key} is missing:\n{all}");
        assert!(all.contains(does), "{does} is missing:\n{all}");
    }
}

#[test]
fn keymap_headings_are_the_quietest_thing_on_the_screen_of_keys() {
    let mut screen = showing(Vec::new(), None);
    screen.mode = Mode::Keys;
    let buffer = cells(&screen, (140, 38));

    let heading = buffer[(0, 3)].clone();
    assert!(
        heading.modifier.contains(Modifier::DIM),
        "a heading stands over the keys and is not one of them: {:?}",
        heading.modifier
    );
    let key = buffer[(0, 4)].clone();
    assert!(
        key.modifier.contains(Modifier::BOLD),
        "the key itself is what somebody came here to find: {:?}",
        key.modifier
    );
}

#[test]
fn keymap_takes_another_column_when_the_rows_will_not_hold_a_group() {
    // Two rows of header, one of space and one of keys leave eleven for
    // the overlay, which is fewer rows than two columns of groups need:
    // rather than cut a group in half or run one off the bottom, the
    // groups deal into as many columns as the height asks for.
    let painted = overlay((140, 15));
    let all = painted.join("\n");
    for (key, _) in HELP {
        assert!(all.contains(key), "{key} is missing:\n{all}");
    }
    assert!(painted[3].starts_with("walk"), "{:?}", painted[3]);
    assert_eq!(
        column_of(&painted[3], "dials"),
        4 * (140 / GROUPS.len() as u16),
        "a column each, in the order the table has them: {:?}",
        painted[3]
    );
}

#[test]
fn keymap_the_keys_give_up_what_they_say_before_they_give_up_a_key() {
    // The same screen with no room for two whole bands. Every key is
    // still on it, because a key nobody can find is worse than one whose
    // line was cut short.
    let painted = overlay((60, 15));
    let all = painted.join("\n");
    for (key, _) in HELP {
        assert!(all.contains(key), "{key} is missing:\n{all}");
    }
    for line in &painted {
        assert!(line.chars().count() <= 60, "{line:?}");
    }
    assert!(
        all.contains('…'),
        "and what was cut says it was cut:\n{all}"
    );
}

#[test]
fn view_reads_the_bottom_of_a_screen_and_drops_what_is_blank() {
    let shown = |text: &'static str, wanted: usize, back: usize| {
        // The blank rows at the bottom are dropped where the body is
        // built, so what `tail` is handed is already the last row anybody
        // wrote on.
        let rows: Vec<&str> = text.lines().collect();
        let mut kept = rows.len();
        while kept > 0 && rows[kept - 1].trim().is_empty() {
            kept -= 1;
        }
        rows[tail(kept, wanted, back)].to_vec()
    };
    assert_eq!(shown("a\nb\nc\n\n\n", 2, 0), ["b", "c"]);
    assert_eq!(shown("a\nb", 5, 0), ["a", "b"]);
    assert!(shown("", 3, 0).is_empty());
    // Paged back, the window stands above the bottom it is read from.
    assert_eq!(shown("a\nb\nc\nd\n\n", 2, 1), ["b", "c"]);
    assert!(shown("a\nb", 2, 5).is_empty());
}

/// The five rows claude draws at the bottom of every pane it has the room
/// for, in the vendor's own order: the composer's top border with its
/// right-anchored label, whatever is staged in the box, the composer's
/// bottom border, the statusline, and the mode footer. Transcribed from a
/// live 2.1.237 at 100 columns on 2026-08-21.
const CHROME: [&str; 5] = [
    "───────────────────────────── execute amx-v2 tail ─",
    "❯ ",
    "───────────────────────────────────────────────────",
    "  Opus 5 │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
    "  ⏵⏵ accept edits on (shift+tab to cycle) · ← 3 agents",
];

/// A row of the agent's own work, which is the one thing no step may take.
const SAID: &str = "what the agent said";

/// That screen with `typed` staged in the composer, under a row of work.
fn staged(typed: &[&'static str]) -> Vec<&'static str> {
    let mut screen = vec![SAID, CHROME[0]];
    screen.extend_from_slice(typed);
    screen.extend_from_slice(&CHROME[2..]);
    screen
}

#[test]
fn view_tail_cuts_the_chrome_claude_draws_under_every_pane() {
    let mut screen = vec![SAID, "", "✻ Nesting… (15s · thinking)", ""];
    screen.extend_from_slice(&CHROME);
    assert_eq!(
        cut(&screen),
        [SAID, ""].as_slice(),
        "the spinner goes with the box it sits over"
    );
}

#[test]
fn view_tail_cuts_a_composer_whatever_is_staged_in_it() {
    // A composer with one row of text in it is the state that let a walk
    // cutting exactly one input row pass for a working rule, so neither
    // fixture here has one: a task wrapped over three rows, and a message
    // typed over four lines.
    let wrapped = staged(&[
        "❯ port the importer and then check every",
        "  call site that used to take the old",
        "  shape",
    ]);
    assert_eq!(cut(&wrapped), [SAID].as_slice());

    let lines = staged(&["❯ first", "  second", "  third", "  fourth"]);
    assert_eq!(cut(&lines), [SAID].as_slice());
}

#[test]
fn view_tail_leaves_a_screen_the_vendor_drew_no_footer_under_alone() {
    // A permission prompt, which ends at its own confirm row: cutting
    // upward from there would take the question the card was opened for.
    let prompt = [
        "───────────────────────────────────",
        " Bash command",
        "   rm -rf build",
        " Do you want to proceed?",
        " ❯ 1. Yes",
        "   2. No",
        " Esc to cancel · Tab to amend",
    ];
    assert_eq!(cut(&prompt), prompt.as_slice());

    // And a pane too short for the vendor to draw its chrome in, whose
    // last row is the composer's own bottom border.
    let short = [SAID, CHROME[0], CHROME[1], CHROME[2]];
    assert_eq!(cut(&short), short.as_slice());
}

#[test]
fn view_tail_gives_back_by_position_what_it_cannot_place() {
    // Three rows between the footer and the nearest rule: not the shape
    // this was measured against, so the statusline step abandons and only
    // the footer — matched by its own opener — stays cut.
    let odd = [SAID, CHROME[2], "one", "two", "three", CHROME[4]];
    assert_eq!(cut(&odd), &odd[..odd.len() - 1]);

    // A composer whose staged text is taller than half the capture: the
    // scan runs past its cap without meeting a top border, so it gives
    // back every row it took and the box survives on screen.
    let mut runaway = vec![SAID];
    runaway.extend((0..8).map(|_| "  typed"));
    runaway.extend_from_slice(&CHROME[2..]);
    assert_eq!(
        cut(&runaway),
        &runaway[..runaway.len() - 3],
        "the footer, the statusline and the bottom border keep their anchors"
    );
}

/// `capture-pane -p -J` of a live claude 2.1.237 at 72 columns on
/// 2026-08-21, with a task typed into the composer and wrapped over three
/// rows. Verbatim, trailing spaces and the no-break space after the
/// chevron included: the rows above are transcriptions, and what a
/// transcription cannot carry is exactly what these predicates walk over.
const CAPTURED: [&str; 9] = [
    "what the agent said",
    "  tmux detected · scroll with PgUp/PgDn · or add 'set -g mouse on' to…",
    "────────────────────────────────────────────────── execute amx-v2 tail ─",
    "❯\u{a0}check every call site 1 check every call site 2 check every call      ",
    "  site 3 check every call site 4 check every call site 5 check every    ",
    "  call site 6                                             ",
    "────────────────────────────────────────────────────────────────────────",
    "  Opus 5 (1M context) (1M context) │ ◈ 0% │ amx-main (main) │ ◖ xhigh",
    "  ⏵⏵ accept edits on (shift+tab to cycle)               ",
];

#[test]
fn view_tail_cuts_what_a_live_vendor_actually_drew() {
    // The pane's own padding under the last row the vendor drew on.
    let mut screen = CAPTURED.to_vec();
    screen.push("");

    // The warning claude renders flush against the composer's top border
    // with no blank row between them stays: it is above the box, and a
    // walk that ran upward until a blank row would have eaten it.
    assert_eq!(cut(&screen), &CAPTURED[..2]);
}

#[test]
fn view_tail_keeps_the_capture_the_card_has_no_question_to_draw() {
    let asked = |question: Option<&str>| {
        let mut card = asking(&[], Some(Kind::Question));
        card.body = format!("{SAID}\n\nWhich features should be enabled?\n");
        card.question = question.map(str::to_string);
        card
    };

    // The one asking card that still shows its pane: amx missed the call
    // that drew the menu, so the pane is the only place the question is
    // written at all.
    let kept = said(asked(None), 24);
    assert!(
        kept.contains(&"Which features should be enabled?".to_string()),
        "{kept:?}"
    );

    // And with the question on it, the card is the question block alone:
    // the pane under it is the same box behind an echo of the prompt.
    let block = said(asked(Some("Which features should be enabled?")), 24);
    assert!(block.is_empty(), "{block:?}");
}

/// What a card's body says, with the paint it says it in set aside.
fn said(card: Card, rows: usize) -> Vec<String> {
    body(&card.read(), rows, 0)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

/// Every standing there is, so a table over them cannot quietly miss one.
const EVERY_STANDING: [Standing; 8] = [
    Standing::Merged,
    Standing::Closed,
    Standing::Draft,
    Standing::Failing,
    Standing::Changes,
    Standing::Running,
    Standing::Ready,
    Standing::Open,
];

/// The colour a word on a row was painted in.
fn word_colour(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Color {
    let buffer = cells(screen, size);
    let line: String = (0..size.0)
        .map(|column| buffer[(column, row)].symbol())
        .collect();
    let at = line
        .find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
    buffer[(line[..at].chars().count() as u16, row)].fg
}

/// And the weight it was painted at, for the tests about the muted rows.
fn word_modifier(screen: &Screen, size: (u16, u16), row: u16, word: &str) -> Modifier {
    let buffer = cells(screen, size);
    let line: String = (0..size.0)
        .map(|column| buffer[(column, row)].symbol())
        .collect();
    let at = line
        .find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
    buffer[(line[..at].chars().count() as u16, row)].modifier
}

#[test]
fn rows_keep_the_name_bright_and_dim_what_the_agent_said() {
    let size = (60, 10);
    let screen = showing(
        vec![
            view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
            view("port-import-b2c", Phase::Done, Some("wrote the tests"), 300),
        ],
        None,
    );

    // The cursor opens on the first agent, and the two rows read the same:
    // the name in the terminal's own, what the agent said and how long it
    // worked dim beside it. Which line the cursor is on is the bar's to
    // say, and a row does not change its tones to say it again.
    for (row, name, said, age) in [
        (3, "fix-login-a1b", "wrote the parser", "1m"),
        (4, "port-import-b2c", "wrote the tests", "5m"),
    ] {
        let named = word_modifier(&screen, size, row, name);
        assert!(
            !named.contains(Modifier::DIM) && !named.contains(Modifier::BOLD),
            "{name} is neither dimmed nor weighted: {named:?}"
        );
        for word in [said, age] {
            assert!(
                word_modifier(&screen, size, row, word).contains(Modifier::DIM),
                "{word} is the quiet half of the row"
            );
        }
    }

    // The state is carried by the glyph's colour alone.
    let (glyph, painted, _) = mark(&screen, size, 4);
    assert_eq!((glyph.as_str(), painted), ("●", theme().done));
}

#[test]
fn rows_hovered_name_takes_the_weight_and_nothing_else_does() {
    let size = (60, 10);
    let mut screen = showing(
        vec![
            view("fix-login-a1b", Phase::Done, Some("wrote the parser"), 60),
            view("port-import-b2c", Phase::Done, Some("wrote the tests"), 300),
        ],
        None,
    );
    // The pointer resting on the second agent's line, which is the third
    // item under the heading.
    screen.hover = Some(2);

    let hovered = word_modifier(&screen, size, 4, "port-import-b2c");
    assert!(hovered.contains(Modifier::BOLD), "{hovered:?}");
    assert!(!hovered.contains(Modifier::DIM), "{hovered:?}");
    assert!(
        word_modifier(&screen, size, 4, "wrote the tests").contains(Modifier::DIM),
        "the tint is the name's alone: what the agent said stays quiet"
    );
    assert_eq!(
        behind(&screen, size, 4),
        vec![Color::Reset; 60],
        "and a hover is not the bar"
    );
}

#[test]
fn rows_on_the_project_axis_keep_the_phase_colour_on_the_state_word() {
    // The state word replaces the icon's job under a project heading, so
    // it keeps the phase colour while the words beside it stay muted.
    let size = (60, 10);
    let screen = by_project(vec![
        at(
            view("busy-c3d", Phase::Working, Some("Running Bash"), 3),
            "/src/api",
        ),
        at(
            view("fix-login-a1b", Phase::Done, Some("fixed it"), 60),
            "/src/api",
        ),
    ]);

    assert_eq!(word_colour(&screen, size, 4, "done"), theme().done);
    assert!(word_modifier(&screen, size, 4, "fixed it").contains(Modifier::DIM));
}

#[test]
fn pr_the_row_says_what_the_branchs_request_is_doing() {
    let screen = over_the_forge(
        vec![
            on_a_branch(view("ask-a1b", Phase::Waiting, None, 30), "amx/ask-a1b"),
            on_a_branch(
                view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
                "amx/busy-b2c",
            ),
        ],
        None,
    );
    let size = (60, 10);
    let lines = painted(&screen, size);
    let row = |word: &str| {
        lines
            .iter()
            .position(|line| line.contains(word))
            .unwrap_or_else(|| panic!("no row says {word:?}: {lines:?}"))
    };

    let asking = row("ask-a1b");
    assert!(lines[asking].contains("#12"), "{:?}", lines[asking]);
    assert_eq!(
        word_colour(&screen, size, asking as u16, "#12"),
        theme().failed,
        "a failing check is a thing that was attempted and failed"
    );

    // One column, so the numbers read down the screen rather than
    // wandering with the length of the name beside them.
    let busy = row("busy-b2c");
    let column = |line: &str, word: &str| {
        let at = line.find(word).expect("the number on the row");
        line[..at].chars().count()
    };
    assert_eq!(
        column(&lines[asking], "#12"),
        column(&lines[busy], "#40"),
        "{lines:?}"
    );
    assert!(
        lines[busy].contains("Running Bash"),
        "and what the agent is doing is still on it: {:?}",
        lines[busy]
    );
    assert!(
        !lines[busy].contains("#7"),
        "the row is read for the attempt that is still going, and the \
         one before it is on the card: {:?}",
        lines[busy]
    );
}

#[test]
fn pr_costs_the_list_nothing_where_no_branch_has_one() {
    // Which is every list on a machine with no forge on it, and the whole
    // of what such a machine loses.
    let fleet = || {
        vec![
            view("ask-a1b", Phase::Waiting, None, 30),
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
        ]
    };
    assert_eq!(
        painted(&over_the_forge(fleet(), None), (60, 10)),
        painted(&showing(fleet(), None), (60, 10)),
        "a fleet with no requests draws the rows amx always drew"
    );
}

#[test]
fn pr_the_card_lists_every_request_the_branch_has() {
    let mut card = asking(&[], None);
    card.id = "busy-b2c".to_string();
    card.phase = Phase::Working;
    card.question = None;
    let screen = over_the_forge(
        vec![on_a_branch(
            view("busy-b2c", Phase::Working, Some("Running Bash"), 3),
            "amx/busy-b2c",
        )],
        Some(card),
    );
    let size = (60, 14);
    let lines = painted(&screen, size);

    let row = lines
        .iter()
        .position(|line| line.contains("#40 open"))
        .unwrap_or_else(|| panic!("nothing on the card lists them: {lines:?}"));
    assert!(
        lines[row].contains("#7 merged"),
        "every request the branch has, each with the question its colour \
         came from: {:?}",
        lines[row]
    );
    assert!(
        lines[..row].iter().any(|line| line.starts_with('╭')),
        "on the card rather than on the row behind it: {lines:?}"
    );
    assert_eq!(word_colour(&screen, size, row as u16, "#7"), theme().done);
}

#[test]
fn pr_every_standing_has_a_word_and_a_colour() {
    // Eight standings and eight words, so a card never says one thing for
    // two of them. The colours are five and are meant to be shared: they
    // answer how it is going, and two standings can have the same answer.
    let said: Vec<&str> = EVERY_STANDING.into_iter().map(Standing::says).collect();
    assert_eq!(
        said.iter().collect::<std::collections::BTreeSet<_>>().len(),
        EVERY_STANDING.len(),
        "{said:?}"
    );
    for standing in EVERY_STANDING {
        assert_eq!(
            request_colour(theme(), standing).bg,
            None,
            "{standing:?} is a word on a row, not a bar under one"
        );
    }
    assert_eq!(
        request_colour(theme(), Standing::Merged).fg,
        Some(theme().done)
    );
    assert_eq!(
        request_colour(theme(), Standing::Failing).fg,
        Some(theme().failed)
    );
    assert_eq!(
        request_colour(theme(), Standing::Changes).fg,
        Some(theme().waiting)
    );
    assert_eq!(
        request_colour(theme(), Standing::Closed).fg,
        Some(theme().stopped)
    );
    assert_eq!(
        request_colour(theme(), Standing::Open).fg,
        None,
        "a request nobody has read yet has nothing to say about how it went"
    );
}

#[test]
fn view_tail_says_so_when_a_capture_is_nothing_but_chrome() {
    let captured = |text: String| {
        let mut card = asking(&[], None);
        card.phase = Phase::Working;
        card.body = text;
        card
    };
    assert_eq!(said(captured(CHROME.join("\n")), 8), [ALL_CHROME]);

    // Which is not what an agent with nothing to say gets: no capture was
    // cut there, and "the pane held only furniture" is a different fact.
    assert!(said(captured(String::new()), 8).is_empty());
}

#[test]
fn view_tail_is_cut_before_the_card_measures_what_it_has() {
    let mut card = asking(&[], None);
    card.phase = Phase::Working;
    card.question = None;
    let mut screen = vec!["what the agent said"];
    screen.extend_from_slice(&CHROME);
    card.body = screen.join("\n");

    // Two borders and the one row left under them, not the six rows the
    // capture has: a card that measured before it cut would spend its
    // height on the vendor's furniture.
    assert_eq!(card_rows(&card.read(), None, &[], false, 60), 3);
}

#[test]
fn view_ages_are_the_readings_own_number_in_the_readings_own_words() {
    // Both the number and the units come from the reading, and the row
    // only asks for them. A row that worked the words out for itself would
    // agree with the table until the next hand touched one of the two, and
    // the person with both open is who finds out.
    for age in [0, 59, 60, 3_599, 3_600, 86_400] {
        let row = drawn(
            vec![view("busy-a1b", Phase::Working, None, age)],
            None,
            WALL,
        )
        .into_iter()
        .find(|line| line.contains("busy-a1b"))
        .expect("the agent's row");
        assert!(
            row.ends_with(&derive::in_words(age)),
            "{age} seconds is drawn as {row:?}"
        );
    }
}

#[test]
fn view_rows_carry_the_worked_seconds_and_not_the_age() {
    // An idle agent's age climbs with every quiet second; what it worked
    // does not, and the column is about the work. The wait and the age
    // stay the card's.
    let mut idle = view("rests-a1b", Phase::Idle, Some("done for now"), 500);
    idle.verdict.worked = 60;
    let row = drawn(vec![idle], None, WALL)
        .into_iter()
        .find(|line| line.contains("rests-a1b"))
        .expect("the agent's row");
    assert!(row.ends_with("1m"), "{row:?}");
}

#[test]
fn view_cuts_text_without_losing_the_last_character_to_the_ellipsis() {
    assert_eq!(fit("short", 10), "short");
    assert_eq!(fit("exactly", 7), "exactly");
    assert_eq!(fit("too long by far", 8), "too lon…");
    assert_eq!(fit("anything", 1), "…");
    assert_eq!(fit("anything", 0), "");
}

#[test]
fn view_a_wide_glyph_in_the_summary_does_not_push_the_age_off_the_edge() {
    // Measured on the wall 2026-08-25: `Hello! 👋` — one char, two
    // columns — shifted everything after it right by one, and the row's
    // age lost its unit to the terminal's edge, reading `5` where every
    // other row read `5m`. A row is measured in columns, not characters.
    let row = drawn(
        vec![view(
            "waves-a1b",
            Phase::Done,
            Some("Hello! 👋 done and dusted"),
            345,
        )],
        None,
        WALL,
    )
    .into_iter()
    .find(|line| line.contains("waves-a1b"))
    .expect("the agent's row");
    assert!(
        row.trim_end().ends_with("5m"),
        "the unit survives the emoji: {row:?}"
    );

    // And the clip itself counts columns: four emoji are eight columns,
    // whole at eight and one emoji plus the ellipsis at four.
    assert_eq!(fit("👋👋👋👋", 8), "👋👋👋👋");
    assert_eq!(fit("👋👋👋👋", 4), "👋…");
    assert_eq!(fit("ab👋cd", 5), "ab👋…");
}
