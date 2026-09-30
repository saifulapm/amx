use super::*;
use crate::config::HarnessConfig;
use crate::derive::{Evidence, Verdict};
use crate::store::{Agent, Ask, Choice, Kind, Meta, Phase, State};
use crate::tmux::{Socket, Spawn};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

/// Keys from a script, then `Gone` once it runs out.
struct Script(std::vec::IntoIter<Typed>);

impl Keys for Script {
    fn next(&mut self, _: Duration) -> Typed {
        self.0.next().unwrap_or(Typed::Gone)
    }
}

/// The titles the view set, in order.
#[derive(Default)]
struct Said(Vec<String>);

impl Titles for Said {
    fn say(&mut self, said: &str) {
        self.0.push(said.to_string());
    }
}

fn ctrl(key: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)
}

fn alt(key: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(key), KeyModifiers::ALT)
}

/// The keys of `text`, one per character.
fn word(text: &str) -> Vec<KeyCode> {
    text.chars().map(KeyCode::Char).collect()
}

/// The fallback vendor's furniture, for bodies planted straight into a card.
/// Those tests are about where the card stands, so no body carries chrome.
fn chrome() -> &'static crate::furniture::Furniture {
    crate::rules::of("").furniture()
}

/// Records an agent whose command ended `ago` seconds ago, with no pane.
fn finished(root: &Path, id: &str, result: &str, ago: u64) {
    finished_in(root, id, result, ago, "/srv/app");
}

/// [`finished`] with the agent's directory given.
fn finished_in(root: &Path, id: &str, result: &str, ago: u64, dir: &str) {
    let at = now() - ago;
    let agent = Agent::create(
        root,
        &Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            dir: PathBuf::from(dir),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx-not-a-server".to_string()),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: at,
        },
    )
    .unwrap();
    // Written directly so the test sets when the agent ended, which orders the
    // finished rows.
    let state = State {
        state: Phase::Done,
        exit: Some(0),
        result: Some(result.to_string()),
        since: at,
        last_event: at,
        ..State::default()
    };
    std::fs::write(
        agent.dir().join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
}

/// Runs the view on a 50x10 screen until the script runs out, and returns the
/// exit code and the last frame.
fn held(root: &Path, keys: &[KeyCode]) -> (i32, String) {
    pressing(
        root,
        keys.iter()
            .map(|code| KeyEvent::new(*code, KeyModifiers::NONE))
            .collect(),
    )
}

/// [`held`] for a script with chords in it.
fn pressing(root: &Path, keys: Vec<KeyEvent>) -> (i32, String) {
    driving(root, keys.into_iter().map(Typed::Key).collect())
}

/// [`held`] for a script of any input, pastes included.
fn driving(root: &Path, script: Vec<Typed>) -> (i32, String) {
    drawn_about(root, &Scope::default(), script, None)
}

/// [`held`] for a view with a scope and, optionally, a view file.
fn drawn_about(
    root: &Path,
    scope: &Scope,
    script: Vec<Typed>,
    remembering: Option<&Path>,
) -> (i32, String) {
    drawn_painted(root, scope, script, remembering, Painting::default())
}

/// [`drawn_about`] with a given theme.
fn drawn_painted(
    root: &Path,
    scope: &Scope,
    script: Vec<Typed>,
    remembering: Option<&Path>,
    painting: Painting,
) -> (i32, String) {
    let (code, buffer) = buffered(root, scope, script, remembering, painting);
    let screen = (0..10)
        .map(|row| {
            (0..50)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (code, screen)
}

/// The last frame's buffer, for tests about styling.
fn buffered(
    root: &Path,
    scope: &Scope,
    script: Vec<Typed>,
    remembering: Option<&Path>,
    painting: Painting,
) -> (i32, Buffer) {
    let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
    let code = watch(
        root,
        &Config::default(),
        None,
        scope,
        &mut terminal,
        &mut Script(script.into_iter()),
        None,
        remembering,
        &mut Said::default(),
        painting,
    )
    .unwrap();
    (code, terminal.backend().buffer().clone())
}

/// The last frame of a view run under `config`.
fn drawn_under(root: &Path, config: &Config, script: Vec<Typed>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
    watch(
        root,
        config,
        None,
        &Scope::default(),
        &mut terminal,
        &mut Script(script.into_iter()),
        None,
        None,
        &mut Said::default(),
        Painting::default(),
    )
    .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..10)
        .map(|row| {
            (0..50)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How many frames the view drew for the script.
fn frames(root: &Path, script: Vec<Typed>) -> usize {
    let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
    watch(
        root,
        &Config::default(),
        None,
        &Scope::default(),
        &mut terminal,
        &mut Script(script.into_iter()),
        None,
        None,
        &mut Said::default(),
        Painting::default(),
    )
    .unwrap();
    terminal.get_frame().count()
}

/// The titles the view set while running the script. A title is set only
/// when it changes.
fn titles(root: &Path, script: Vec<Typed>) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
    let mut said = Said::default();
    watch(
        root,
        &Config::default(),
        None,
        &Scope::default(),
        &mut terminal,
        &mut Script(script.into_iter()),
        None,
        None,
        &mut said,
        Painting::default(),
    )
    .unwrap();
    said.0
}

/// Records an agent that stopped on a question `ago` seconds ago, on a socket
/// no tmux server listens on.
fn waiting(root: &Path, id: &str, ago: u64) {
    let at = now() - ago;
    let agent = Agent::create(
        root,
        &Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx-not-a-server".to_string()),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: at,
        },
    )
    .unwrap();
    let state = State {
        state: Phase::Waiting,
        question: Some("Do you want to proceed?".to_string()),
        options: vec!["Yes".to_string(), "No".to_string()],
        kind: Some(Kind::Permission),
        since: at,
        last_event: at,
        ..State::default()
    };
    std::fs::write(
        agent.dir().join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
}

#[test]
fn view_draws_the_records_before_it_asks_tmux_about_a_pane() {
    let root = TempDir::new().unwrap();
    waiting(root.path(), "asks-a1b", 30);

    // The opening frame comes from the records, which say the agent is waiting.
    // The first reading asks tmux, finds no pane and calls it stopped, so the
    // title changes between the first two frames.
    let said = titles(
        root.path(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
    );
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said[0].contains("1 waiting"),
        "the records alone, on the opening frame: {said:?}"
    );
    assert_eq!(
        said[1], "amx",
        "and the reading straight after it, which waited for no keystroke"
    );
}

#[test]
fn a_theme_that_would_not_read_says_why_where_the_view_says_everything() {
    let root = TempDir::new().unwrap();

    // A theme that fails to load falls back to the built-in palette, and the
    // view has to say so.
    let (_, screen) = drawn_painted(
        root.path(),
        &Scope::default(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
        None,
        Painting {
            warnings: vec!["no theme `solarized`".to_string()],
            ..Painting::default()
        },
    );
    assert!(
        screen.contains("no theme `solarized`"),
        "the reason is nowhere on the screen:\n{screen}"
    );
}

#[test]
fn a_theme_written_under_the_open_view_is_what_the_next_frame_is_painted_in() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "fix-login-a1b", "did what it was asked", 60);
    let themes = TempDir::new().unwrap();

    // The view opens on a theme name with no file yet, so it uses the built-in
    // palette until the file appears.
    let watching = Watch::of_in(themes.path(), "mine");
    std::fs::write(themes.path().join("mine.toml"), "done = \"#ff00ff\"\n").unwrap();

    let (_, buffer) = buffered(
        root.path(),
        &Scope::default(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
        None,
        Painting {
            watching,
            ..Painting::default()
        },
    );
    assert!(
        buffer
            .content()
            .iter()
            .any(|cell| cell.fg == Color::Rgb(255, 0, 255)),
        "nothing was painted in the colour the file says"
    );
}

#[test]
fn a_theme_put_right_takes_its_own_complaint_off_the_screen() {
    let root = TempDir::new().unwrap();
    let themes = TempDir::new().unwrap();
    let watching = Watch::of_in(themes.path(), "mine");
    std::fs::write(themes.path().join("mine.toml"), "done = \"#ff00ff\"\n").unwrap();

    // Opened on a theme file that fails to parse, so the view shows why.
    let (_, screen) = drawn_painted(
        root.path(),
        &Scope::default(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
        None,
        Painting {
            watching,
            warnings: vec!["ignoring mine.toml, painting the default".to_string()],
            ..Painting::default()
        },
    );
    assert!(
        !screen.contains("ignoring mine.toml"),
        "the complaint outlived the file that earned it:\n{screen}"
    );
}

#[test]
fn a_theme_says_nothing_over_a_notice_somebody_else_put_there() {
    let mut screen = Screen::default();
    screen.say_of_the_theme(&["ignoring mine.toml".to_string()]);
    screen.notice = Some(Notice::Failed("could not stop fix-login-a1b".to_string()));

    // The theme file is fixed, but the notice now showing is not the theme's.
    screen.say_of_the_theme(&[]);
    let said = match &screen.notice {
        Some(Notice::Failed(said) | Notice::Refused(said) | Notice::Advice(said)) => said.as_str(),
        None => "nothing at all",
    };
    assert_eq!(said, "could not stop fix-login-a1b");
}

#[test]
fn header_dials_start_at_what_the_config_file_asked_for() {
    let config = Config {
        model: Some("opus".to_string()),
        permission: Some("plan".to_string()),
        effort: Some("high".to_string()),
        worktrees: false,
        max_agents: 3,
        ..Config::default()
    };
    let profile = Profile::open(
        &config,
        Some(config.max_agents),
        Some(Path::new("/home/dev/code/amx")),
        Some(Path::new("/home/dev")),
    );

    assert_eq!(profile.model, "opus");
    assert_eq!(profile.permission, "plan");
    assert_eq!(profile.effort, "high");
    assert!(!profile.worktree);
    assert_eq!(profile.cap, Some(3), "what the door said to count against");
    assert_eq!(
        profile.dir, "~/code/amx",
        "where the next one will run, written the way the headings write it"
    );
}

#[test]
fn header_dials_offer_what_the_vendor_declares_and_come_back_round() {
    let mut profile = Profile::default();
    assert_eq!(
        profile.model,
        registry::DEFAULT,
        "nothing in config, so the vendor's own choice"
    );

    for want in ["fable", "opus", "sonnet", "haiku", registry::DEFAULT] {
        profile.cycle_model();
        assert_eq!(profile.model, want, "claude's own cycle, in its own order");
    }

    profile.cycle_permission();
    assert_eq!(profile.permission, "acceptEdits");

    assert!(profile.worktree);
    profile.toggle_worktree();
    assert!(!profile.worktree);
}

#[test]
fn profile_the_effort_dial_offers_the_levels_the_vendor_declares() {
    let mut profile = Profile::default();
    assert_eq!(
        profile.effort,
        registry::DEFAULT,
        "how hard it thinks is claude's own answer until somebody says"
    );

    for want in ["low", "medium", "high", "xhigh", "max", registry::DEFAULT] {
        profile.cycle_effort();
        assert_eq!(profile.effort, want, "claude's own levels, in its order");
    }

    // A level the new vendor does not take falls back to the sentinel.
    profile.effort = "xhigh".to_string();
    profile.cycle_vendor();
    assert_eq!(profile.agent, "pi");
    assert_eq!(profile.effort, "xhigh", "and pi has a level of that name");
    profile.effort = "minimal".to_string();
    profile.cycle_vendor();
    assert_eq!(profile.agent, "codex");
    assert_eq!(
        profile.effort,
        registry::DEFAULT,
        "codex has no level called minimal, so the dial rests at the sentinel"
    );
    profile.effort = "minimal".to_string();
    profile.cycle_vendor();
    assert_eq!(profile.agent, "opencode");
    assert_eq!(
        profile.effort,
        registry::DEFAULT,
        "opencode has no effort dial, so the dial rests at the sentinel"
    );
    profile.effort = "minimal".to_string();
    profile.cycle_vendor();
    assert_eq!(profile.agent, "claude");
    assert_eq!(
        profile.effort,
        registry::DEFAULT,
        "claude has no level called minimal, so the dial is where it was \
             before anybody turned it"
    );
}

#[test]
fn header_dials_the_model_dial_stays_on_the_harness_on_the_row() {
    // alt+m walks the models of the harness on the row and never another
    // harness's; switching harness is the vendor key's job.
    let told = |models: &[&str]| HarnessConfig {
        models: models.iter().map(|model| model.to_string()).collect(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    let config = Config {
        agent: "claude".to_string(),
        harnesses: BTreeMap::from([
            ("claude".to_string(), told(&["opus", "sonnet"])),
            ("pi".to_string(), told(&["openai/gpt-5"])),
        ]),
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);
    assert_eq!(profile.model, registry::DEFAULT);

    for want in ["opus", "sonnet", registry::DEFAULT] {
        profile.cycle_model();
        assert_eq!(profile.model, want, "claude's own list, in its own order");
        assert_eq!(profile.agent, "claude", "and the harness never moves");
    }

    // After a vendor switch, pi's own models.
    profile.cycle_vendor();
    assert_eq!(profile.agent, "pi");
    assert_eq!(profile.model, registry::DEFAULT);
    for want in ["openai/gpt-5", registry::DEFAULT] {
        profile.cycle_model();
        assert_eq!(profile.model, want, "pi's own list");
        assert_eq!(profile.agent, "pi", "and still pi");
    }
}

#[test]
fn header_dials_the_model_dial_takes_this_harnesss_written_models() {
    let told = |models: &[&str]| HarnessConfig {
        models: models.iter().map(|model| model.to_string()).collect(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    let config = Config {
        agent: "claude --add-dir ..".to_string(),
        permission: Some("plan".to_string()),
        harnesses: BTreeMap::from([
            ("claude".to_string(), told(&["big"])),
            ("pi".to_string(), told(&["openai/gpt-5"])),
        ]),
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);

    profile.cycle_model();
    assert_eq!(
        profile.model, "big",
        "the file's list, not the dial's cycle"
    );
    assert_eq!(
        profile.agent, "claude --add-dir ..",
        "spelled the way the file spells the harness it names"
    );
    assert_eq!(profile.permission, "plan");

    profile.cycle_model();
    assert_eq!(
        profile.model,
        registry::DEFAULT,
        "the file's list is the whole dial, and past it is the sentinel"
    );
    assert_eq!(
        profile.agent, "claude --add-dir ..",
        "and the harness line is the one the file wrote"
    );
}

#[test]
fn header_dials_the_vendor_cycle_starts_at_the_command_config_asked_for() {
    // An unregistered vendor from the config is still where the cycle starts
    // and returns to.
    let config = Config {
        agent: "mock-claude".to_string(),
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);

    profile.cycle_vendor();
    assert_eq!(profile.agent, "claude");
    assert!(
        profile.model_dial().is_some(),
        "and the dials it declares come with it"
    );

    profile.cycle_vendor();
    assert_eq!(profile.agent, "pi", "then the other registered vendors");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "codex");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "opencode");

    profile.cycle_vendor();
    assert_eq!(
        profile.agent, "mock-claude",
        "and round again to the file's own answer"
    );
}

#[test]
fn header_dials_a_turned_vendor_takes_the_dials_it_declares_and_no_others() {
    let config = Config {
        agent: "mock-claude".to_string(),
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);

    profile.cycle_vendor();
    profile.cycle_model();
    assert_eq!(profile.model, "fable");

    profile.cycle_vendor();
    assert_eq!(profile.agent, "pi");
    assert_eq!(
        profile.model,
        registry::DEFAULT,
        "a model belongs to the harness that runs it, so a harness \
             somebody turns to starts at its own answer"
    );
    profile.cycle_vendor();
    assert_eq!(profile.agent, "codex");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "opencode");

    profile.cycle_vendor();
    assert_eq!(
        profile.agent, "mock-claude",
        "and round again to the command amx has no entry for"
    );
    assert_eq!(
        profile.model,
        registry::DEFAULT,
        "a vendor that declares no model dial is not started with a model"
    );
    assert_eq!(profile.launching(&config).model, None);
}

#[test]
fn header_dials_the_vendor_key_leaves_a_command_it_could_not_put_back() {
    // Four registered vendors, so the fourth press is back at claude.
    let mut profile = Profile::default();
    profile.cycle_vendor();
    assert_eq!(profile.agent, "pi");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "codex");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "opencode");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "claude");

    // A vendor command with arguments: only a full cycle back to the configured
    // command restores them.
    let config = Config {
        agent: "claude --add-dir ..".to_string(),
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);
    profile.cycle_vendor();
    assert_eq!(profile.agent, "pi");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "codex");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "opencode");
    profile.cycle_vendor();
    assert_eq!(profile.agent, "claude --add-dir ..");
}

#[test]
fn header_dials_a_vendor_amx_never_heard_of_declares_none() {
    // The config loader clears dials an unregistered vendor cannot take; the
    // profile offers none either.
    let config = Config {
        agent: "mock-claude".to_string(),
        model: Some("opus".to_string()),
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);

    assert!(profile.model_dial().is_none());
    assert!(profile.permission_dial().is_none());
    assert!(profile.effort_dial().is_none());
    assert_eq!(profile.model, registry::DEFAULT);
    profile.cycle_model();
    assert_eq!(profile.model, registry::DEFAULT, "and nothing to cycle to");
    profile.cycle_effort();
    assert_eq!(profile.effort, registry::DEFAULT, "nor for the effort key");
}

#[test]
fn header_dials_are_the_config_the_next_spawn_is_made_under() {
    let config = Config {
        max_agents: 4,
        ..Config::default()
    };
    let mut profile = Profile::open(&config, None, None, None);

    let resting = profile.launching(&config);
    assert_eq!(
        resting.model, None,
        "the sentinel is said by holding no value, because there is no \
             flag that means what the vendor was going to do anyway"
    );
    assert_eq!(resting.permission, None);
    assert_eq!(resting.effort, None);
    assert!(resting.worktrees);
    assert_eq!(
        resting.max_agents, 4,
        "and the rest of the file is what it was"
    );

    profile.cycle_model();
    profile.cycle_permission();
    profile.cycle_effort();
    profile.toggle_worktree();

    let turned = profile.launching(&config);
    assert_eq!(turned.model.as_deref(), Some("fable"));
    assert_eq!(turned.permission.as_deref(), Some("acceptEdits"));
    assert_eq!(turned.effort.as_deref(), Some("low"));
    assert!(!turned.worktrees, "whichever way the file had it");
}

#[test]
fn header_dials_turn_under_the_keys_that_say_so() {
    let root = TempDir::new().unwrap();
    let config = Config {
        agent: "mock-claude".to_string(),
        ..Config::default()
    };
    let mut screen = Screen {
        profile: Profile::open(&config, None, None, None),
        ..Screen::default()
    };
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, alt('v'));
    assert_eq!(
        screen.profile.agent, "mock-claude",
        "the vendor is under the initial of the dial the row names, and \
             the key it used to be under turns nothing"
    );
    press(&mut screen, alt('a'));
    assert_eq!(screen.profile.agent, "claude");
    press(&mut screen, alt('m'));
    assert_eq!(screen.profile.model, "fable");
    press(&mut screen, alt('e'));
    assert_eq!(screen.profile.effort, "low");
    press(
        &mut screen,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    assert_eq!(screen.profile.permission, "acceptEdits");
    press(&mut screen, alt('w'));
    assert!(!screen.profile.worktree);

    // The dials also turn while a task line is open.
    screen.mode = Mode::Typing(Composer::new(Asking::Task));
    press(&mut screen, alt('m'));
    assert_eq!(screen.profile.model, "opus");
    press(
        &mut screen,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
    );
    assert_eq!(screen.profile.permission, "auto");

    press(&mut screen, KeyEvent::from(KeyCode::Tab));
    press(&mut screen, KeyEvent::from(KeyCode::Char('m')));
    let Mode::Typing(composer) = &screen.mode else {
        panic!("still typing")
    };
    assert_eq!(
        composer.text, "@m",
        "a letter without the chord is a letter, and tab without it is the \
             mark the line opens its agents with"
    );
    assert_eq!(
        screen.profile.permission, "auto",
        "and tab on its own is not the chord that turns the dial"
    );
}

/// A reading of an agent, as derive hands one to the view.
fn reading(id: &str, phase: Phase, state: State) -> View {
    View::new(
        Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "port the importer".to_string(),
            agent: None,
            model: None,
            effort: None,
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx-not-a-server".to_string()),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        },
        state,
        Verdict {
            phase,
            evidence: Evidence::Hooks,
            rule: None,
            age: 29,
            worked: 29,
        },
    )
}

/// An agent stopped on a vendor question that takes words as well as a key.
fn stopped_on_a_question(id: &str) -> View {
    reading(
        id,
        Phase::Waiting,
        State {
            state: Phase::Waiting,
            question: Some("Which fixture should the port keep?".to_string()),
            options: vec!["the sqlite one".to_string(), "the docker one".to_string()],
            kind: Some(Kind::Question),
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )
}

/// The same question, marked as taking more than one choice.
fn stopped_on_a_checkbox_question(id: &str) -> View {
    let mut view = stopped_on_a_question(id);
    view.state.asking = vec![Ask {
        header: Some("Fixtures".to_string()),
        text: view.state.question.clone().unwrap_or_default(),
        options: view
            .state
            .options
            .iter()
            .map(|label| Choice {
                label: label.clone(),
                description: None,
                preview: None,
            })
            .collect(),
        multi: true,
        answer: None,
    }];
    view
}

/// A screen showing these agents, cursor where it opens.
fn watching(views: Vec<View>) -> Screen {
    let mut screen = Screen::default();
    screen.list.show(views);
    screen
}

#[test]
fn the_view_keeps_the_projects_its_agents_run_in_for_a_line_to_offer() {
    // The projects a `d:` offers come from the records: an agent in an amx tree
    // belongs to the repository the tree was cut from, and two agents in one
    // project give one entry.
    let mut screen = Screen::default();
    let mut cut = reading("port-a1b", Phase::Working, State::default());
    cut.meta.worktree = Some(PathBuf::from("/srv/api/.amx/worktrees/port-a1b"));

    screen.showing(vec![
        reading("fix-login-b2c", Phase::Working, State::default()),
        cut,
        reading("port-c3d", Phase::Working, State::default()),
    ]);
    assert_eq!(
        screen.projects,
        [PathBuf::from("/srv/app"), PathBuf::from("/srv/api")]
    );
}

/// The agent on the wall before a line was typed.
fn was_there() -> View {
    reading("fix-login-a1b", Phase::Idle, State::default())
}

/// The agent the line started.
fn just_started() -> View {
    reading("port-b2c", Phase::Working, State::default())
}

#[test]
fn the_cursor_lands_on_the_agent_the_line_started_when_the_wall_shows_it() {
    let mut screen = watching(vec![was_there()]);
    assert_eq!(screen.list.selected().unwrap().id(), "fix-login-a1b");

    // Just after a start the agent runs but the wall is a reading old, so it
    // has no row yet.
    screen.started = Some("port-b2c".to_string());
    screen.showing(vec![was_there(), just_started()]);
    screen.land_on_what_was_started();
    assert_eq!(screen.list.selected().unwrap().id(), "port-b2c");
    assert!(
        screen.started.is_none(),
        "and the cursor is done following it"
    );
}

#[test]
fn the_cursor_waits_for_no_agent_the_narrowing_on_the_screen_hides() {
    let mut screen = watching(vec![was_there()]);
    screen.started = Some("port-b2c".to_string());

    // The next reading is narrowed and does not include the new agent.
    screen.showing(vec![was_there()]);
    screen.land_on_what_was_started();
    assert_eq!(screen.list.selected().unwrap().id(), "fix-login-a1b");

    // The started agent is dropped, so a later reading that shows it leaves the
    // cursor alone.
    screen.showing(vec![was_there(), just_started()]);
    screen.land_on_what_was_started();
    assert_eq!(screen.list.selected().unwrap().id(), "fix-login-a1b");
}

#[test]
fn card_opens_on_the_question_under_the_cursor_with_the_line_to_answer_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };

    press(&mut screen, KeyCode::Char(' '));
    assert!(
        screen.card.as_ref().is_some_and(Card::asks),
        "the card is open on what the agent is asking"
    );
    assert!(
        screen.answering().is_some_and(|line| line.text.is_empty()),
        "with the line to answer it on, and nothing typed at it yet"
    );

    // At a question that takes several choices, a digit names one box and goes
    // on the line; the line waits for the rest and the submit key.
    let mut screen = watching(vec![stopped_on_a_checkbox_question("ask-a1b")]);
    press(&mut screen, KeyCode::Char(' '));
    press(&mut screen, KeyCode::Char('2'));
    assert_eq!(screen.answering().expect("still typing").text, "2");

    // Esc closes the line and the card together.
    press(&mut screen, KeyCode::Esc);
    assert!(screen.card.is_none());
    assert!(screen.answering().is_none());
    assert!(matches!(screen.mode, Mode::List), "back on the agents");
}

#[test]
fn card_reads_a_digit_at_a_question_that_takes_one_choice_as_that_choice() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    let key = |code| KeyEvent::from(code);
    screen
        .act(key(KeyCode::Char(' ')), root.path(), &config, None)
        .unwrap();

    // A digit is a choice only on an empty line and only when the card shows
    // that many choices; a 3 at a two-choice question is a character.
    let line = screen.answering().expect("the line to answer on");
    assert_eq!(
        screen.picking(line, key(KeyCode::Char('2'))),
        Some(("ask-a1b".to_string(), "2".to_string()))
    );
    assert_eq!(screen.picking(line, key(KeyCode::Char('3'))), None);
    assert_eq!(
        screen.picking(line, KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT)),
        None,
        "and alt+1 is the key that reaches the first agent on the wall"
    );

    // The digit is sent as the answer and never reaches the line. No agent is
    // behind a hand-written record, so the reply fails with a notice.
    screen
        .act(key(KeyCode::Char('2')), root.path(), &config, None)
        .unwrap();
    assert!(
        screen.answering().is_none(),
        "the digit was the answer rather than a character on the line"
    );
    assert!(screen.notice.is_some(), "and what came of it is said");

    // On a line with text, a digit is a character.
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    for code in [KeyCode::Char(' '), KeyCode::Char('k'), KeyCode::Char('2')] {
        screen.act(key(code), root.path(), &config, None).unwrap();
    }
    assert_eq!(screen.answering().expect("still typing").text, "k2");
}

/// An agent that finished with the answer `result`.
fn finished_saying(id: &str, result: &str) -> View {
    reading(
        id,
        Phase::Done,
        State {
            state: Phase::Done,
            exit: Some(0),
            result: Some(result.to_string()),
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )
}

#[test]
fn card_holds_the_whole_recorded_answer_and_not_the_pane() {
    // A finished turn whose record holds an answer shows that whole answer,
    // read from the top, for idle, done, failed and stopped alike. The pane is
    // not read: claude scrolls its viewport on its own.
    let long: String = (0..60).map(|n| format!("line {n}\n")).collect();
    for phase in [Phase::Idle, Phase::Done, Phase::Failed, Phase::Stopped] {
        let agent = reading(
            "said-a1b",
            phase,
            State {
                state: phase,
                result: Some(long.clone()),
                since: 1,
                last_event: 1,
                ..State::default()
            },
        );
        let (card, _) = card_of(
            &agent,
            Path::new(""),
            76,
            Theme::default(),
            &mut Heard::default(),
        );
        assert_eq!(card.body.says(), long, "the whole answer, {phase:?}");
        assert!(card.answer, "an answer reads forward, {phase:?}");
    }

    // A working agent's card is still the pane, whatever the record holds.
    let busy = reading(
        "busy-b2c",
        Phase::Working,
        State {
            state: Phase::Working,
            result: Some("an old answer".to_string()),
            since: 1,
            last_event: 1,
            ..State::default()
        },
    );
    assert!(
        !card_of(
            &busy,
            Path::new(""),
            76,
            Theme::default(),
            &mut Heard::default()
        )
        .0
        .answer
    );

    // An idle agent with no recorded answer falls back to the pane too; there
    // is no pane here, so the card is empty.
    let quiet = reading(
        "quiet-c3d",
        Phase::Idle,
        State {
            state: Phase::Idle,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    );
    let (card, _) = card_of(
        &quiet,
        Path::new(""),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    assert!(!card.answer);
    assert_eq!(card.body.says(), "");
}

#[test]
fn card_on_a_transcript_with_nothing_on_it_yet_is_the_task_the_agent_was_given() {
    // Between a vendor starting and its first turn landing, the named
    // transcript is missing or empty. The card shows the task as the first
    // prompt, with the live stream under it.
    let root = TempDir::new().unwrap();
    let dir = root.path().join("port-a1b");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("live"), "reading the importer\n").unwrap();

    let held = TempDir::new().unwrap();
    let empty = held.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    // A file not created yet, and one created empty.
    for path in [held.path().join("unwritten.jsonl"), empty] {
        let mut view = reading("port-a1b", Phase::Working, State::default());
        view.meta.transcript = Some(path.clone());
        let (card, _) = card_of(
            &view,
            root.path(),
            76,
            Theme::default(),
            &mut Heard::default(),
        );
        let says = card.body.says();
        assert!(
            says.starts_with("❯ port the importer"),
            "the task behind the composer's own glyph, {path:?}:\n{says}"
        );
        assert!(
            says.ends_with("\n\nreading the importer"),
            "and what it is saying now one blank row under it, {path:?}:\n{says}"
        );
        assert!(!card.answer, "read up from the live edge, {path:?}");
    }
}

#[test]
fn card_stands_the_task_in_for_a_working_agents_transcript_and_no_other() {
    // Only a working agent gets the task in place of an empty transcript: an
    // adopted agent names no transcript, an asking card shows only the
    // question, and a finished agent has its answer on the record.
    let root = TempDir::new().unwrap();
    let held = TempDir::new().unwrap();
    let unwritten = held.path().join("unwritten.jsonl");

    let adopted = reading("port-a1b", Phase::Working, State::default());
    assert_eq!(
        card_of(
            &adopted,
            root.path(),
            76,
            Theme::default(),
            &mut Heard::default()
        )
        .0
        .body
        .says(),
        "",
        "an adopted agent has no transcript to stand in for"
    );

    let mut asking = stopped_on_a_question("ask-b2c");
    asking.meta.transcript = Some(unwritten.clone());
    let (card, _) = card_of(
        &asking,
        root.path(),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    assert_eq!(card.body.says(), "", "a card that is asking shows nothing");
    assert!(card.question.is_some());

    let mut done = finished_saying("done-c3d", "the answer");
    done.meta.transcript = Some(unwritten);
    let (card, _) = card_of(
        &done,
        root.path(),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    assert_eq!(card.body.says(), "the answer", "the answer it left");
    assert!(card.answer);
}

/// Writes a command's printed output beside its record.
fn printed(root: &Path, id: &str, output: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("output"), output).unwrap();
    dir
}

#[test]
fn card_on_a_command_row_is_what_the_command_printed() {
    // A command row's card is its output file, whole and in its own colours. No
    // furniture is cut, because the anchors belong to vendors and a command
    // that prints a rule and a prompt is printing its own output.
    let root = TempDir::new().unwrap();
    printed(
        root.path(),
        "build-a1b",
        "make: entering\n\n────\n❯ \n────\n  statusline\n",
    );
    // The trailing blank of the prompt row and the row after the last newline
    // are cells the command never wrote, so they are not in the reading.
    let output = "make: entering\n\n────\n❯\n────\n  statusline";

    // While it runs, the card reads up from the end.
    let running = reading("build-a1b", Phase::Working, State::default());
    let (card, _) = card_of(
        &running,
        root.path(),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    assert_eq!(card.body.says(), output);
    assert!(!card.answer, "a running command's card follows its output");

    // Once it has ended, the card reads from the top.
    for phase in [Phase::Done, Phase::Failed, Phase::Stopped] {
        let ended = reading(
            "build-a1b",
            phase,
            State {
                state: phase,
                exit: Some(0),
                since: 1,
                last_event: 1,
                ..State::default()
            },
        );
        let (card, _) = card_of(
            &ended,
            root.path(),
            76,
            Theme::default(),
            &mut Heard::default(),
        );
        assert_eq!(card.body.says(), output, "{phase:?}");
        assert!(card.answer, "read forward, {phase:?}");
        assert_eq!(card.body.anchor(), 0, "from the top, {phase:?}");
    }
}

#[test]
fn card_on_a_vendor_that_died_before_it_spoke_is_what_it_printed() {
    // A vendor that died before announcing a session has no conversation and no
    // answer; its boot output is the card.
    let root = TempDir::new().unwrap();
    printed(
        root.path(),
        "fix-login-a1b",
        "could not read the state file\n",
    );
    let mut view = reading(
        "fix-login-a1b",
        Phase::Failed,
        State {
            state: Phase::Failed,
            exit: Some(1),
            ..State::default()
        },
    );
    view.meta.agent = Some("claude".to_string());

    let (card, _) = card_of(
        &view,
        root.path(),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    assert_eq!(card.body.says(), "could not read the state file");
    assert!(card.answer, "a dead vendor's card is read forward");
}

#[test]
fn card_on_a_long_command_row_is_the_end_of_what_it_printed() {
    // A running build's log keeps growing, so the card holds the end of it:
    // 3840 rows of 80 bytes, whose last 256 KiB starts on row 565.
    let root = TempDir::new().unwrap();
    let log: String = (1..=3840)
        .map(|n| format!("{:<79}\n", format!("row {n}")))
        .collect();
    printed(root.path(), "build-a1b", &log);

    let running = reading("build-a1b", Phase::Working, State::default());
    let (card, _) = card_of(
        &running,
        root.path(),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    let says = card.body.says();
    let rows: Vec<&str> = log.lines().map(str::trim_end).collect();
    assert!(
        says.len() <= crate::store::OUTPUT_TAIL as usize && rows.join("\n").ends_with(&says),
        "a quarter megabyte of the log at most, and the end of it"
    );
    assert_eq!(
        says.lines().next().unwrap().trim_end(),
        "row 565",
        "opening on a whole row, never on the first row of a long log"
    );
    assert_eq!(
        says.lines().last().unwrap().trim_end(),
        "row 3840",
        "and ending on the last row the command printed"
    );
}

#[test]
fn card_on_a_command_that_has_printed_nothing_is_empty() {
    // An empty output file is an empty card. The pane is not captured: that
    // would read another program's screen against a vendor's anchors.
    let root = TempDir::new().unwrap();
    printed(root.path(), "quiet-a1b", "");

    let quiet = reading("quiet-a1b", Phase::Working, State::default());
    let (card, _) = card_of(
        &quiet,
        root.path(),
        76,
        Theme::default(),
        &mut Heard::default(),
    );
    assert_eq!(card.body.says(), "");
}

#[test]
fn card_cuts_a_pane_with_the_furniture_of_the_vendor_the_record_names() {
    // pi's chrome has none of claude's anchors, so walking a pi pane with the
    // fallback anchors would leave pi's box and stats line on the card. The
    // anchors come from the command recorded at spawn.
    let pane = [
        " the work itself",
        "",
        "────────────────────────────",
        "",
        "────────────────────────────",
        "~/srv/app",
        "$0.001 (sub) 0.5%/264k (auto)",
    ];
    let ran = |agent: Option<&str>| {
        let mut view = reading("fix-login-a1b", Phase::Working, State::default());
        view.meta.agent = agent.map(str::to_string);
        crate::furniture::cut(own_chrome(&view.meta), &pane).to_vec()
    };

    assert_eq!(ran(Some("pi")), [" the work itself", ""]);
    assert_eq!(
        ran(None),
        pane,
        "and a record naming no command keeps the reading it has always had"
    );
}

#[test]
fn card_reads_the_recorded_answer_rather_than_taking_a_copy_of_it() {
    // The card walks the record's answer where it lies instead of copying it
    // first; a question card is retaken on every pass, so the copy would
    // repeat.
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    screen.look = Look::Screen;

    let walked = paint::walks();
    screen.follow_the_cursor();
    assert_eq!(
        paint::walks(),
        walked + 1,
        "walked where the card was built, and once"
    );
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("the answer".to_string())
    );
}

#[test]
fn card_pages_under_the_page_keys_and_a_cursor_move_resets_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };

    press(&mut screen, KeyCode::Char(' '));
    assert!(screen.card.is_some(), "the card is open");
    assert_eq!(screen.scroll.away.get(), 0, "on its natural edge");

    // A recorded answer reads down from its top: pgdn leaves the edge, pgup
    // comes back and stops there.
    press(&mut screen, KeyCode::PageDown);
    let away = screen.scroll.away.get();
    assert!(away > 0, "paged away from the top");
    press(&mut screen, KeyCode::PageDown);
    assert!(screen.scroll.away.get() > away, "and further");
    press(&mut screen, KeyCode::PageUp);
    assert_eq!(screen.scroll.away.get(), away, "a page back");
    press(&mut screen, KeyCode::PageUp);
    press(&mut screen, KeyCode::PageUp);
    assert_eq!(screen.scroll.away.get(), 0, "the edge is where it stops");

    // The arrows still move the cursor, and the next card opens on its own
    // edge.
    press(&mut screen, KeyCode::PageDown);
    press(&mut screen, KeyCode::Down);
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-b2c"),
        "the card followed the cursor"
    );
    assert_eq!(screen.scroll.away.get(), 0, "and stands where it opens");
}

#[test]
fn keys_j_and_k_walk_the_list_the_way_the_arrows_do() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the answer"),
        finished_saying("done-b2c", "the other answer"),
    ]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let on = |screen: &Screen| {
        screen
            .list
            .selected()
            .map(|view| view.id().to_string())
            .unwrap_or_default()
    };

    assert_eq!(
        on(&screen),
        "done-a1b",
        "the view opens on the first of them"
    );
    press(&mut screen, KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(on(&screen), "done-b2c", "j walks down");
    press(&mut screen, KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(on(&screen), "done-a1b", "and k walks back up");

    // With a card open the arrows walk and the letters do not: the card's line
    // takes every letter.
    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    press(&mut screen, KeyEvent::from(KeyCode::Down));
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-b2c"),
        "the card followed the arrow"
    );
    press(&mut screen, KeyEvent::from(KeyCode::Char('j')));
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-b2c"),
        "and stood still under the letter"
    );
    assert_eq!(screen.answering().expect("the card's line").text, "j");
}

/// Two agents with distinct tasks. A search also matches the task, so a shared
/// task would match both.
fn a_fleet_to_search() -> Vec<View> {
    let mut asking = stopped_on_a_question("ask-a1b");
    asking.meta.task = "fix the login bug".to_string();
    let mut ported = finished_saying("port-b2c", "the answer");
    ported.meta.task = "port the importer".to_string();
    vec![asking, ported]
}

/// The ids of the agents the list shows.
#[cfg(test)]
fn showing_ids(screen: &Screen) -> Vec<String> {
    screen
        .list
        .items()
        .iter()
        .filter_map(|item| screen.list.agent(*item))
        .map(|view| view.id().to_string())
        .collect()
}

#[test]
fn find_slash_narrows_the_list_as_it_is_typed_and_enter_keeps_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    assert!(
        matches!(&screen.mode, Mode::Typing(line) if matches!(line.asking, Asking::Find)),
        "a line of its own, which is not the task line"
    );
    assert!(
        screen.banded().is_none(),
        "and no band under the list: the list is what is being read while \
             it filters, so nothing dims it and nothing takes its rows"
    );
    assert_eq!(showing_ids(&screen), ["ask-a1b", "port-b2c"]);

    // The list narrows on every keystroke.
    press(&mut screen, KeyEvent::from(KeyCode::Char('p')));
    assert_eq!(
        showing_ids(&screen),
        ["port-b2c"],
        "narrowed as it is typed"
    );
    press(&mut screen, KeyEvent::from(KeyCode::Char('o')));
    assert_eq!(showing_ids(&screen), ["port-b2c"]);
    press(&mut screen, KeyEvent::from(KeyCode::Char('z')));
    assert!(showing_ids(&screen).is_empty(), "and past the last match");
    press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    assert_eq!(showing_ids(&screen), ["port-b2c"], "backspace widens it");

    // Enter closes the line and keeps the narrowing.
    press(&mut screen, KeyEvent::from(KeyCode::Enter));
    assert!(matches!(screen.mode, Mode::List), "back on the agents");
    assert_eq!(showing_ids(&screen), ["port-b2c"], "still narrowed");
    assert!(screen.list.narrowing().is_some());
}

#[test]
fn find_backspace_on_an_empty_line_leaves_the_find() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    assert!(matches!(screen.mode, Mode::List), "the mark is taken back");
    assert_eq!(showing_ids(&screen), ["ask-a1b", "port-b2c"]);

    // A backspace with a character left deletes it; only the next one leaves
    // the find.
    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    press(&mut screen, KeyEvent::from(KeyCode::Char('p')));
    press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    assert!(
        matches!(&screen.mode, Mode::Typing(line) if matches!(line.asking, Asking::Find)),
        "one character back is still the find"
    );
    assert_eq!(showing_ids(&screen), ["ask-a1b", "port-b2c"]);
    press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    assert!(
        matches!(screen.mode, Mode::List),
        "and the next one is the way out"
    );
    assert!(screen.list.narrowing().is_none());
}

#[test]
fn find_esc_clears_the_narrowing_rather_than_leaving_it_behind() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    for key in [KeyCode::Char('/'), KeyCode::Char('p'), KeyCode::Esc] {
        press(&mut screen, KeyEvent::from(key));
    }
    assert!(matches!(screen.mode, Mode::List));
    assert_eq!(
        showing_ids(&screen),
        ["ask-a1b", "port-b2c"],
        "the whole fleet is back"
    );
    assert!(
        screen.list.narrowing().is_none(),
        "esc clears the narrowing rather than leaving one nobody can see \
             the line for"
    );
}

#[test]
fn find_esc_clears_a_narrowing_that_is_already_standing() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    // Found and kept: the line is gone and the narrowing stays.
    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    for key in word("port") {
        press(&mut screen, KeyEvent::from(key));
    }
    press(&mut screen, KeyEvent::from(KeyCode::Enter));
    assert_eq!(showing_ids(&screen), ["port-b2c"]);

    // Esc on the list drops a narrowing too, since it outlives the line it was
    // typed on.
    press(&mut screen, KeyEvent::from(KeyCode::Esc));
    assert_eq!(
        showing_ids(&screen),
        ["ask-a1b", "port-b2c"],
        "the whole fleet is back"
    );
    assert!(screen.list.narrowing().is_none());
}

#[test]
fn find_esc_puts_the_card_away_before_it_touches_the_narrowing() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    for key in word("port") {
        press(&mut screen, KeyEvent::from(key));
    }
    press(&mut screen, KeyEvent::from(KeyCode::Enter));
    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    assert!(screen.card.is_some(), "a card over the narrowed wall");

    // Esc closes the card first, before touching the narrowing.
    press(&mut screen, KeyEvent::from(KeyCode::Esc));
    assert!(screen.card.is_none(), "the card went");
    assert_eq!(showing_ids(&screen), ["port-b2c"], "and the narrowing held");

    press(&mut screen, KeyEvent::from(KeyCode::Esc));
    assert_eq!(showing_ids(&screen), ["ask-a1b", "port-b2c"]);
}

#[test]
fn find_is_the_only_way_to_narrow_by_name_now_that_a_is_not_a_token() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    // `a:` is plain text in a task now, not a filter.
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for key in word("a:port") {
        press(&mut screen, KeyEvent::from(key));
    }
    assert_eq!(
        screen.banded().map(|line| line.label()),
        Some("TASK"),
        "a line beginning `a:` starts an agent rather than narrowing"
    );
    press(&mut screen, KeyEvent::from(KeyCode::Esc));

    // On a find line it is the name to look for, colon included.
    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    for key in word("a:port") {
        press(&mut screen, KeyEvent::from(key));
    }
    assert!(
        showing_ids(&screen).is_empty(),
        "nothing is called `a:port`"
    );
}

#[test]
fn find_reads_the_state_tokens_the_task_line_read_once() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_fleet_to_search());
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('/')));
    for key in word("s:waiting") {
        press(&mut screen, KeyEvent::from(key));
    }
    assert_eq!(
        showing_ids(&screen),
        ["ask-a1b"],
        "a line of nothing but filter tokens narrows by state, which is \
             the one thing `/` does that a name does not"
    );
}

#[test]
fn keys_gg_reaches_the_top_of_the_list_and_g_alone_waits_for_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        stopped_on_a_question("ask-a1b"),
        finished_saying("done-b2c", "the answer"),
    ]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let on = |screen: &Screen| screen.list.cursor();

    // G goes to the foot in one press.
    press(&mut screen, KeyEvent::from(KeyCode::Char('G')));
    assert_eq!(
        screen.list.selected().map(|view| view.id()),
        Some("done-b2c"),
        "G lands on the last agent there is"
    );

    // One g moves nothing and waits for the second.
    let foot = on(&screen);
    press(&mut screen, KeyEvent::from(KeyCode::Char('g')));
    assert_eq!(on(&screen), foot, "one g moves nothing");
    assert!(screen.going, "and waits for the g that would finish it");

    press(&mut screen, KeyEvent::from(KeyCode::Char('g')));
    assert_eq!(on(&screen), 0, "the second g goes to the top");
    assert!(!screen.going, "with nothing left waiting");
}

#[test]
fn keys_a_g_waiting_for_its_second_is_cancelled_by_any_other_key() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        stopped_on_a_question("ask-a1b"),
        finished_saying("done-b2c", "the answer"),
    ]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('G')));
    press(&mut screen, KeyEvent::from(KeyCode::Char('g')));
    assert!(screen.going);

    // Any other key cancels the pending g and does its own job in full.
    press(&mut screen, KeyEvent::from(KeyCode::Char('k')));
    assert!(!screen.going, "the g is gone");
    assert!(
        screen.list.selected().map(|view| view.id()) != Some("done-b2c"),
        "and k walked, rather than being eaten by the g in front of it"
    );

    // So the next g is a fresh first press.
    press(&mut screen, KeyEvent::from(KeyCode::Char('g')));
    assert!(screen.going);
    let before = screen.list.cursor();
    press(&mut screen, KeyEvent::from(KeyCode::Char('j')));
    assert_ne!(screen.list.cursor(), before, "j walked");
    assert!(!screen.going);
}

#[test]
fn keys_alt_arrows_bring_back_the_lines_sent_and_the_plain_ones_keep_the_card_moving() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the answer"),
        finished_saying("done-b2c", "another"),
    ]);
    screen.sent.remember_line(&Asking::Reply, "carry on");
    screen.sent.remember_line(&Asking::Reply, "ship it");
    let press =
        |screen: &mut Screen, key: KeyEvent| screen.act(key, root.path(), &config, None).unwrap();
    let line = |screen: &Screen| screen.answering().expect("the card's line").text.clone();

    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    let opened_on = screen.card.as_ref().map(|card| card.id.clone());
    assert!(opened_on.is_some(), "space opens the card");

    // alt+up walks back through the replies sent, newest first; alt+down
    // returns to the empty draft.
    press(&mut screen, KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(line(&screen), "ship it");
    press(&mut screen, KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(line(&screen), "carry on");
    press(&mut screen, KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
    assert_eq!(line(&screen), "ship it");
    press(&mut screen, KeyEvent::new(KeyCode::Down, KeyModifiers::ALT));
    assert_eq!(line(&screen), "", "the draft was the empty line");
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.clone()),
        opened_on,
        "and none of that moved the card"
    );

    // The plain arrows still move the card to the next agent.
    press(&mut screen, KeyEvent::from(KeyCode::Down));
    if screen.card.as_ref().map(|card| card.id.clone()) == opened_on {
        press(&mut screen, KeyEvent::from(KeyCode::Up));
    }
    assert_ne!(
        screen.card.as_ref().map(|card| card.id.clone()),
        opened_on,
        "a plain arrow moved the card"
    );
    assert_eq!(line(&screen), "", "and brought nothing back");
}

#[test]
fn keys_a_task_line_walks_the_lines_sent_on_the_plain_arrows() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    screen
        .sent
        .remember_line(&Asking::Task, "port the importer");
    screen.sent.remember_line(&Asking::Reply, "not a task");
    let press =
        |screen: &mut Screen, key: KeyEvent| screen.act(key, root.path(), &config, None).unwrap();
    let line = |screen: &Screen| match &screen.mode {
        Mode::Typing(composer) => composer.text.clone(),
        _ => panic!("no line is open"),
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    press(&mut screen, KeyEvent::from(KeyCode::Char('h')));
    press(&mut screen, KeyEvent::from(KeyCode::Char('i')));
    assert_eq!(line(&screen), "hi");

    // A task line has no card to move, so plain up walks back through the
    // tasks, not the replies.
    press(&mut screen, KeyEvent::from(KeyCode::Up));
    assert_eq!(line(&screen), "port the importer");
    press(&mut screen, KeyEvent::from(KeyCode::Up));
    assert_eq!(
        line(&screen),
        "port the importer",
        "the oldest is where it stops"
    );
    press(&mut screen, KeyEvent::from(KeyCode::Down));
    assert_eq!(line(&screen), "hi", "and down past the newest is the draft");

    // The alt arrows work here too, so one chord works on every line.
    press(&mut screen, KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(line(&screen), "port the importer");
}

#[test]
fn replied_keeps_the_line_it_sent_and_not_one_refused() {
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    let mut sent = Composer::new(Asking::Reply);
    sent.insert("ship it");
    screen.replied(Ok(Replied::Yes("sent to done-a1b".to_string())), sent);
    assert_eq!(screen.sent.lines_for(&Asking::Reply), ["ship it"]);

    // A refused line is still being written, so it is not kept.
    let mut refused = Composer::new(Asking::Reply);
    refused.insert("not yet");
    screen.replied(Ok(Replied::No("busy".to_string())), refused);
    assert_eq!(screen.sent.lines_for(&Asking::Reply), ["ship it"]);
    assert!(
        matches!(screen.notice, Some(Notice::Refused(_))),
        "and the reason it would not take it is said as a refusal"
    );
}

#[test]
fn remembered_keeps_the_lines_sent_for_the_next_view() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("view.json");
    let mut screen = Screen {
        remembering: Some(path.clone()),
        ..Screen::default()
    };
    screen.remember_line(&Asking::Task, "port the importer");
    screen.remember_line(&Asking::Reply, "ship it");

    // Written immediately, for the next view to read.
    let read = Remembered::read(&path);
    assert_eq!(read.sent.lines_for(&Asking::Task), ["port the importer"]);
    assert_eq!(read.sent.lines_for(&Asking::Reply), ["ship it"]);

    // A file from an older amx without `sent` still reads.
    std::fs::write(&path, b"{\"statusline\": true}\n").unwrap();
    assert_eq!(Remembered::read(&path).sent, act::Backlog::default());
}

#[test]
fn a_pin_in_one_view_lands_in_the_other_on_the_next_reading() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("view.json");
    let fleet = || {
        vec![
            reading("one-a1b", Phase::Working, State::default()),
            reading("two-b2c", Phase::Working, State::default()),
        ]
    };
    let mut left = Screen {
        remembering: Some(path.clone()),
        ..watching(fleet())
    };
    let mut right = Screen {
        remembering: Some(path.clone()),
        ..watching(fleet())
    };

    // The left view pins its cursor row and writes the file.
    left.list.top();
    while left.list.selected().is_none() {
        left.list.down();
    }
    assert!(left.list.hold_or_let_go());
    left.keep(true);
    assert!(left.list.arrangement().has_pinned("one-a1b"));
    assert!(Remembered::read(&path).arrangement.has_pinned("one-a1b"));

    // The right view has not read the file yet.
    assert!(!right.list.arrangement().has_pinned("one-a1b"));
    right.adopt_the_view();
    assert!(
        right.list.arrangement().has_pinned("one-a1b"),
        "a pin one view makes is on the other by its next reading"
    );
    assert!(right.published.has_pinned("one-a1b"));
}

#[test]
fn a_line_sent_before_the_next_reading_does_not_hide_the_other_views_pin() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("view.json");
    let fleet = || {
        vec![
            reading("one-a1b", Phase::Working, State::default()),
            reading("two-b2c", Phase::Working, State::default()),
        ]
    };
    let mut left = Screen {
        remembering: Some(path.clone()),
        ..watching(fleet())
    };
    let mut right = Screen {
        remembering: Some(path.clone()),
        ..watching(fleet())
    };
    right.adopt_the_view();

    left.list.top();
    while left.list.selected().is_none() {
        left.list.down();
    }
    assert!(left.list.hold_or_let_go());
    left.keep(true);

    // The right view sends a line before its next reading, writing the file.
    right.remember_line(&Asking::Task, "port the importer");
    right.adopt_the_view();
    assert!(
        right.list.arrangement().has_pinned("one-a1b"),
        "the pin is on the right view after its next reading"
    );
}

#[test]
fn a_pin_this_view_makes_does_not_take_the_other_views_off_the_file() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("view.json");
    let fleet = || {
        vec![
            reading("one-a1b", Phase::Working, State::default()),
            reading("two-b2c", Phase::Working, State::default()),
        ]
    };

    // The right view pins its own row and writes.
    let mut right = Screen {
        remembering: Some(path.clone()),
        ..watching(fleet())
    };
    right.list.bottom();
    while right.list.selected().is_none() {
        right.list.up();
    }
    assert!(right.list.hold_or_let_go());
    right.keep(true);

    // The left view opened before that and has not read since, so it does not
    // know the other pin.
    let mut left = Screen {
        remembering: Some(path.clone()),
        ..watching(fleet())
    };
    left.list.top();
    while left.list.selected().is_none() {
        left.list.down();
    }
    assert!(left.list.hold_or_let_go());
    left.keep(true);

    let on_disk = Remembered::read(&path).arrangement;
    assert!(on_disk.has_pinned("one-a1b"), "the left view's own pin");
    assert!(
        on_disk.has_pinned("two-b2c"),
        "and the pin the right view made before it"
    );
}

#[test]
fn keys_v_shows_what_each_row_runs_and_the_next_view_opens_on_the_same_wall() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let path = root.path().join("view.json");
    let mut screen = Screen {
        remembering: Some(path.clone()),
        ..watching(vec![finished_saying("done-a1b", "the answer")])
    };
    let press = |screen: &mut Screen, key: KeyCode| {
        screen
            .act(KeyEvent::from(key), root.path(), &config, None)
            .unwrap();
    };

    assert!(!screen.vendor, "the wall opens without the column");
    press(&mut screen, KeyCode::Char('v'));
    assert!(screen.vendor, "and the key puts it up");
    assert!(
        Remembered::read(&path).vendor,
        "written as it is pressed, the way the arrangement is"
    );

    // Pressing it again hides the column, and that is saved too.
    press(&mut screen, KeyCode::Char('v'));
    assert!(!screen.vendor);
    assert!(!Remembered::read(&path).vendor);

    // The next view opens with the column as the file says.
    press(&mut screen, KeyCode::Char('v'));
    let next = Screen {
        vendor: Remembered::read(&path).vendor,
        ..Screen::default()
    };
    assert!(
        next.vendor,
        "the next view opens where the last one was left"
    );

    // A file from an older amx without `vendor` reads with the column hidden.
    std::fs::write(&path, b"{\"statusline\": true}\n").unwrap();
    assert!(!Remembered::read(&path).vendor);
}

#[test]
fn keys_l_goes_in_the_way_enter_does_and_leaves_the_card_to_space() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key: KeyCode| {
        screen
            .act(KeyEvent::from(key), root.path(), &config, None)
            .unwrap()
    };
    // `l` is vim's right, and right of a row is its agent, so `l` does what
    // enter does. Here both fail the same way reaching a pane for a record with
    // no state directory.
    let went_in = |key| {
        let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
        let reached = screen.act(KeyEvent::from(key), root.path(), &config, None);
        let said = match reached {
            Ok(_) => String::new(),
            Err(e) => format!("{e:#}"),
        };
        (said, screen.card.is_some())
    };
    assert_eq!(
        went_in(KeyCode::Char('l')),
        went_in(KeyCode::Enter),
        "the same key, spelt twice"
    );
    assert!(!went_in(KeyCode::Char('l')).1, "and it opens no card");

    // Only space opens the card.
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    press(&mut screen, KeyCode::Char(' '));
    assert!(screen.card.is_some(), "space opens the card");

    // With a card open, `l` is text on the card's line.
    press(&mut screen, KeyCode::Char('l'));
    assert!(screen.card.is_some(), "and leaves it open");
    assert_eq!(screen.answering().expect("the card's line").text, "l");

    // Esc closes the line and the card together.
    press(&mut screen, KeyCode::Esc);
    assert!(screen.card.is_none(), "esc closes the card");
    assert!(matches!(screen.mode, Mode::List), "and the line with it");

    press(&mut screen, KeyCode::Esc);
    assert!(screen.card.is_none(), "and pressed again leaves it closed");
}

#[test]
fn card_pages_half_a_page_under_ctrl_d_and_ctrl_u() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    // The page size the last frame gave the body.
    screen.scroll.page.set(10);

    press(&mut screen, ctrl('d'));
    assert_eq!(screen.scroll.away.get(), 5, "half a page away from the top");
    press(&mut screen, ctrl('f'));
    assert_eq!(screen.scroll.away.get(), 15, "and a whole one after it");
    press(&mut screen, ctrl('u'));
    assert_eq!(screen.scroll.away.get(), 10, "half a page back");
    press(&mut screen, ctrl('b'));
    assert_eq!(screen.scroll.away.get(), 0, "and a whole one home");
}

#[test]
fn card_pages_under_ctrl_f_and_ctrl_b_exactly_as_the_page_keys() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    // A recorded answer reads down from its top: ctrl+f leaves the edge like
    // pgdn, ctrl+b comes back like pgup.
    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    press(&mut screen, ctrl('f'));
    assert!(
        screen.scroll.away.get() > 0,
        "ctrl+f paged away from the top"
    );
    press(&mut screen, ctrl('b'));
    assert_eq!(screen.scroll.away.get(), 0, "and ctrl+b is the page back");

    // A patch also reads down from its top, so the keys go the same way.
    screen.look = Look::Changes;
    screen.card = Some(Card {
        id: "done-a1b".to_string(),
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        walked: false,
        kind: None,
        body: Body::patch("+ line"),
        changes: true,
        answer: false,
        listening: true,
        queued: Vec::new(),
    });
    press(&mut screen, ctrl('f'));
    assert!(
        screen.scroll.away.get() > 0,
        "ctrl+f paged down into the patch"
    );
    press(&mut screen, ctrl('b'));
    assert_eq!(screen.scroll.away.get(), 0, "and back to the top");
}

#[test]
fn card_line_leaves_ctrl_f_and_ctrl_b_to_the_card_like_the_page_keys() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    assert!(screen.answering().is_some(), "the line is up");

    // Chords are not text, so they page the card under the line. A question
    // reads up from the bottom, so ctrl+b leaves the edge.
    press(&mut screen, ctrl('b'));
    assert_eq!(screen.scroll.away.get(), 1, "ctrl+b paged the card");
    press(&mut screen, ctrl('f'));
    assert_eq!(screen.scroll.away.get(), 0, "and ctrl+f paged it home");
    assert_eq!(screen.answering().expect("still typing").text, "");
}

#[test]
fn card_holding_a_patch_pages_the_other_way_round() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "an answer")]);
    // A patch card, as `d` leaves one.
    screen.look = Look::Changes;
    screen.card = Some(Card {
        id: "done-a1b".to_string(),
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        walked: false,
        kind: None,
        body: Body::patch("+ line"),
        changes: true,
        answer: false,
        listening: true,
        queued: Vec::new(),
    });
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };

    // A patch reads down from its top: pgdn leaves the edge.
    press(&mut screen, KeyCode::PageDown);
    assert!(screen.scroll.away.get() > 0, "paged down into the patch");
    press(&mut screen, KeyCode::PageUp);
    assert_eq!(screen.scroll.away.get(), 0, "and back to the top");
}

/// A patch of two files, one hunk each.
const TWO_HUNKS: &str = "\
diff --git a/src/foo.rs b/src/foo.rs
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,2 +1,2 @@
-    let old = 1;
+    let new = 2;
diff --git a/src/bar.rs b/src/bar.rs
--- a/src/bar.rs
+++ b/src/bar.rs
@@ -8,1 +8,2 @@
 done
+and more";

#[test]
fn keys_ctrl_n_and_ctrl_p_step_the_hunks_of_a_changes_card_alone() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "an answer")]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    // On a card that is not a patch the keys have no hunks to step through.
    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.at_hunk(), None, "no patch to step through");

    // A patch card, as `d` leaves one.
    screen.look = Look::Changes;
    screen.card = Some(Card {
        id: "done-a1b".to_string(),
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        walked: false,
        kind: None,
        body: Body::patch(TWO_HUNKS),
        changes: true,
        answer: false,
        listening: true,
        queued: Vec::new(),
    });
    screen.scroll.open_at(0);

    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.at_hunk(), Some(0), "the first from none");
    assert_eq!(screen.scroll.away.get(), 1, "its header row, at the top");
    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.at_hunk(), Some(1), "and the next after it");
    assert_eq!(screen.scroll.away.get(), 5);
    press(&mut screen, ctrl('p'));
    assert_eq!(screen.scroll.at_hunk(), Some(0), "and the one before it");

    // The line takes neither chord as text. Its words stay with the hunk they
    // were typed under.
    press(&mut screen, KeyEvent::from(KeyCode::Char('x')));
    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.at_hunk(), Some(1), "stepped from under it");
    assert_eq!(screen.scroll.remarked(Some(0)), "x", "kept on the hunk");

    // The hunk under the cursor gives the file and line a comment names.
    let (at, hunk) = screen.at_hunk().expect("a hunk under the cursor");
    assert_eq!((at, hunk.path.as_str(), hunk.line), (1, "src/bar.rs", 8));
}

/// A screen with the card's line open under a patch of [`TWO_HUNKS`].
fn reviewing() -> Screen {
    let mut screen = watching(vec![finished_saying("done-a1b", "an answer")]);
    screen.look = Look::Changes;
    screen.card = Some(Card {
        id: "done-a1b".to_string(),
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        walked: false,
        kind: None,
        body: Body::patch(TWO_HUNKS),
        changes: true,
        answer: false,
        listening: true,
        queued: Vec::new(),
    });
    screen.scroll.open_at(0);
    screen.mode = Mode::Typing(Composer::new(Asking::Reply));
    screen
}

#[test]
fn card_line_carries_its_words_to_the_hunk_it_steps_off() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = reviewing();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let types = |screen: &mut Screen, text: &str| {
        for key in word(text) {
            press(screen, KeyEvent::from(key));
        }
    };

    // Words written at the top of the patch are the review's opening, not a
    // note on a hunk.
    types(&mut screen, "looks close");
    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.remarked(None), "looks close");
    assert!(screen.scroll.noted().is_empty(), "an opening is no note");
    assert_eq!(
        screen.answering().expect("the line").text,
        "",
        "and the hunk stepped to has nothing on it yet"
    );

    types(&mut screen, "why this row?");
    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.remarked(Some(0)), "why this row?");
    assert_eq!(screen.scroll.noted(), vec![0], "one note behind the line");

    // Stepping back puts the kept words on the line, cursor at their end.
    press(&mut screen, ctrl('p'));
    let line = screen.answering().expect("the line");
    assert_eq!(line.text, "why this row?");
    assert_eq!(line.at, "why this row?".chars().count());
    press(&mut screen, ctrl('p'));
    assert_eq!(screen.scroll.at_hunk(), None, "and the top above them");
    assert_eq!(screen.answering().expect("the line").text, "looks close");

    // Emptying the line by hand drops the note kept there.
    for _ in 0.."looks close".len() {
        press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    }
    press(&mut screen, ctrl('n'));
    assert_eq!(screen.scroll.remarked(None), "", "the opening withdrawn");
    assert_eq!(screen.scroll.noted(), vec![0], "the note still standing");
}

#[test]
fn card_line_sends_the_whole_review_as_one_message() {
    let screen = reviewing();
    let card = screen.card.as_ref().expect("the card");
    let hunks = card.body.hunks();

    // With nothing kept, the line alone is the message: the hunk under the
    // cursor, then the words, as `on_hunk` writes it.
    screen.scroll.to_hunk(hunks, true);
    assert_eq!(
        screen.written(&screen.review("why this row?")),
        act::on_hunk(&hunks[0], "why this row?")
    );

    // With a review kept, one message: the opening, then each noted hunk in
    // patch order, with the line's words as the note on the current hunk.
    screen.scroll.remark(None, "looks close");
    screen.scroll.remark(Some(0), "why this row?");
    screen.scroll.to_hunk(hunks, true);
    assert_eq!(screen.noted("and this one is new"), vec![0, 1]);
    assert_eq!(
        screen.written(&screen.review("and this one is new")),
        format!(
            "looks close\n\n{}\n\n{}",
            act::on_hunk(&hunks[0], "why this row?"),
            act::on_hunk(&hunks[1], "and this one is new")
        )
    );

    // An empty line sends what is kept; an opening with no note on any hunk is
    // nothing to send.
    assert_eq!(screen.noted(""), vec![0]);
    screen.scroll.remark(Some(0), "");
    assert!(screen.noted("").is_empty());
}

#[test]
fn esc_puts_the_card_away_and_the_review_with_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = reviewing();
    screen.scroll.remark(None, "looks close");
    screen.scroll.remark(Some(0), "why this row?");

    screen
        .act(KeyEvent::from(KeyCode::Esc), root.path(), &config, None)
        .unwrap();
    assert!(screen.card.is_none(), "the card went");
    assert!(screen.scroll.remarks().is_empty(), "and the review with it");
    assert_eq!(screen.scroll.at_hunk(), None);
}

#[test]
fn card_taken_again_stands_on_its_natural_edge() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };

    press(&mut screen, KeyCode::Char(' '));
    press(&mut screen, KeyCode::PageDown);
    assert!(screen.scroll.away.get() > 0);

    press(&mut screen, KeyCode::Esc);
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(screen.scroll.away.get(), 0, "reopened where it opens");
}

#[test]
fn card_paged_away_stops_following_until_paged_back() {
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    screen.look = Look::Screen;
    screen.follow_the_cursor();
    screen.card.as_mut().expect("a card").body = Body::screen(chrome(), "what she was reading");

    // Paged away, the card holds between rereads so the text does not move
    // under the reader.
    screen.scroll.away.set(3);
    screen.follow_the_cursor();
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("what she was reading".to_string()),
        "held while paged away"
    );

    // Back on the edge, it follows again.
    screen.scroll.away.set(0);
    screen.follow_the_cursor();
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("the answer".to_string()),
        "taken again at the edge"
    );
}

#[test]
fn card_held_still_lets_a_question_through() {
    let mut screen = watching(vec![reading(
        "ask-a1b",
        Phase::Working,
        State {
            state: Phase::Working,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);
    screen.look = Look::Screen;
    screen.follow_the_cursor();
    screen.card.as_mut().expect("a card").body = Body::screen(chrome(), "old capture");
    screen.scroll.away.set(3);

    // A question takes the card back from the hold.
    screen.list.show(vec![stopped_on_a_question("ask-a1b")]);
    screen.follow_the_cursor();
    assert!(
        screen.card.as_ref().is_some_and(Card::asks),
        "the question card arrived"
    );
    assert_eq!(screen.scroll.away.get(), 0);
}

#[test]
fn card_holding_a_patch_yields_to_an_arrow_wherever_it_lands() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    // One agent, so the cursor cannot move; the press still replaces a patch
    // with the agent's own card.
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    screen.look = Look::Changes;
    screen.card = Some(Card {
        id: "done-a1b".to_string(),
        phase: Phase::Done,
        question: None,
        options: Vec::new(),
        walked: false,
        kind: None,
        body: Body::patch("+ line"),
        changes: true,
        answer: false,
        listening: true,
        queued: Vec::new(),
    });
    screen.scroll.away.set(5);

    screen
        .act(KeyEvent::from(KeyCode::Down), root.path(), &config, None)
        .unwrap();
    assert!(
        screen.card.as_ref().is_some_and(|card| !card.changes),
        "the patch went with the press"
    );
    assert_eq!(screen.scroll.away.get(), 0);
}

#[test]
fn card_asking_is_never_held_still() {
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    screen.look = Look::Screen;
    screen.follow_the_cursor();
    screen.card.as_mut().expect("a card").question = Some("an old question".to_string());

    // A question card is always retaken: the record moves while the vendor
    // redraws, and a held card would pair the old question with the new tab.
    screen.scroll.away.set(3);
    screen.follow_the_cursor();
    assert_eq!(
        screen
            .card
            .as_ref()
            .and_then(|card| card.question.as_deref()),
        Some("Which fixture should the port keep?")
    );
}

#[test]
fn card_line_leaves_the_page_keys_to_the_card() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };

    press(&mut screen, KeyCode::Char(' '));
    assert!(screen.answering().is_some(), "the line is up");

    // The line has no use for page keys, so the card under it takes them.
    press(&mut screen, KeyCode::PageUp);
    assert_eq!(screen.scroll.away.get(), 1, "pgup paged the card");
    press(&mut screen, KeyCode::PageDown);
    assert_eq!(screen.scroll.away.get(), 0, "and pgdn paged it home");
    assert_eq!(screen.answering().expect("still typing").text, "");
}

#[test]
fn card_line_takes_the_keys_a_line_reads_and_no_others() {
    let plain = KeyEvent::from;
    let alt = |code| KeyEvent::new(code, KeyModifiers::ALT);
    let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);

    // The line takes characters, the keys that move along it and the keys that
    // end it. A letter counts whether or not shift was held.
    for key in [
        plain(KeyCode::Char('j')),
        plain(KeyCode::Char('q')),
        plain(KeyCode::Char('/')),
        plain(KeyCode::Char('?')),
        plain(KeyCode::Char(' ')),
        shift(KeyCode::Char('A')),
        plain(KeyCode::Enter),
        alt(KeyCode::Enter),
        plain(KeyCode::Esc),
        plain(KeyCode::Tab),
        plain(KeyCode::Backspace),
        alt(KeyCode::Backspace),
        plain(KeyCode::Delete),
        plain(KeyCode::Left),
        plain(KeyCode::Right),
        plain(KeyCode::Home),
        plain(KeyCode::End),
        ctrl('a'),
        ctrl('e'),
        ctrl('w'),
        ctrl('g'),
        ctrl('j'),
        alt(KeyCode::Up),
        alt(KeyCode::Down),
    ] {
        assert!(
            the_lines(&Composer::new(Asking::Reply), key),
            "{key:?} is the line's"
        );
    }

    // Every other key walks the wall, pages the card or acts on an agent, as it
    // does with no line up.
    for key in [
        plain(KeyCode::Up),
        plain(KeyCode::Down),
        shift(KeyCode::Down),
        plain(KeyCode::PageUp),
        plain(KeyCode::PageDown),
        ctrl('f'),
        ctrl('b'),
        ctrl('u'),
        ctrl('d'),
        ctrl('x'),
        ctrl('t'),
        ctrl('s'),
        ctrl('r'),
        alt(KeyCode::Char('1')),
        alt(KeyCode::Char('a')),
    ] {
        assert!(
            !the_lines(&Composer::new(Asking::Reply), key),
            "{key:?} is the list's"
        );
    }

    // Except the two arrows while suggestions are open: they walk the
    // suggestions, as under the task line.
    let mut offering = Composer::new(Asking::Reply);
    offering.suggest = Some(act::Suggest {
        word: 0..1,
        entries: vec![crate::catalog::Entry {
            spelled: "/review".to_string(),
            kind: crate::catalog::Kind::Skill,
            about: String::new(),
        }],
        chosen: 0,
    });
    for key in [plain(KeyCode::Up), plain(KeyCode::Down)] {
        assert!(the_lines(&offering, key), "{key:?} walks the words offered");
    }
}

#[test]
fn card_line_offers_the_words_of_the_agents_own_vendor_and_directory() {
    // The card's line suggests from the directory the agent runs in, which the
    // view's own directory knows nothing about, and the arrows walk the
    // suggestions while they are open.
    let root = TempDir::new().unwrap();
    let there = TempDir::new().unwrap();
    for file in ["importer.rs", "imports.rs"] {
        std::fs::write(there.path().join(file), "").unwrap();
    }
    let config = Config::default();
    let mut view = finished_saying("done-a1b", "the answer");
    view.meta.dir = there.path().to_path_buf();
    let mut screen = watching(vec![view, finished_saying("done-b2c", "another")]);
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let offered = |screen: &Screen| -> Vec<String> {
        screen
            .answering()
            .and_then(|line| line.suggest.as_ref())
            .map(|suggest| {
                suggest
                    .entries
                    .iter()
                    .map(|entry| entry.spelled.clone())
                    .collect()
            })
            .unwrap_or_default()
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    for key in word("see @imp") {
        press(&mut screen, KeyEvent::from(key));
    }
    assert_eq!(offered(&screen), ["@importer.rs", "@imports.rs"]);

    // Down walks the suggestions; the cursor stays on its row.
    press(&mut screen, KeyEvent::from(KeyCode::Down));
    assert_eq!(
        screen
            .answering()
            .and_then(|line| line.suggest.as_ref())
            .map(|suggest| suggest.chosen),
        Some(1)
    );
    assert_eq!(
        screen.list.selected().map(|view| view.id().to_string()),
        Some("done-a1b".to_string())
    );
    press(&mut screen, KeyEvent::from(KeyCode::Tab));
    assert_eq!(
        screen.answering().expect("the line").text,
        "see @imports.rs ",
        "and tab takes the word the choice is on"
    );

    // A dial token typed there is part of the message, so nothing is suggested.
    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    for key in word("m:") {
        press(&mut screen, KeyEvent::from(key));
    }
    assert!(offered(&screen).is_empty(), "{:?}", offered(&screen));
}

#[test]
fn card_line_leaves_the_keys_it_has_no_use_for_to_the_list() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the answer"),
        finished_saying("done-b2c", "the other answer"),
    ]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);

    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    assert!(screen.answering().is_some(), "the card's line is up");

    // Each of these does to the list what it does with no line open.
    press(&mut screen, shift(KeyCode::Down));
    assert_eq!(
        ordered(&screen),
        ["done-b2c", "done-a1b"],
        "shift and an arrow moved the agent down its group"
    );

    press(&mut screen, ctrl('t'));
    assert!(
        screen
            .list
            .selected()
            .is_some_and(|view| screen.list.holding(view)),
        "ctrl+t pinned the row the cursor is on"
    );

    press(&mut screen, ctrl('x'));
    assert_eq!(screen.armed(), ["done-a1b"], "ctrl+x armed the forget");

    assert_eq!(
        screen.answering().expect("the line is still up").text,
        "",
        "and none of them was typed into the line"
    );
}

/// A screen with a card open on the cursor's agent and its line empty.
fn carded(root: &Path, config: &Config, views: Vec<View>) -> Screen {
    let mut screen = watching(views);
    screen
        .act(KeyEvent::from(KeyCode::Char(' ')), root, config, None)
        .unwrap();
    assert!(
        screen.answering().is_some_and(|line| line.text.is_empty()),
        "the card opened with its line empty"
    );
    screen
}

#[test]
fn card_line_reads_space_on_an_empty_line_as_the_key_that_closes_the_card() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let space = KeyEvent::from(KeyCode::Char(' '));

    // On an empty line, space is the key that closes the card.
    let mut screen = carded(
        root.path(),
        &config,
        vec![finished_saying("done-a1b", "the answer")],
    );
    press(&mut screen, space);
    assert!(screen.card.is_none(), "space closed the card");
    assert!(matches!(screen.mode, Mode::List), "and its line with it");
    assert_eq!(
        screen.list.selected().map(|view| view.id().to_string()),
        Some("done-a1b".to_string()),
        "with the cursor still on the row the card was opened from"
    );

    // With text on the line, a space is a character.
    let mut screen = carded(
        root.path(),
        &config,
        vec![finished_saying("done-a1b", "the answer")],
    );
    press(&mut screen, KeyEvent::from(KeyCode::Char('o')));
    press(&mut screen, space);
    press(&mut screen, KeyEvent::from(KeyCode::Char('k')));
    assert_eq!(screen.answering().expect("still typing").text, "o k");
    assert!(screen.card.is_some(), "and the card is still up");
}

#[test]
fn card_line_reads_enter_on_an_empty_line_as_the_lists_own_enter() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let enter = KeyEvent::from(KeyCode::Enter);

    // On a row, enter attaches as on the list. The wall here is a hand-made
    // reading with nothing on disk, so the attempt fails with the store not
    // knowing the agent, which only the list's enter would look up.
    let mut screen = carded(root.path(), &config, a_wall());
    let tried = screen.act(enter, root.path(), &config, None);
    assert!(
        tried.is_err_and(|why| format!("{why:#}").contains("ask-a1b")),
        "enter went to bring the agent forward"
    );
    assert!(screen.card.is_some(), "and the card is still up");
    assert_eq!(
        screen.answering().expect("with its line").text,
        "",
        "which took none of the keypress"
    );

    // On the heading, enter shuts the group.
    let mut screen = carded(root.path(), &config, a_wall());
    press(&mut screen, KeyEvent::from(KeyCode::Up));
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    press(&mut screen, enter);
    assert!(
        !showing_ids(&screen).contains(&"ask-a1b".to_string()),
        "the group under the heading is shut: {:?}",
        showing_ids(&screen)
    );

    // On the fold, enter unfolds it.
    let mut screen = watching(a_folding_wall());
    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    for _ in 0..rows::FOLD_AT + 2 {
        press(&mut screen, KeyEvent::from(KeyCode::Down));
    }
    assert!(screen.list.on_fold(), "the cursor is on the fold");
    let held = showing_ids(&screen).len();
    press(&mut screen, enter);
    assert!(
        showing_ids(&screen).len() > held,
        "the fold gave its rows back: {:?}",
        showing_ids(&screen)
    );
    assert!(
        screen.answering().is_some(),
        "with the card's line standing"
    );
}

#[test]
fn card_line_sends_what_is_typed_on_it_rather_than_reading_the_lists_enter() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    let mut screen = carded(root.path(), &config, vec![stopped_on_a_question("ask-a1b")]);
    for code in word("keep it") {
        press(&mut screen, KeyEvent::from(code));
    }
    press(&mut screen, KeyEvent::from(KeyCode::Enter));
    assert!(
        screen.answering().is_none(),
        "the line was spent on the answer rather than on an attach"
    );
    assert!(screen.notice.is_some(), "and what came of it is said");
}

#[test]
fn card_holds_the_agent_it_is_showing_while_the_cursor_is_on_a_heading() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char(' ')));
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-a1b")
    );

    // Up onto the heading. A heading has no card of its own, so the card stays
    // on the agent it was showing instead of closing and taking its line with
    // it.
    press(&mut screen, KeyEvent::from(KeyCode::Up));
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-a1b"),
        "and the card is still the one it was"
    );
    assert!(screen.answering().is_some(), "with its line still up");
}

#[test]
fn card_kept_by_an_arrow_that_takes_no_other_card_stays_where_it_opened() {
    // A finished conversation opens on its end. An arrow that lands on no other
    // agent, onto the heading or past the end of the list, keeps the card where
    // it opened.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    let long: String = (0..40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&path, transcript(&long)).unwrap();
    let mut screen = watching_a_transcript(&path);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let at_its_end = |screen: &mut Screen, when: &str| {
        a_frame(screen);
        let opened = screen.scroll.away.get();
        assert!(opened > 0, "{when}: a body taller than the card");
        assert!(!screen.scroll.paged(), "{when}: on its edge");
        opened
    };

    let opened = at_its_end(&mut screen, "opened");

    // Down, with no row below to land on.
    press(&mut screen, KeyEvent::from(KeyCode::Down));
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("port-a1b")
    );
    assert_eq!(
        at_its_end(&mut screen, "after an arrow at the end of the list"),
        opened
    );

    // Up onto the heading, where the card holds.
    press(&mut screen, KeyEvent::from(KeyCode::Up));
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    assert_eq!(at_its_end(&mut screen, "held on a heading"), opened);
}

#[test]
fn card_is_taken_again_every_pass_while_it_asks() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();

    // Answering one tab of a call advances the record at once, while the vendor
    // is still redrawing its pane.
    screen.list.show(vec![reading(
        "ask-a1b",
        Phase::Waiting,
        State {
            state: Phase::Waiting,
            question: Some("And which docker tag?".to_string()),
            options: vec!["latest".to_string(), "pinned".to_string()],
            kind: Some(Kind::Question),
            since: 1,
            last_event: 2,
            ..State::default()
        },
    )]);

    // The pass between rereads retakes the card, so it shows the new question.
    screen.freshen();
    assert_eq!(
        screen
            .card
            .as_ref()
            .and_then(|card| card.question.as_deref()),
        Some("And which docker tag?")
    );
    assert!(
        screen.answering().is_some(),
        "and the line being typed on the card survives the retake"
    );

    // A diff is taken on request, and a pass does not retake it.
    screen.look = Look::Changes;
    screen.card.as_mut().expect("the card is open").changes = true;
    screen.card.as_mut().expect("the card is open").body = Body::patch("+ a line");
    screen.freshen();
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("+ a line".to_string())
    );
}

#[test]
fn card_of_a_waiting_agent_with_no_question_keeps_the_readings_cadence() {
    // A waiting agent with no recorded question has the pane as its card, and
    // the capture is a tmux fork. It keeps the reading's cadence instead of
    // being retaken on every tick, keystroke and mouse move.
    let mut screen = watching(vec![reading(
        "hush-a1b",
        Phase::Waiting,
        State {
            state: Phase::Waiting,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);
    screen.look = Look::Screen;
    screen.follow_the_cursor();
    screen.card.as_mut().expect("a card").body = Body::screen(chrome(), "what the pane said");

    let walked = paint::walks();
    for _ in 0..4 {
        screen.freshen();
    }
    assert_eq!(paint::walks(), walked, "no card was built between readings");
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("what the pane said".to_string()),
        "the card the reading took is the card still on the screen"
    );

    // The reading retakes it, at the same cadence as the rest of the wall.
    screen.follow_the_cursor();
    assert_eq!(
        paint::walks(),
        walked + 1,
        "and the reading itself takes it again"
    );
}

/// A claude transcript holding one answer.
fn transcript(said: &str) -> String {
    let turn = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": said}]},
    });
    format!("{turn}\n")
}

/// A screen with its card open on an idle agent whose transcript is `path`.
fn watching_a_transcript(path: &Path) -> Screen {
    let mut view = reading(
        "port-a1b",
        Phase::Idle,
        State {
            state: Phase::Idle,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    );
    view.meta.transcript = Some(path.to_path_buf());
    let mut screen = watching(vec![view]);
    screen.look = Look::Screen;
    screen.follow_the_cursor();
    screen
}

#[test]
fn card_line_growing_keeps_the_end_of_the_conversation_in_view() {
    // A finished conversation opens on its end. As the reply line grows, the
    // rows it takes come off the top of the card, so the end of the
    // conversation stays in view.
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    let long: String = (0..40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&path, transcript(&long)).unwrap();
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching_a_transcript(&path);
    let drawn = |screen: &Screen| -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|frame| paint::draw(frame, screen)).unwrap();
        (0..20)
            .map(|row| {
                (0..60)
                    .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    };
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    let opened = drawn(&screen);
    assert!(
        opened.iter().any(|row| row.contains("line 39")),
        "the end of the answer:\n{}",
        opened.join("\n")
    );

    for key in word("one") {
        press(&mut screen, KeyEvent::from(key));
    }
    press(
        &mut screen,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
    );
    for key in word("two") {
        press(&mut screen, KeyEvent::from(key));
    }
    let grown = drawn(&screen);
    assert!(
        grown.iter().any(|row| row == "  two"),
        "the line has grown a row:\n{}",
        grown.join("\n")
    );
    assert!(
        grown.iter().any(|row| row.contains("line 39")),
        "and the end of the answer is still in view over it:\n{}",
        grown.join("\n")
    );
    assert!(!screen.scroll.paged(), "which is still a card nobody paged");
}

#[test]
fn card_stands_until_a_file_it_was_read_from_moves() {
    // Taking a card reads and draws the whole transcript, and between readings
    // the file has usually not changed, so the card on screen stands.
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    std::fs::write(&path, transcript("the first answer")).unwrap();
    let mut screen = watching_a_transcript(&path);
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("the first answer".to_string())
    );

    // The file has not changed, so the card is not retaken.
    screen.card.as_mut().expect("a card").body = Body::said("what she was reading");
    screen.follow_the_cursor();
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("what she was reading".to_string()),
        "the transcript stood, so the card was not read again"
    );

    // The agent answers again and the file changes.
    let both = transcript("the first answer") + &transcript("the second answer");
    std::fs::write(&path, both).unwrap();
    screen.follow_the_cursor();
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("the first answer\n\nthe second answer".to_string()),
        "the card follows the file it was read from"
    );
}

/// What `heard` holds for the transcript at `path`, one debug string per entry.
fn heard_of(heard: &mut Heard, path: &Path, format: Transcript) -> Vec<String> {
    heard
        .of(path, format)
        .iter()
        .map(|said| format!("{said:?}"))
        .collect()
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    let mut file = std::fs::File::options().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

#[test]
fn heard_reads_a_growing_claude_transcript_on_from_where_it_stopped() {
    use std::io::Write;
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    std::fs::write(&path, transcript("first")).unwrap();
    let mut heard = Heard::default();
    let text = |said: &str| format!("{:?}", crate::conversation::Said::Text(said.into()));
    assert_eq!(
        heard_of(&mut heard, &path, Transcript::Claude),
        [text("first")]
    );

    // The first line is changed in place and a second appended; only the
    // appended bytes are read.
    let mut file = std::fs::File::options().write(true).open(&path).unwrap();
    file.write_all(transcript("FIRST").as_bytes()).unwrap();
    file.write_all(transcript("second").as_bytes()).unwrap();
    drop(file);
    assert_eq!(
        heard_of(&mut heard, &path, Transcript::Claude),
        [text("first"), text("second")]
    );

    // A partial line waits for the rest of it.
    let third = transcript("third");
    let (head, tail) = third.split_at(12);
    append(&path, head);
    assert_eq!(heard_of(&mut heard, &path, Transcript::Claude).len(), 2);
    append(&path, tail);
    assert_eq!(
        heard_of(&mut heard, &path, Transcript::Claude),
        [text("first"), text("second"), text("third")]
    );

    // A file renamed over it is read from the start.
    let fresh = held.path().join("fresh.jsonl");
    std::fs::write(&fresh, transcript("again")).unwrap();
    std::fs::rename(&fresh, &path).unwrap();
    assert_eq!(
        heard_of(&mut heard, &path, Transcript::Claude),
        [text("again")]
    );

    // A missing file reads as nothing.
    std::fs::remove_file(&path).unwrap();
    assert!(heard_of(&mut heard, &path, Transcript::Claude).is_empty());
}

#[test]
fn heard_a_line_at_a_time_is_the_whole_file_read_at_once() {
    // Reading on from the last offset is only correct while the reader keeps no
    // state between lines.
    let claude = [
        serde_json::json!({"type": "user", "message": {"content": "port the importer"}}),
        serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Read", "input": {"file_path": "src/importer.rs"}},
        ]}}),
        serde_json::json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "content": "fn main() {}"},
        ]}}),
        serde_json::json!({"type": "queue-operation", "operation": "remove",
                "reason": "absorbed_mid_turn", "content": "and the tests"}),
        serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "text", "text": "Ported."},
        ]}}),
    ]
    .map(|entry| format!("{entry}\n"))
    .concat();
    let codex = include_str!("../../tests/codex/rollouts/turn-steer-abort-kill-resume.jsonl");

    for (format, whole) in [
        (Transcript::Claude, claude.as_str()),
        (Transcript::Codex, codex),
    ] {
        let held = TempDir::new().unwrap();
        let path = held.path().join("session.jsonl");
        std::fs::write(&path, "").unwrap();
        let mut heard = Heard::default();
        for line in whole.split_inclusive('\n') {
            append(&path, line);
            heard.of(&path, format);
        }
        assert_eq!(
            heard.said,
            crate::conversation::read(format, whole),
            "{format:?}"
        );
        assert!(
            heard.said.len() > 3,
            "a fixture with something in it, {format:?}"
        );
    }
}

#[test]
fn heard_reads_a_transcript_its_vendor_writes_over_whole() {
    // opencode's list is rewritten as the turn goes, so a longer file is not
    // the old one with more appended.
    let held = TempDir::new().unwrap();
    let path = held.path().join("messages.jsonl");
    let said = |text: &str| {
        let entry = serde_json::json!({"type": "assistant",
                "content": [{"type": "text", "text": text}]});
        format!("{entry}\n")
    };
    std::fs::write(&path, said("draft")).unwrap();
    let mut heard = Heard::default();
    assert_eq!(heard_of(&mut heard, &path, Transcript::Opencode).len(), 1);

    std::fs::write(&path, said("final") + &said("and more")).unwrap();
    assert_eq!(
        heard_of(&mut heard, &path, Transcript::Opencode),
        [
            format!("{:?}", crate::conversation::Said::Text("final".into())),
            format!("{:?}", crate::conversation::Said::Text("and more".into())),
        ]
    );
}

#[test]
fn card_on_a_finished_conversation_opens_on_its_last_rows_and_reads_as_unpaged() {
    // A conversation card is anchored past its last row and only the paint
    // knows the card's height, so the first frame clamps it to the last page.
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    let long: String = (0..40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&path, transcript(&long)).unwrap();
    let mut screen = watching_a_transcript(&path);

    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal.draw(|frame| paint::draw(frame, &screen)).unwrap();
    let drawn: Vec<String> = (0..14)
        .map(|row| {
            (0..60)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect();
    let card = drawn.join("\n");
    assert!(card.contains("line 39"), "the end of the answer:\n{card}");
    assert!(!card.contains("line 0 "), "and not its top:\n{card}");
    assert!(card.contains("more"), "with the rest a page up:\n{card}");

    // It reads as unpaged, so it keeps following its agent; a paged card would
    // hold still on every reading.
    assert!(!screen.scroll.paged(), "opened where it stands");
    assert_eq!(screen.scroll.away.get(), screen.scroll.opened.get());

    // The page keys still leave it and come back to where it opened.
    let opened = screen.scroll.away.get();
    assert!(opened > 0, "a body taller than the card");
    screen.paged(true);
    assert!(screen.scroll.away.get() < opened, "a page up");
    assert!(screen.scroll.paged(), "and that is a card being read");
    screen.paged(false);
    assert_eq!(screen.scroll.away.get(), opened);
    assert!(!screen.scroll.paged(), "back where it opened");
}

#[test]
fn card_is_taken_again_in_the_palette_a_theme_reread_brought() {
    // The palette changes while no file behind the card has moved, so the theme
    // reread is what marks the card stale.
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    std::fs::write(&path, transcript("the first answer")).unwrap();
    let mut screen = watching_a_transcript(&path);

    screen.card.as_mut().expect("a card").body = Body::said("in the old colours");
    screen.repaint(Theme {
        accent: Color::Rgb(255, 0, 255),
        ..Theme::default()
    });
    screen.follow_the_cursor();
    assert_eq!(
        screen.card.as_ref().map(|card| card.body.says()),
        Some("the first answer".to_string())
    );
}

#[test]
fn card_freshness_is_the_pair_each_file_was_read_at() {
    let held = TempDir::new().unwrap();
    let path = held.path().join("session.jsonl");
    std::fs::write(&path, transcript("the first answer")).unwrap();

    let at = stamped(&path);
    let fresh = Freshness::Files(vec![(path.clone(), at)]);
    assert!(!fresh.moved(), "nothing has written to it");

    // A turn is appended, so the file is longer than when the card read it.
    let both = transcript("the first answer") + &transcript("the second answer");
    std::fs::write(&path, both).unwrap();
    assert!(fresh.moved(), "a turn was appended");

    // The same bytes back with the original mtime. Length and mtime are all a
    // card checks, so the file counts as unchanged.
    std::fs::write(&path, transcript("the first answer")).unwrap();
    let (_, written) = at.expect("the file was there to be read");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(written))
        .unwrap();
    assert!(!fresh.moved(), "the same bytes at the same moment");

    // Nothing on disk says a vendor redrew its pane, so a captured card always
    // counts as moved.
    assert!(Freshness::Pane.moved());
}

#[test]
fn card_keeps_an_answer_line_up_for_as_long_as_a_question_is_pending() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };

    // The card opens with its line, and pressing a choice spends it. No agent
    // is behind this record, so the reply fails and the mode is left as a sent
    // answer leaves it.
    press(&mut screen, KeyCode::Char(' '));
    press(&mut screen, KeyCode::Char('1'));
    assert!(screen.answering().is_none(), "the line was spent");

    // The call moves to its next tab while the card is open, and the pass
    // between rereads brings back an empty line for the new question.
    screen.list.show(vec![reading(
        "ask-a1b",
        Phase::Waiting,
        State {
            state: Phase::Waiting,
            question: Some("And which docker tag?".to_string()),
            options: vec!["latest".to_string(), "pinned".to_string()],
            kind: Some(Kind::Question),
            since: 1,
            last_event: 2,
            ..State::default()
        },
    )]);
    screen.freshen();
    assert_eq!(
        screen
            .card
            .as_ref()
            .and_then(|card| card.question.as_deref()),
        Some("And which docker tag?")
    );
    assert!(
        screen.answering().is_some_and(|line| line.text.is_empty()),
        "the answer line is there whenever a question is pending, empty"
    );

    // When the last answer resolves the call, the line stays: the agent is
    // working and the card still takes a message.
    press(&mut screen, KeyCode::Char('2'));
    screen.list.show(vec![reading(
        "ask-a1b",
        Phase::Working,
        State {
            state: Phase::Working,
            since: 1,
            last_event: 3,
            ..State::default()
        },
    )]);
    screen.freshen();
    assert!(
        screen.card.as_ref().is_some_and(|card| !card.asks()),
        "the card is not asking anything now"
    );
    assert!(
        screen.answering().is_some_and(|line| line.text.is_empty()),
        "and the line is still there, empty, for a message instead"
    );
}

#[test]
fn card_on_an_agent_that_is_asking_nothing_still_opens_its_line() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![reading(
        "busy-a1b",
        Phase::Working,
        State {
            state: Phase::Working,
            summary: Some("Running Bash".to_string()),
            ..State::default()
        },
    )]);

    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    assert!(
        screen.card.is_some(),
        "a closer look is still a closer look"
    );
    assert!(
        screen.answering().is_some(),
        "with the line at its foot: an agent at work can be told something"
    );
    assert!(
        screen.banded().is_none(),
        "and that line is the card's rather than a band of its own under it"
    );
}

#[test]
fn card_is_where_a_reply_is_typed_whatever_the_agent_is_doing() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![stopped_on_a_question("ask-a1b")]);

    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    assert!(
        screen.answering().is_some(),
        "the card the choices are on opens with the line to answer on"
    );

    // An agent between turns takes a message on the same line; there is no
    // separate band for it.
    let mut screen = watching(vec![reading(
        "fix-login-b2c",
        Phase::Idle,
        State {
            state: Phase::Idle,
            ..State::default()
        },
    )]);
    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("fix-login-b2c"),
        "the key opens the card on the agent under the cursor"
    );
    assert!(screen.answering().is_some(), "with the line at its foot");
    assert!(screen.banded().is_none(), "and no band under the wall");
}

#[test]
fn acts_the_status_line_is_offered_once_and_never_written_anywhere() {
    let state = TempDir::new().unwrap();
    let root = state.path().join("agents");
    std::fs::create_dir_all(&root).unwrap();

    let kept = crate::paths::view_file(&root).expect("somewhere to keep it");

    let offer = offer_the_statusline(&kept).expect("the first quit offers");
    assert!(
        offer.contains("set -g status-right '#(amx statusline) | %H:%M'"),
        "it is pasted, so it is the whole line tmux takes: {offer}"
    );
    assert!(
        offer.lines().count() > 1,
        "and the line has a row to itself, to be copied off: {offer}"
    );

    assert_eq!(
        offer_the_statusline(&kept),
        None,
        "an offer that comes back every time is an advertisement"
    );
    assert_eq!(
        kept.parent(),
        Some(state.path()),
        "and it is remembered beside the agents rather than among them, \
             so the next view knows"
    );
    assert!(kept.exists());
}

/// The ids of the agents the list shows, in order.
fn ordered(screen: &Screen) -> Vec<String> {
    screen
        .list
        .items()
        .iter()
        .filter_map(|item| screen.list.agent(*item))
        .map(|view| view.id().to_string())
        .collect()
}

#[test]
fn acts_shift_with_an_arrow_moves_the_agent_rather_than_the_cursor() {
    let root = TempDir::new().unwrap();
    let working = |id: &str| {
        reading(
            id,
            Phase::Working,
            State {
                state: Phase::Working,
                since: 1,
                last_event: 1,
                ..State::default()
            },
        )
    };
    let mut screen = watching(vec![
        working("busy-a1b"),
        working("busy-b2c"),
        working("busy-c3d"),
    ]);
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen
            .act(key, root.path(), &Config::default(), None)
            .unwrap();
    };
    let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);

    assert_eq!(ordered(&screen), ["busy-a1b", "busy-b2c", "busy-c3d"]);
    press(&mut screen, shift(KeyCode::Down));
    assert_eq!(ordered(&screen), ["busy-b2c", "busy-a1b", "busy-c3d"]);
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "busy-a1b",
        "the cursor goes with the agent rather than off it"
    );

    press(&mut screen, shift(KeyCode::Up));
    assert_eq!(
        ordered(&screen),
        ["busy-a1b", "busy-b2c", "busy-c3d"],
        "and back where it was"
    );

    // With one agent pinned, a move reaches the first row of what is left of
    // its group, which has a heading above it and nothing to move into.
    press(&mut screen, KeyEvent::from(KeyCode::Down));
    press(&mut screen, ctrl('t'));
    assert_eq!(ordered(&screen), ["busy-b2c", "busy-a1b", "busy-c3d"]);
    for _ in 0..2 {
        press(&mut screen, KeyEvent::from(KeyCode::Down));
    }
    press(&mut screen, shift(KeyCode::Up));
    assert_eq!(ordered(&screen), ["busy-b2c", "busy-a1b", "busy-c3d"]);
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "busy-a1b",
        "a move that was refused is not a cursor that moved"
    );

    for _ in 0..2 {
        press(&mut screen, KeyEvent::from(KeyCode::Up));
    }
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "busy-b2c",
        "and the arrow without the chord walks the list as it always did"
    );
}

#[test]
fn acts_what_the_view_keeps_opens_the_next_one_and_leaves_the_file_alone() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);

    // A file from an older amx that knows only the statusline offer.
    let kept = root.path().join("view.json");
    std::fs::write(&kept, b"{\"statusline\": true}\n").unwrap();

    let at = |screen: &str, id: &str| {
        screen
            .lines()
            .position(|line| line.contains(id))
            .unwrap_or_else(|| panic!("no row for {id} in:\n{screen}"))
    };
    let (_, screen) = drawn_about(
        root.path(),
        &Scope::default(),
        vec![
            Typed::Key(KeyEvent::from(KeyCode::Down)),
            Typed::Key(ctrl('t')),
            Typed::Key(ctrl('s')),
            Typed::Key(KeyEvent::from(KeyCode::Char('q'))),
        ],
        Some(&kept),
    );
    assert!(
        at(&screen, "second-b2c") < at(&screen, "first-a1b"),
        "the one being held is over the groups:\n{screen}"
    );

    let (_, again) = drawn_about(
        root.path(),
        &Scope::default(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
        Some(&kept),
    );
    assert!(
        at(&again, "second-b2c") < at(&again, "first-a1b"),
        "and the next view opens on it, having been told nothing else:\n{again}"
    );
    assert!(
        again.contains("/srv/app"),
        "gathered the way the last one was left, too:\n{again}"
    );

    let written = std::fs::read_to_string(&kept).unwrap();
    assert!(
        written.contains("\"statusline\": true"),
        "what the file already said is still in it: {written}"
    );
}

#[test]
fn acts_ctrl_x_arms_a_finished_row_and_the_press_after_it_forgets_it() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);
    let left = || crate::store::list(root.path()).unwrap().len();

    // The view opens on the newest agent, where the key is read.
    let (_, armed) = pressing(root.path(), vec![ctrl('x')]);
    assert!(armed.contains("ctrl+x again forgets"), "{armed}");
    assert!(
        armed.contains("wrote the tests"),
        "and the row nobody armed is saying what it always said: {armed}"
    );
    assert_eq!(left(), 2, "and one press forgets nothing");

    // A second press on another row arms that row instead of finishing the
    // first.
    let (_, moved) = pressing(
        root.path(),
        vec![ctrl('x'), KeyEvent::from(KeyCode::Down), ctrl('x')],
    );
    assert!(moved.contains("ctrl+x again forgets"), "{moved}");
    assert_eq!(left(), 2, "a press on another row arms that one instead");

    let (_, gone) = pressing(root.path(), vec![ctrl('x'), ctrl('x')]);
    assert!(gone.contains("first-a1b forgotten"), "{gone}");
    assert_eq!(left(), 1);
}

/// Records an agent whose state still reads live, behind a socket with no
/// server; the test hands the wall its phase.
fn idle(root: &Path, id: &str) {
    let agent = Agent::create(
        root,
        &Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: None,
            model: None,
            effort: None,
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name("amx-not-a-server".to_string()),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: now(),
        },
    )
    .unwrap();
    let state = State {
        state: Phase::Idle,
        since: now(),
        last_event: now(),
        ..State::default()
    };
    std::fs::write(
        agent.dir().join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
}

#[test]
fn acts_ctrl_x_on_a_live_row_stops_it_and_arms_it_and_the_press_after_forgets_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "quiet-a1b");
    let mut screen = watching(vec![reading(
        "quiet-a1b",
        Phase::Idle,
        State {
            state: Phase::Idle,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);

    // The first press stops a live agent and arms the row in the same move.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    let agent = Agent::open(root.path(), "quiet-a1b").unwrap();
    assert_eq!(agent.state().unwrap().state, Phase::Stopped);
    assert_eq!(
        screen.armed(),
        ["quiet-a1b".to_string()],
        "armed by the press that stopped it, not by a press of its own"
    );
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        1,
        "and stopping is not forgetting"
    );

    // The second press inside the window forgets it, even while the list still
    // holds the reading from before the stop.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    let Some(Notice::Advice(said)) = &screen.notice else {
        panic!("the second press said nothing")
    };
    assert!(said.contains("forgotten"), "{said}");
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "two presses on a live row, not three"
    );
}

#[test]
fn acts_ctrl_x_on_a_heading_arms_the_finished_under_it_and_the_press_after_forgets_them() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);
    let left = || crate::store::list(root.path()).unwrap().len();

    // Up from the opening row is the heading. The first press arms every
    // finished row under it, each saying so in its summary column, and the
    // footer asks nothing. At 50 columns the warning is cut like any summary;
    // the full sentence is tested in the paint.
    let (_, armed) = pressing(root.path(), vec![KeyEvent::from(KeyCode::Up), ctrl('x')]);
    assert_eq!(armed.matches("ctrl+x again stops").count(), 2, "{armed}");
    assert!(!armed.contains("forget 2 finished"), "{armed}");
    assert_eq!(left(), 2, "and arming is all that has happened");

    // Any other key forgets nothing.
    let (_, kept) = pressing(
        root.path(),
        vec![
            KeyEvent::from(KeyCode::Up),
            ctrl('x'),
            KeyEvent::from(KeyCode::Down),
        ],
    );
    assert_eq!(left(), 2);
    assert!(!kept.contains("forgot"), "{kept}");

    // The second press on the heading, inside the window, forgets them all.
    let (_, swept) = pressing(
        root.path(),
        vec![KeyEvent::from(KeyCode::Up), ctrl('x'), ctrl('x')],
    );
    assert!(swept.contains("forgot 2"), "{swept}");
    assert_eq!(left(), 0);
}

#[test]
fn acts_ctrl_x_says_a_tree_it_kept_as_something_that_did_not_happen() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let held = has_landed(root.path(), repo.path(), "fix-login-a1b");
    std::fs::write(
        held.meta.worktree.as_deref().unwrap().join("login.rs"),
        "fn login() {}\n",
    )
    .unwrap();
    let mut screen = watching(vec![held]);

    // Two presses, as for any forget. The second finds uncommitted work, keeps
    // the tree and its record, and says so where the forget would have; only
    // the colour tells the two outcomes apart.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("a tree kept back was said as though the row had gone")
    };
    assert!(said.contains("keeping fix-login-a1b"), "{said}");
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["fix-login-a1b".to_string()],
        "and what it says is what happened"
    );
}

#[test]
fn acts_ctrl_x_on_a_heading_says_a_tree_it_kept_the_same_way() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let held = has_landed(root.path(), repo.path(), "fix-login-a1b");
    std::fs::write(
        held.meta.worktree.as_deref().unwrap().join("login.rs"),
        "fn login() {}\n",
    )
    .unwrap();
    let gone = has_landed(root.path(), repo.path(), "port-importer-b2c");
    let mut screen = watching(vec![held, gone]);
    screen.list.up();

    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("a group that left a tree standing was said as a clean sweep")
    };
    assert_eq!(said, "forgot 1 · kept 1 holding work no commit has");
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["fix-login-a1b".to_string()]
    );
}

#[test]
fn acts_ctrl_x_on_a_heading_arms_a_live_row_without_stopping_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "quiet-a1b");
    let mut screen = watching(vec![reading(
        "quiet-a1b",
        Phase::Idle,
        State {
            state: Phase::Idle,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);
    screen.list.up();

    // The first press on the group only arms its rows: the live agent keeps
    // running and nothing is refused.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    let agent = Agent::open(root.path(), "quiet-a1b").unwrap();
    assert_eq!(
        agent.state().unwrap().state,
        Phase::Idle,
        "a heading's first press costs no pane"
    );
    assert_eq!(screen.armed(), ["quiet-a1b".to_string()]);
    assert!(
        screen.notice.is_none(),
        "the rows carry the warning, and no group is refused any more"
    );
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        1,
        "and the first press forgets nothing"
    );

    // The second press on the heading stops it and forgets it.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert!(crate::store::list(root.path()).unwrap().is_empty());
}

/// A reading of an agent in `phase`, settled there since the first second.
fn in_phase(id: &str, phase: Phase) -> View {
    reading(
        id,
        phase,
        State {
            state: phase,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )
}

#[test]
fn ctrl_x_a_row_press_under_a_heading_arm_stops_before_forgetting() {
    // The heading armed two live agents. A press on one of their rows is that
    // row's own first press: it stops the agent and arms the row, and the
    // record stays.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "busy-a1b");
    idle(root.path(), "busy-b2c");
    let mut screen = watching(vec![
        in_phase("busy-a1b", Phase::Working),
        in_phase("busy-b2c", Phase::Working),
    ]);
    screen.list.up();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(screen.armed().len(), 2, "the heading armed both");

    screen.list.down();
    assert_eq!(screen.list.selected().unwrap().id(), "busy-a1b");
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(
        Agent::open(root.path(), "busy-a1b")
            .unwrap()
            .state()
            .unwrap()
            .state,
        Phase::Stopped,
        "stopped"
    );
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        2,
        "and nothing forgotten under a running pane"
    );
    assert_eq!(screen.armed(), ["busy-a1b".to_string()], "armed on its own");
}

#[test]
fn ctrl_x_a_first_press_on_another_heading_only_arms() {
    // Working was armed and one of its agents ended into Completed. Working is
    // still on the wall, so a press on Completed is Completed's own first press
    // and leaves the agent still under Working alone.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    for id in ["busy-a1b", "busy-b2c", "done-c3d"] {
        idle(root.path(), id);
    }
    let mut screen = watching(vec![
        in_phase("busy-a1b", Phase::Working),
        in_phase("busy-b2c", Phase::Working),
        in_phase("done-c3d", Phase::Done),
    ]);
    screen.list.top();
    assert!(screen.list.on_heading());
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(screen.armed().len(), 2);

    screen.showing(vec![
        in_phase("busy-a1b", Phase::Working),
        in_phase("busy-b2c", Phase::Done),
        in_phase("done-c3d", Phase::Done),
    ]);
    screen.keep_the_sweep();
    screen.list.bottom();
    while !screen.list.on_heading() {
        screen.list.up();
    }
    assert_eq!(
        screen.list.heading(),
        Some(rows::Under::Group(rows::Group::Completed))
    );
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();

    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        3,
        "a first press forgets nothing"
    );
    assert_eq!(
        Agent::open(root.path(), "busy-a1b")
            .unwrap()
            .state()
            .unwrap()
            .state,
        Phase::Idle,
        "and stops nothing"
    );
    let mut armed = screen.armed().to_vec();
    armed.sort();
    assert_eq!(armed, ["busy-b2c", "done-c3d"], "it armed its own rows");
}

#[test]
fn ctrl_x_a_reread_moves_no_pointer_onto_another_agent() {
    let mut screen = watching(vec![
        in_phase("ask-a1b", Phase::Waiting),
        in_phase("busy-b2c", Phase::Working),
    ]);
    let at = screen
        .list
        .items()
        .iter()
        .position(|item| {
            screen
                .list
                .agent(*item)
                .is_some_and(|view| view.id() == "ask-a1b")
        })
        .unwrap();
    screen.hover = Some(at);

    // The same wall again: the pointer stays.
    screen.showing(vec![
        in_phase("ask-a1b", Phase::Waiting),
        in_phase("busy-b2c", Phase::Working),
    ]);
    assert_eq!(screen.hover, Some(at));

    // The agent under the pointer changes group and another takes its line.
    screen.showing(vec![
        in_phase("ask-a1b", Phase::Done),
        in_phase("busy-b2c", Phase::Waiting),
    ]);
    assert_eq!(screen.hover, None, "the pointer is on nothing now");
}

#[test]
fn ctrl_x_a_vanished_armed_row_forgets_nothing() {
    // An armed finished row is forgotten in another shell. The row that moves
    // into its line must not be forgotten by the press meant for the first.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "done-a1b");
    idle(root.path(), "done-b2c");
    let mut screen = watching(vec![
        in_phase("done-a1b", Phase::Done),
        in_phase("done-b2c", Phase::Done),
    ]);
    assert_eq!(screen.list.selected().unwrap().id(), "done-a1b");
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(screen.armed(), ["done-a1b".to_string()]);

    screen.showing(vec![in_phase("done-b2c", Phase::Done)]);
    assert!(screen.armed().is_empty(), "the arm went with its row");
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert!(
        crate::store::list(root.path())
            .unwrap()
            .contains(&"done-b2c".to_string()),
        "the row that drifted in is still there"
    );
}

#[test]
fn acts_ctrl_x_on_a_heading_disarms_the_group_when_the_window_is_left_to_lapse() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "quiet-a1b");
    let mut screen = watching(vec![reading(
        "quiet-a1b",
        Phase::Idle,
        State {
            state: Phase::Idle,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);
    screen.list.up();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(screen.armed(), ["quiet-a1b".to_string()]);

    // Move the arm's clock past the window without a press.
    let arm = screen.arm.as_mut().expect("the arm the press left");
    arm.at = arm
        .at
        .checked_sub(ARMED)
        .expect("a machine that has been up longer than the window");
    assert!(
        screen.armed().is_empty(),
        "the rows have nothing left to say about a press that lapsed"
    );

    // So the next press is a first press again: it arms the group and stops
    // nothing.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(
        Agent::open(root.path(), "quiet-a1b")
            .unwrap()
            .state()
            .unwrap()
            .state,
        Phase::Idle,
    );
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        1,
        "and forgets nothing"
    );
    assert_eq!(screen.armed(), ["quiet-a1b".to_string()]);
}

#[test]
fn acts_ctrl_x_sweep_leaves_a_row_it_could_not_stop_unforgotten() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "quiet-a1b");
    // A row whose state cannot be read, so its stop fails before doing
    // anything.
    idle(root.path(), "broken-b2c");
    let broken = Agent::open(root.path(), "broken-b2c").unwrap();
    std::fs::write(broken.dir().join("state.json"), "not a reading").unwrap();

    let live = |id: &str| {
        reading(
            id,
            Phase::Idle,
            State {
                state: Phase::Idle,
                since: 1,
                last_event: 1,
                ..State::default()
            },
        )
    };
    let mut screen = watching(vec![live("quiet-a1b"), live("broken-b2c")]);
    screen.list.up();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(screen.armed().len(), 2, "both rows are armed either way");

    // The row that would not stop is kept, and the rest of the group goes.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["broken-b2c".to_string()],
        "a record amx could not stop is a record it keeps"
    );
    let Some(Notice::Failed(said)) = &screen.notice else {
        panic!("a failed stop is said louder than what went through")
    };
    assert!(said.contains("forgot 1"), "{said}");
    assert!(said.contains("1 would not stop: broken-b2c"), "{said}");
}

#[test]
fn acts_ctrl_x_on_a_project_heading_reaches_rows_in_every_state() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "quiet-a1b");
    finished(root.path(), "done-b2c", "wrote the tests", 60);
    // On the project axis one heading covers live and finished rows at once.
    let mut screen = Screen::default();
    screen.list.turn();
    screen.list.show(vec![
        reading(
            "quiet-a1b",
            Phase::Idle,
            State {
                state: Phase::Idle,
                since: 1,
                last_event: 1,
                ..State::default()
            },
        ),
        finished_saying("done-b2c", "wrote the tests"),
    ]);
    screen.list.up();

    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    let mut armed: Vec<&str> = screen.armed().iter().map(String::as_str).collect();
    armed.sort_unstable();
    assert_eq!(
        armed,
        ["done-b2c", "quiet-a1b"],
        "every row under the heading, whatever its state"
    );
    assert_eq!(
        Agent::open(root.path(), "quiet-a1b")
            .unwrap()
            .state()
            .unwrap()
            .state,
        Phase::Idle,
        "and the live one is still running after the press that armed it"
    );

    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert!(crate::store::list(root.path()).unwrap().is_empty());
}

#[test]
fn acts_ctrl_x_sweep_follows_its_rows_when_another_heading_drifts_into_the_cursor() {
    // An agent that ends while the window is open dissolves its heading, and
    // the live group below moves up into the cursor's index. The second press
    // there must finish the sweep, never stop the group that moved in.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "ask-a1b");
    idle(root.path(), "busy-b2c");
    idle(root.path(), "busy-c3d");
    let working = |id: &str| {
        reading(
            id,
            Phase::Working,
            State {
                state: Phase::Working,
                since: 1,
                last_event: 1,
                ..State::default()
            },
        )
    };
    let mut screen = watching(vec![
        stopped_on_a_question("ask-a1b"),
        working("busy-b2c"),
        working("busy-c3d"),
    ]);
    screen.list.up();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(screen.armed(), ["ask-a1b".to_string()]);

    // The next reading: the armed agent has ended, its heading is gone, and the
    // working heading now sits at the cursor's index.
    screen.list.show(vec![
        reading(
            "ask-a1b",
            Phase::Stopped,
            State {
                state: Phase::Stopped,
                since: 1,
                last_event: 1,
                ..State::default()
            },
        ),
        working("busy-b2c"),
        working("busy-c3d"),
    ]);
    screen.keep_the_sweep();

    // The second press forgets what the first armed and leaves the working
    // group alone.
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert!(
        !crate::store::list(root.path())
            .unwrap()
            .contains(&"ask-a1b".to_string()),
        "the sweep finished on what it armed"
    );
    for id in ["busy-b2c", "busy-c3d"] {
        assert_eq!(
            Agent::open(root.path(), id).unwrap().state().unwrap().state,
            Phase::Idle,
            "{id} was not stopped by a press that was about another group"
        );
    }
}

#[test]
fn acts_ctrl_x_second_press_lands_on_the_heading_now_over_the_armed_rows() {
    // An agent that ends while the window is open moves to completed, so on the
    // state axis the pressed heading can dissolve by the next reading. The
    // second press finds the armed rows under the heading now over them.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "quiet-a1b");
    let mut screen = watching(vec![reading(
        "quiet-a1b",
        Phase::Idle,
        State {
            state: Phase::Idle,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);
    screen.list.up();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();

    // The agent ends on its own: the idle heading is gone and the row is under
    // completed, with the cursor on that heading.
    screen.list.show(vec![reading(
        "quiet-a1b",
        Phase::Stopped,
        State {
            state: Phase::Stopped,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )]);
    screen.list.up();

    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "the second press forgets what the first one armed"
    );
}

/// Runs git with no user configuration and a fixed identity.
fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// A repository with one commit.
fn a_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    std::fs::write(dir.path().join("README.md"), "before\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-m", "first"]);
    dir
}

/// Records an ended agent with a tree cut from `repo` and left in place. Its
/// branch holds exactly what main holds, so git reads the work as landed.
fn has_landed(root: &Path, repo: &Path, id: &str) -> View {
    let tree = crate::worktree::create(repo, id, None).unwrap();
    let meta = Meta {
        role: None,
        parent: None,
        depth: 0,
        id: id.to_string(),
        task: "fix the login bug".to_string(),
        agent: None,
        model: None,
        effort: None,
        dir: repo.to_path_buf(),
        worktree: Some(tree.path.clone()),
        branch: Some(tree.branch.clone()),
        base: Some(tree.base.clone()),
        socket: Socket::Name("amx-not-a-server".to_string()),
        pane: PaneId::new("%404").unwrap(),
        bg: false,
        session: None,
        transcript: None,
        created: 1,
    };
    let state = State {
        state: Phase::Done,
        exit: Some(0),
        since: 1,
        last_event: 1,
        ..State::default()
    };
    let agent = Agent::create(root, &meta).unwrap();
    std::fs::write(
        agent.dir().join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    View::new(
        meta,
        state,
        Verdict {
            phase: Phase::Done,
            evidence: Evidence::Hooks,
            rule: None,
            age: 29,
            worked: 29,
        },
    )
}

/// The `c` key.
fn c() -> KeyEvent {
    KeyEvent::from(KeyCode::Char('c'))
}

/// A stopped agent with no branch and no pull request.
fn was_stopped(id: &str) -> View {
    reading(
        id,
        Phase::Stopped,
        State {
            state: Phase::Stopped,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )
}

#[test]
fn keys_c_says_nothing_has_finished_over_a_wall_that_is_still_at_work() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(vec![at_work("port-a1b"), stopped_on_a_question("ask-b2c")]);
    screen
        .act(c(), root.path(), &Config::default(), None)
        .unwrap();
    let Some(Notice::Advice(said)) = &screen.notice else {
        panic!("nothing said about a wall with nothing on it to clear")
    };
    assert_eq!(said, "nothing has finished");
    assert!(
        screen.arm.is_none(),
        "and nothing is left armed for a second press to take"
    );
}

#[test]
fn keys_c_arms_every_finished_row_with_its_reason_and_the_press_after_takes_them() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    for id in ["first-a1b", "second-b2c", "third-c3d", "quiet-d4e"] {
        idle(root.path(), id);
    }
    let mut screen = watching(vec![
        was_stopped("first-a1b"),
        was_stopped("second-b2c"),
        was_stopped("third-c3d"),
        // And one mid-turn, which `c` ignores.
        at_work("quiet-d4e"),
    ]);

    screen.act(c(), root.path(), &config, None).unwrap();
    let arm = screen.arm.as_ref().expect("the arm the press left");
    let mut marked: Vec<(&str, &str)> = arm
        .ids
        .iter()
        .map(String::as_str)
        .zip(arm.why.iter().map(String::as_str))
        .collect();
    marked.sort_unstable();
    assert_eq!(
        marked,
        [
            ("first-a1b", "stopped"),
            ("second-b2c", "stopped"),
            ("third-c3d", "stopped"),
        ],
        "every row whose turn is over, each with the reason it is on the \
             list, and the one still at work on none of it"
    );
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        4,
        "and the first press takes nothing"
    );

    screen.act(c(), root.path(), &config, None).unwrap();
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["quiet-d4e".to_string()],
        "the records of all three, which is what a stopped row has"
    );
    let Some(Notice::Advice(said)) = &screen.notice else {
        panic!("nothing said about what went")
    };
    assert_eq!(said, "cleared 3");
    assert!(screen.arm.is_none(), "and the arm is taken with them");
}

/// `c` covers the whole fleet, not only the rows the screen has room for.
#[test]
fn keys_c_reaches_the_finished_rows_the_fold_is_holding_back() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(a_folding_wall());
    assert!(
        screen
            .list
            .items()
            .iter()
            .any(|item| matches!(item, rows::Item::Fold(..))),
        "the wall this is asked of is drawing a fold"
    );

    screen
        .act(c(), root.path(), &Config::default(), None)
        .unwrap();
    let arm = screen.arm.as_ref().expect("the arm the press left");
    assert_eq!(
        arm.ids.len(),
        rows::FOLD_AT + 2,
        "every ended row on the wall, drawn or folded away: {:?}",
        arm.ids
    );
}

/// The arming window is five seconds, long enough to read a wall of reasons
/// before pressing again.
#[test]
fn keys_c_still_clears_four_seconds_after_the_press_that_armed_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    idle(root.path(), "first-a1b");
    let mut screen = watching(vec![was_stopped("first-a1b")]);
    screen.act(c(), root.path(), &config, None).unwrap();

    let arm = screen.arm.as_mut().expect("the arm the press left");
    arm.at = arm
        .at
        .checked_sub(Duration::from_secs(4))
        .expect("a machine that has been up longer than the window");
    screen.act(c(), root.path(), &config, None).unwrap();
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "the press was still inside the window"
    );
}

#[test]
fn keys_c_arms_what_has_landed_and_the_press_after_it_clears_them() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let first = has_landed(root.path(), repo.path(), "fix-login-a1b");
    let second = has_landed(root.path(), repo.path(), "port-importer-b2c");
    // And one still at work, which `c` ignores.
    idle(root.path(), "quiet-c3d");
    let mut screen = watching(vec![
        first,
        second,
        reading(
            "quiet-c3d",
            Phase::Idle,
            State {
                state: Phase::Idle,
                since: 1,
                last_event: 1,
                ..State::default()
            },
        ),
    ]);

    screen.act(c(), root.path(), &config, None).unwrap();
    let arm = screen.arm.as_ref().expect("the arm the press left");
    assert!(arm.cleared, "the arm c leaves is c's own");
    let mut marked: Vec<(&str, &str)> = arm
        .ids
        .iter()
        .map(String::as_str)
        .zip(arm.why.iter().map(String::as_str))
        .collect();
    marked.sort_unstable();
    assert_eq!(
        marked,
        [
            ("fix-login-a1b", "amx/fix-login-a1b merged into main"),
            (
                "port-importer-b2c",
                "amx/port-importer-b2c merged into main"
            ),
        ],
        "every row that has landed, with the reason it is on the list, \
             and the one still at work on neither"
    );
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        3,
        "and the first press takes nothing"
    );

    screen.act(c(), root.path(), &config, None).unwrap();
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["quiet-c3d".to_string()],
        "the trees, the branches and the records of both"
    );
    let Some(Notice::Advice(said)) = &screen.notice else {
        panic!("nothing said about what went")
    };
    assert_eq!(said, "cleared 2");
    assert!(screen.arm.is_none(), "and the arm is taken with them");
}

#[test]
fn keys_c_marks_a_tree_that_holds_work_no_commit_has_and_counts_it_when_it_keeps_it() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let held = has_landed(root.path(), repo.path(), "fix-login-a1b");
    std::fs::write(
        held.meta.worktree.as_deref().unwrap().join("login.rs"),
        "fn login() {}\n",
    )
    .unwrap();
    let gone = has_landed(root.path(), repo.path(), "port-importer-b2c");
    let mut screen = watching(vec![held, gone]);

    screen.act(c(), root.path(), &config, None).unwrap();
    let arm = screen.arm.as_ref().expect("the arm the press left");
    assert_eq!(
        arm.held,
        ["fix-login-a1b".to_string()],
        "the press asked each tree it found whether it holds work, and only \
             the one that does is marked as the press after it will leave it"
    );

    screen.act(c(), root.path(), &config, None).unwrap();
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("a row the press passed by was said as though it had gone")
    };
    assert_eq!(said, "cleared 1 · kept 1 holding work no commit has");
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["fix-login-a1b".to_string()],
        "the record that names the tree stays with it"
    );
}

/// The second press does what the rows said: a tree cleaned up inside the
/// window is still passed by, because the sweep is not asked twice.
#[test]
fn keys_c_passes_by_a_row_it_marked_whatever_the_tree_holds_by_the_second_press() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let held = has_landed(root.path(), repo.path(), "fix-login-a1b");
    let loose = held.meta.worktree.as_deref().unwrap().join("login.rs");
    std::fs::write(&loose, "fn login() {}\n").unwrap();
    let mut screen = watching(vec![held]);

    screen.act(c(), root.path(), &config, None).unwrap();
    std::fs::remove_file(&loose).unwrap();
    screen.act(c(), root.path(), &config, None).unwrap();
    assert_eq!(
        crate::store::list(root.path()).unwrap(),
        ["fix-login-a1b".to_string()],
        "the row the press was told to leave alone was never handed to the taker"
    );
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("nothing said about the row the press passed by")
    };
    assert_eq!(said, "cleared 0 · kept 1 holding work no commit has");
}

#[test]
fn keys_c_counts_the_trees_it_keeps_when_more_than_one_holds_work() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let holding: Vec<View> = ["fix-login-a1b", "port-importer-b2c"]
        .into_iter()
        .map(|id| {
            let view = has_landed(root.path(), repo.path(), id);
            std::fs::write(
                view.meta.worktree.as_deref().unwrap().join("login.rs"),
                "fn login() {}\n",
            )
            .unwrap();
            view
        })
        .collect();
    let mut screen = watching(holding);

    screen.act(c(), root.path(), &config, None).unwrap();
    screen.act(c(), root.path(), &config, None).unwrap();
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("nothing said about the rows the press passed by")
    };
    assert_eq!(said, "cleared 0 · kept 2 holding work no commit has");
    assert_eq!(
        crate::store::list(root.path()).unwrap().len(),
        2,
        "and neither record went"
    );
}

#[test]
fn keys_c_and_ctrl_x_do_not_finish_each_other_s_press() {
    let root = TempDir::new().unwrap();
    let repo = a_repo();
    let config = Config::default();
    let mut screen = watching(vec![has_landed(root.path(), repo.path(), "fix-login-a1b")]);
    let still_there = || crate::store::list(root.path()).unwrap().len();

    // A ctrl+x inside c's window forgets nothing c marked and arms its own row.
    screen.act(c(), root.path(), &config, None).unwrap();
    screen.act(ctrl('x'), root.path(), &config, None).unwrap();
    assert_eq!(
        still_there(),
        1,
        "the press that asked for nothing took nothing"
    );
    assert!(
        screen.arm.as_ref().is_some_and(|arm| !arm.cleared),
        "and the row under the cursor is armed for its own second press"
    );

    // The other way round: c does not finish a ctrl+x arm, it asks again.
    screen.act(c(), root.path(), &config, None).unwrap();
    assert_eq!(still_there(), 1, "which takes nothing either");
    assert!(
        screen.arm.as_ref().is_some_and(|arm| arm.cleared),
        "the arm is c's again, and it is a first press"
    );
}

/// The `w` key.
fn w() -> KeyEvent {
    KeyEvent::from(KeyCode::Char('w'))
}

/// A working agent.
fn at_work(id: &str) -> View {
    reading(
        id,
        Phase::Working,
        State {
            state: Phase::Working,
            since: 1,
            last_event: 1,
            ..State::default()
        },
    )
}

#[test]
fn keys_w_lands_the_cursor_on_the_first_agent_that_needs_you() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(vec![
        at_work("port-import-b2c"),
        stopped_on_a_question("ask-a1b"),
    ]);

    // From elsewhere on the wall, below the working rows.
    screen.list.bottom();
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "port-import-b2c",
        "the cursor starts away from the agent that is asking"
    );

    screen
        .act(w(), root.path(), &Config::default(), None)
        .unwrap();
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "ask-a1b",
        "the question is what is holding somebody up"
    );
    assert!(screen.notice.is_none(), "and the key had nothing to say");
    assert!(
        screen.card.is_none(),
        "the cursor is the whole of what the press moves"
    );
}

#[test]
fn keys_w_says_nothing_is_waiting_when_every_agent_is_at_work() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(vec![at_work("port-import-b2c"), at_work("fix-login-c3d")]);
    screen.list.bottom();
    let standing = screen.list.selected().unwrap().id().to_string();

    screen
        .act(w(), root.path(), &Config::default(), None)
        .unwrap();
    let Some(Notice::Advice(said)) = &screen.notice else {
        panic!("nothing said about a wall with nothing waiting on it")
    };
    assert_eq!(said, "nothing on the wall is waiting on you");
    assert_eq!(
        screen.list.selected().unwrap().id(),
        standing,
        "and the cursor is left where somebody put it"
    );
}

/// The backspace key.
fn backspace() -> KeyEvent {
    KeyEvent::from(KeyCode::Backspace)
}

/// A state root inside `state`. The trail file sits beside the agents
/// directory, so a root that is the temporary directory itself would put the
/// trail outside it.
fn a_root(state: &TempDir) -> PathBuf {
    state.path().join("agents")
}

#[test]
fn keys_backspace_lands_the_cursor_on_the_agent_you_were_last_in() {
    let state = TempDir::new().unwrap();
    let root = a_root(&state);
    let mut screen = watching(vec![at_work("port-import-b2c"), at_work("fix-login-c3d")]);

    // The terminal's trail, oldest first: the two rows on the wall, then an
    // agent since forgotten.
    for id in ["fix-login-c3d", "port-import-b2c", "forgotten-z9z"] {
        verbs::attach::note_visited(&root, id);
    }

    // On the agent visited last, going back reads the trail past the cursor's
    // row and past the name no longer on the wall.
    screen.list.land_on("port-import-b2c");
    screen
        .act(backspace(), &root, &Config::default(), None)
        .unwrap();
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "fix-login-c3d",
        "the agent this terminal was in before the one it is on"
    );
    assert!(screen.notice.is_none(), "and the key had nothing to say");
    assert!(
        screen.card.is_none(),
        "the cursor is the whole of what the press moves"
    );
}

#[test]
fn keys_backspace_says_there_is_nowhere_to_go_back_to() {
    let state = TempDir::new().unwrap();
    let root = a_root(&state);
    let mut screen = watching(vec![at_work("port-import-b2c"), at_work("fix-login-c3d")]);

    // A trail with only the cursor's agent and a forgotten one. The message is
    // `amx attach --last`'s own.
    for id in ["forgotten-z9z", "port-import-b2c"] {
        verbs::attach::note_visited(&root, id);
    }
    screen.list.land_on("port-import-b2c");

    screen
        .act(backspace(), &root, &Config::default(), None)
        .unwrap();
    let Some(Notice::Advice(said)) = &screen.notice else {
        panic!("nothing said about a terminal that has been nowhere else")
    };
    assert_eq!(said, "no agent to go back to");
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "port-import-b2c",
        "and the cursor is left where somebody put it"
    );
}

#[test]
fn acts_peeking_at_an_agent_writes_the_look_on_its_record() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);

    // The view opens on the newest ending, so the card opens on that one. The
    // wall shows nothing for having read a card, but the record keeps it for
    // the next view and the fold ordering.
    let (code, painted) = buffered(
        root.path(),
        &Scope::default(),
        vec![
            Typed::Key(KeyEvent::from(KeyCode::Char(' '))),
            // ctrl+c, because q is text on the card's line.
            Typed::Key(ctrl('c')),
        ],
        None,
        Painting::default(),
    );
    assert_eq!(code, exit::OK);
    let lines: Vec<String> = (0..painted.area().height)
        .map(|row| {
            (0..painted.area().width)
                .map(|column| painted[(column, row)].symbol())
                .collect()
        })
        .collect();
    let drawn = lines.join("\n");
    let weighty = |id: &str| {
        let (row, line) = lines
            .iter()
            .enumerate()
            .find(|(_, line)| line.contains(id))
            .unwrap_or_else(|| panic!("no row for {id}:\n{drawn}"));
        let at = line[..line.find(id).unwrap()].chars().count() as u16;
        painted[(at, row as u16)].modifier.contains(Modifier::BOLD)
    };
    for id in ["first-a1b", "second-b2c"] {
        assert!(
            !weighty(id),
            "the row somebody opened and the row they did not read alike:\n{drawn}"
        );
    }

    let looked = |id: &str| {
        crate::store::Agent::open(root.path(), id)
            .unwrap()
            .state()
            .unwrap()
            .seen
    };
    assert!(looked("first-a1b") > 0, "the look is on the record");
    assert_eq!(
        looked("second-b2c"),
        0,
        "and an agent nobody opened was not marked read on their behalf"
    );
}

#[test]
fn acts_alt_and_a_digit_reach_the_agent_at_that_place_on_the_wall() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);

    // Neither ended agent has anything recorded to pick up, so reaching fails
    // with its name, which shows which row was counted. The cursor stays on the
    // first.
    let (_, second) = pressing(
        root.path(),
        vec![alt('2'), KeyEvent::from(KeyCode::Char('q'))],
    );
    assert!(
        second.contains("no session was ever recorded for second-b2c"),
        "{second}"
    );

    let (_, first) = pressing(
        root.path(),
        vec![alt('1'), KeyEvent::from(KeyCode::Char('q'))],
    );
    assert!(
        first.contains("no session was ever recorded for first-a1b"),
        "{first}"
    );

    // A digit past the end of the wall says so. The digits count agents, not
    // headings.
    let (_, past) = pressing(
        root.path(),
        vec![alt('9'), KeyEvent::from(KeyCode::Char('q'))],
    );
    assert!(past.contains("fewer than 9"), "{past}");
}

#[test]
fn acts_ctrl_r_opens_the_line_on_the_name_the_row_is_carrying() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![reading(
        "fix-login-a1b",
        Phase::Idle,
        State {
            state: Phase::Idle,
            name: Some("auth".to_string()),
            ..State::default()
        },
    )]);

    screen.act(ctrl('r'), root.path(), &config, None).unwrap();
    let line = screen.banded().expect("a line of its own");
    assert_eq!(line.label(), "RENAME");
    assert_eq!(line.about().as_deref(), Some("fix-login-a1b"));
    assert_eq!(
        line.text, "auth",
        "seeded with what the row says, because a rename is an edit of it \
             rather than a name typed again from nothing"
    );
}

/// Every key a terminal can send the view: printable characters and named
/// keys, each under every modifier combination.
fn every_key() -> Vec<KeyEvent> {
    let named = [
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
    ];
    let chords = [
        KeyModifiers::NONE,
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ];
    (' '..='~')
        .map(KeyCode::Char)
        .chain(named)
        .chain((1..=12).map(KeyCode::F))
        .flat_map(|code| chords.map(move |held| KeyEvent::new(code, held)))
        .collect()
}

/// The name the keys overlay would give this key.
fn named(key: KeyEvent) -> String {
    let mut said = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        said.push_str("ctrl+");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        said.push_str("alt+");
    }
    // Shift is named only on BackTab; elsewhere it arrives as the character it
    // typed.
    if key.code == KeyCode::BackTab
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
    {
        said.push_str("shift+");
    }
    said.push_str(&match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(typed) => typed.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab | KeyCode::BackTab => "tab".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::PageUp => "pgup".to_string(),
        KeyCode::PageDown => "pgdn".to_string(),
        other => format!("{other:?}").to_lowercase(),
    });
    said
}

/// Whether the keys overlay names this key.
fn listed(key: KeyEvent) -> bool {
    let named = named(key);
    paint::HELP
        .iter()
        .any(|(keys, _)| keys.split(' ').any(|key| key == named || runs(key, &named)))
}

/// Whether a key column naming a run of keys, such as `alt+1..9`, names this
/// one.
fn runs(column: &str, named: &str) -> bool {
    let Some((first, last)) = column.split_once("..") else {
        return false;
    };
    let Some((chord, from)) = first.split_at_checked(first.len() - 1) else {
        return false;
    };
    match named.strip_prefix(chord) {
        Some(key) if key.len() == 1 => (from..=last).contains(&key),
        _ => false,
    }
}

/// Everything one keypress could change, as a string to compare.
fn standing(screen: &Screen) -> String {
    let mode = match &screen.mode {
        Mode::List => "list".to_string(),
        Mode::Keys => "keys".to_string(),
        Mode::Confirming(asked) => format!("confirming {}", asked.question()),
        Mode::Typing(composer) => format!("typing {} {}", composer.label(), composer.text),
    };
    let look = match screen.look {
        Look::Away => "away",
        Look::Screen => "screen",
        Look::Changes => "changes",
    };
    let notice = match &screen.notice {
        Some(Notice::Advice(said) | Notice::Refused(said) | Notice::Failed(said)) => said.as_str(),
        None => "",
    };
    let dials = &screen.profile;
    format!(
        "{mode} · {look} · {notice} · {:?} · {:?} · {} · {} · {} {} {} {} {}",
        screen.list,
        screen.card.as_ref().map(|card| (&card.id, card.changes)),
        screen.scroll.away.get(),
        screen.vendor,
        dials.agent,
        dials.model,
        dials.permission,
        dials.effort,
        dials.worktree,
    )
}

/// A fleet with every kind of line for the cursor: a running agent, group
/// headings and enough finished agents for a fold.
fn a_wall() -> Vec<View> {
    let mut views = vec![stopped_on_a_question("ask-a1b")];
    views.extend((0..5).map(|n| {
        reading(
            &format!("done-{n}"),
            Phase::Done,
            State {
                state: Phase::Done,
                exit: Some(0),
                since: 1,
                last_event: 1,
                ..State::default()
            },
        )
    }));
    views
}

/// [`a_wall`] with more finished agents than one group shows, so it folds.
fn a_folding_wall() -> Vec<View> {
    let mut views = a_wall();
    views.extend((5..rows::FOLD_AT + 2).map(|n| {
        reading(
            &format!("done-{n}"),
            Phase::Done,
            State {
                state: Phase::Done,
                exit: Some(0),
                since: 1,
                last_event: 1,
                ..State::default()
            },
        )
    }));
    views
}

/// A place the cursor can stand, and its name for failure messages.
type Standing = (&'static str, fn(&mut Screen));

/// Whether pressing `key` on a screen placed by `stand` does anything.
///
/// Keys that hand the terminal to tmux or an editor say so by what they return,
/// not by changing the screen.
fn acts_on(key: KeyEvent, root: &Path, stand: fn(&mut Screen)) -> bool {
    let mut screen = watching(a_wall());
    stand(&mut screen);

    let before = standing(&screen);
    let did = !matches!(
        screen.act(key, root, &Config::default(), None),
        Ok(Doing::Carry)
    );
    did || standing(&screen) != before
}

#[test]
fn keymap_every_key_the_list_acts_on_is_named_among_the_keys() {
    let root = TempDir::new().unwrap();
    // Each kind of line the cursor stops on, since one key does different
    // things on each, plus a card over the list.
    let standing: [Standing; 4] = [
        ("an agent's row", |_| {}),
        ("a heading", |screen| screen.list.up()),
        ("the fold", |screen| {
            for _ in 0..5 {
                screen.list.down();
            }
        }),
        ("a card", |screen| {
            screen.look = Look::Screen;
            screen.card = screen.list.selected().map(|view| {
                card_of(
                    view,
                    Path::new(""),
                    76,
                    Theme::default(),
                    &mut Heard::default(),
                )
                .0
            });
        }),
    ];

    for (where_it_is, stand) in standing {
        for key in every_key() {
            if !acts_on(key, root.path(), stand) {
                continue;
            }
            assert!(
                listed(key),
                "{} does something on {where_it_is} and is not among the \
                     keys, so nobody who pressed it could find out what it did",
                named(key)
            );
        }
    }
}

#[test]
fn keys_o_says_which_row_has_no_pull_request_and_leaves_a_heading_alone() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let o = KeyEvent::from(KeyCode::Char('o'));

    // A row with no branch has no pull request, and the message names the row.
    let mut screen = watching(a_wall());
    press(&mut screen, o);
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("nothing said about a row with no request");
    };
    assert_eq!(said, "no pull request on ask-a1b");

    // A heading has no pull request; nothing is said.
    let mut screen = watching(a_wall());
    screen.list.up();
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    press(&mut screen, o);
    assert!(screen.notice.is_none(), "a heading is left alone");
}

#[test]
fn keys_alt_d_answers_with_the_row_to_read_and_names_the_key_that_reads_it() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(a_wall());

    // With no diff key in the config, the message names the key to set.
    let doing = screen
        .act(alt('d'), root.path(), &Config::default(), None)
        .unwrap();
    assert!(matches!(doing, Doing::Carry), "nothing is borrowed for it");
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("nothing said about a view with no viewer");
    };
    assert!(said.contains("diff"), "the key is named: {said}");

    // With one, the key returns the cursor's agent for the loop to hand off.
    let config = Config {
        diff: Some("delta".to_string()),
        ..Config::default()
    };
    let doing = screen.act(alt('d'), root.path(), &config, None).unwrap();
    let Doing::View { id } = doing else {
        panic!("the patch was not handed anywhere");
    };
    assert_eq!(id, "ask-a1b");

    // A heading has no patch.
    let mut screen = watching(a_wall());
    screen.list.up();
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    let doing = screen.act(alt('d'), root.path(), &config, None).unwrap();
    assert!(matches!(doing, Doing::Carry), "a heading is left alone");
    assert!(screen.notice.is_none(), "and nothing is said about it");
}

#[test]
fn keys_a_bound_key_runs_on_the_row_and_is_read_after_every_key_amx_binds() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    // Three spellings: one unbound, and two the view already binds (a dial and
    // the close key).
    let bound = || {
        keyname::bound_by(&BTreeMap::from([
            ("alt+g".to_string(), "lazygit".to_string()),
            ("alt+a".to_string(), "never runs".to_string()),
            ("q".to_string(), "never runs either".to_string()),
        ]))
        .0
    };
    let watching_them = || {
        let mut screen = watching(a_wall());
        screen.bound = bound();
        screen
    };

    let mut screen = watching_them();
    let doing = screen.act(alt('g'), root.path(), &config, None).unwrap();
    let Doing::Bound {
        id,
        spelling,
        command,
    } = doing
    else {
        panic!("the key nothing else answers to did not reach the table");
    };
    assert_eq!(
        (id.as_str(), spelling.as_str(), command.as_str()),
        ("ask-a1b", "alt+g", "lazygit"),
        "the agent under the cursor, the spelling as it was written, and \
             what it runs"
    );

    // A key amx binds keeps its meaning: the dials are checked first, then the
    // list's own keys, then the config's.
    let mut screen = watching_them();
    let doing = screen.act(alt('a'), root.path(), &config, None).unwrap();
    assert!(matches!(doing, Doing::Carry), "alt+a is still the dial");
    let mut screen = watching_them();
    let doing = screen
        .act(
            KeyEvent::from(KeyCode::Char('q')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    assert!(matches!(doing, Doing::Close), "q still closes the view");

    // A heading has no tree to run the command in, as with alt+d.
    let mut screen = watching_them();
    screen.list.up();
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    let doing = screen.act(alt('g'), root.path(), &config, None).unwrap();
    assert!(matches!(doing, Doing::Carry), "a heading is left alone");
    assert!(screen.notice.is_none(), "and nothing is said about it");
}

/// A tmux server owned by the test, killed on drop.
///
/// `i` only acts on rows the reader calls live, which needs a real pane.
struct TestServer(Server);

impl TestServer {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "amx-test-view-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        // An empty config so the developer's ~/.tmux.conf cannot affect the
        // tests.
        Self(Server::named(&name).with_conf("/dev/null"))
    }

    /// A pane for this agent, in a session named as spawn names one, so the
    /// pane answers for the agent.
    fn pane(&self, id: &str) -> PaneId {
        self.0
            .new_session(&Spawn {
                name: Some(&format!("{}{id}", crate::tmux::SESSION_PREFIX)),
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .expect("a pane for it")
            .1
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

/// Writes the record a key is checked against.
fn a_record(
    root: &Path,
    id: &str,
    vendor: Option<&str>,
    (socket, pane): (Socket, PaneId),
    state: &State,
) -> Agent {
    let agent = Agent::create(
        root,
        &Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "port the importer".to_string(),
            agent: vendor.map(str::to_string),
            model: None,
            effort: None,
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket,
            pane,
            bg: false,
            session: None,
            transcript: None,
            created: now(),
        },
    )
    .unwrap();
    std::fs::write(
        agent.dir().join("state.json"),
        serde_json::to_vec(state).unwrap(),
    )
    .unwrap();
    agent
}

/// A socket and pane that nothing answers for.
fn no_pane() -> (Socket, PaneId) {
    (
        Socket::Name("amx-not-a-server".to_string()),
        PaneId::new("%404").unwrap(),
    )
}

#[test]
fn keys_i_cuts_short_the_turn_and_says_which_rows_have_none_to_cut() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let server = TestServer::new();
    let i = KeyEvent::from(KeyCode::Char('i'));
    // The notice's text and kind: the same words mean different things as
    // advice and as a refusal.
    let press = |views: Vec<View>| -> String {
        let mut screen = watching(views);
        screen.act(i, root.path(), &config, None).unwrap();
        match &screen.notice {
            Some(Notice::Advice(said)) => format!("advice: {said}"),
            Some(Notice::Refused(said)) => format!("refused: {said}"),
            Some(Notice::Failed(said)) => format!("failed: {said}"),
            None => "nothing said".to_string(),
        }
    };

    // A turn at a live pane is the one thing the key ends; the verb logs it
    // before sending the key.
    let working = State {
        state: Phase::Working,
        since: now(),
        last_event: now(),
        ..State::default()
    };
    let agent = a_record(
        root.path(),
        "port-a1b",
        Some("claude"),
        (server.0.socket().clone(), server.pane("port-a1b")),
        &working,
    );
    assert_eq!(
        press(vec![reading("port-a1b", Phase::Working, working)]),
        "advice: interrupted port-a1b"
    );
    let kinds: Vec<String> = agent
        .events()
        .unwrap()
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(kinds, ["interrupt"], "the turn it cut short is on the log");

    // The three rows nothing is sent to, each named by what is at its pane: a
    // question Escape would dismiss, a command with no vendor, and an agent
    // with no turn running.
    let asking = stopped_on_a_question("ask-b2c");
    a_record(
        root.path(),
        "ask-b2c",
        Some("claude"),
        (server.0.socket().clone(), server.pane("ask-b2c")),
        &asking.state,
    );
    assert_eq!(
        press(vec![asking]),
        "refused: ask-b2c is waiting on a question, not working; esc on \
             its card dismisses it"
    );

    let starting = State {
        state: Phase::Starting,
        since: now(),
        last_event: now(),
        ..State::default()
    };
    a_record(
        root.path(),
        "build-c3d",
        None,
        (server.0.socket().clone(), server.pane("build-c3d")),
        &starting,
    );
    assert_eq!(
        press(vec![reading("build-c3d", Phase::Working, starting)]),
        "refused: build-c3d is a command, not an agent; ctrl+x stops it"
    );

    // A parked agent reads idle, so the refusal says what the verb says: amx
    // took its pane away.
    let parked = State {
        state: Phase::Idle,
        since: now(),
        last_event: now(),
        parked_at: now(),
        ..State::default()
    };
    let agent = a_record(root.path(), "quiet-d4e", Some("claude"), no_pane(), &parked);
    let mut row = reading("quiet-d4e", Phase::Idle, parked);
    row.verdict.evidence = Evidence::LetGo;
    assert_eq!(
        press(vec![row]),
        "refused: quiet-d4e is parked; nothing is running to interrupt"
    );
    assert!(
        agent.events().unwrap().is_empty(),
        "and a refusal writes nothing down"
    );
}

#[test]
fn keys_i_on_a_heading_is_a_key_about_no_agent_at_all() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(a_wall());
    screen.list.up();
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    screen
        .act(
            KeyEvent::from(KeyCode::Char('i')),
            root.path(),
            &Config::default(),
            None,
        )
        .unwrap();
    assert!(screen.notice.is_none(), "a heading has no turn in it");
}

#[test]
fn keys_f_opens_a_line_that_copies_the_row_and_turns_a_command_away() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let f = KeyEvent::from(KeyCode::Char('f'));
    let press = |screen: &mut Screen| {
        screen.act(f, root.path(), &config, None).unwrap();
    };

    // On an agent's row the fork line opens, bound to that agent however the
    // cursor moves while the task is typed.
    let mut agent = reading("port-a1b", Phase::Idle, State::default());
    agent.meta.agent = Some("claude".to_string());
    let mut screen = watching(vec![agent]);
    press(&mut screen);
    let Mode::Typing(line) = &screen.mode else {
        panic!("no line was opened on the agent's row");
    };
    assert_eq!(line.label(), "FORK");
    assert_eq!(line.about().as_deref(), Some("port-a1b"));
    assert!(screen.notice.is_none(), "and nothing to say about it");

    // A command has no vendor and so no conversation to copy; the key says so
    // and opens no line.
    let mut screen = watching(vec![reading("ls-b2c", Phase::Working, State::default())]);
    press(&mut screen);
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("nothing said about a row with no conversation to copy");
    };
    assert_eq!(
        said,
        "ls-b2c is a command, not an agent; there is no conversation to copy"
    );
    assert!(
        matches!(screen.mode, Mode::List),
        "and the wall is still what is on the screen"
    );
}

#[test]
fn keys_f_enters_an_empty_line_as_a_copy_waiting_for_a_turn() {
    // The fork line is the only line entered empty, which forks with no first
    // turn. The verb reports an unknown agent, which is as far as an empty root
    // gets.
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut agent = reading("port-a1b", Phase::Idle, State::default());
    agent.meta.agent = Some("claude".to_string());
    let mut screen = watching(vec![agent]);
    screen.mode = Mode::Typing(Composer::new(Asking::Fork {
        id: "port-a1b".to_string(),
    }));
    screen
        .act(KeyEvent::from(KeyCode::Enter), root.path(), &config, None)
        .unwrap();
    let Some(Notice::Failed(said)) = &screen.notice else {
        panic!("the empty line was dropped instead of being entered");
    };
    assert!(said.contains("no agent"), "{said}");
}

#[test]
fn keys_f_on_a_heading_is_a_key_about_no_agent_at_all() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(a_wall());
    screen.list.up();
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    screen
        .act(
            KeyEvent::from(KeyCode::Char('f')),
            root.path(),
            &Config::default(),
            None,
        )
        .unwrap();
    assert!(screen.notice.is_none(), "a heading is no agent to copy");
    assert!(matches!(screen.mode, Mode::List), "and no line is opened");
}

/// The headings and agents on the wall, in drawing order.
fn wall(screen: &Screen) -> Vec<String> {
    screen
        .list
        .items()
        .iter()
        .filter_map(|item| match item {
            rows::Item::Heading(under, _) => Some(screen.list.title(*under)),
            rows::Item::Agent(_) => screen.list.agent(*item).map(|view| view.id().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn keys_z_puts_the_row_under_everything_and_again_wakes_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let z = KeyEvent::from(KeyCode::Char('z'));

    let mut screen = watching(vec![
        stopped_on_a_question("ask-a1b"),
        finished_saying("done-b2c", "the answer"),
    ]);
    assert_eq!(
        wall(&screen),
        ["Needs input", "ask-a1b", "Completed", "done-b2c"]
    );

    // Moved under everything, even though it is the one agent asking.
    press(&mut screen, z);
    assert_eq!(
        wall(&screen),
        ["Completed", "done-b2c", "Asleep", "ask-a1b"]
    );
    assert!(
        screen
            .list
            .selected()
            .is_some_and(|view| screen.list.sleeping(view)),
        "and the cursor went with it"
    );

    // The same key wakes it.
    press(&mut screen, z);
    assert_eq!(
        wall(&screen),
        ["Needs input", "ask-a1b", "Completed", "done-b2c"]
    );

    // A heading is not a row to put away.
    screen.list.up();
    assert!(screen.list.on_heading(), "the cursor is on the heading");
    press(&mut screen, z);
    assert_eq!(
        wall(&screen),
        ["Needs input", "ask-a1b", "Completed", "done-b2c"]
    );
}

#[test]
fn keymap_the_letters_the_card_gave_up_are_bound_nowhere() {
    let root = TempDir::new().unwrap();
    // Both were card keys: r opened it and h closed it. Every card now ends
    // with a line, so over one they are text; space and l open the card and esc
    // closes it.
    for key in [
        KeyEvent::from(KeyCode::Char('r')),
        KeyEvent::from(KeyCode::Char('h')),
    ] {
        assert!(
            !acts_on(key, root.path(), |_| {}),
            "{} is not a key of this view",
            named(key)
        );
    }
}

#[test]
fn keymap_a_chord_the_view_never_bound_reaches_none_of_its_keys() {
    let root = TempDir::new().unwrap();
    // Each carries a key the list acts on, but under a modifier the list never
    // binds: alt+q is window management, not a request to close the view.
    for key in [alt('q'), ctrl('q'), ctrl('n'), alt('?')] {
        assert!(
            !acts_on(key, root.path(), |_| {}),
            "{} is not a key of this view",
            named(key)
        );
    }
}

#[test]
fn glyphs_and_notices_take_their_severity_from_the_writer() {
    assert!(
        matches!(
            said(Ok("started fix-login-a1b".into())),
            Some(Notice::Advice(_))
        ),
        "what an action came back with is advice, whatever it says"
    );
    assert!(
        matches!(
            said(Err(anyhow::anyhow!("git is busy"))),
            Some(Notice::Failed(_))
        ),
        "and what went wrong is a failure"
    );
}

#[test]
fn axis_turns_under_the_key_that_says_so() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);

    let (code, screen) = pressing(
        root.path(),
        vec![ctrl('s'), KeyEvent::from(KeyCode::Char('q'))],
    );
    assert_eq!(code, exit::OK);
    let drawn: Vec<&str> = screen.lines().map(str::trim_end).collect();
    // Only where the heading starts; what it carries beside the path and its
    // inset belong to the wall's own tests.
    assert!(
        drawn[2].trim_start().starts_with("/srv/app"),
        "the heading is where the agent is, not what it needs:\n{screen}"
    );
    assert!(
        drawn[3].contains("done"),
        "and the row carries the state the heading used to say:\n{screen}"
    );
    assert!(
        drawn[0].contains("1 done"),
        "what there is does not change with the way it is laid out:\n{screen}"
    );
}

#[test]
fn axis_narrows_the_list_by_name_from_the_find_line() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);

    let mut keys = vec![KeyCode::Char('/')];
    keys.extend(word("second"));
    keys.push(KeyCode::Enter);
    keys.push(KeyCode::Char('q'));

    let (code, screen) = held(root.path(), &keys);
    assert_eq!(code, exit::OK);
    assert!(
        screen.contains("/second"),
        "the header reads it back in the words it was typed in:\n{screen}"
    );
    assert!(screen.contains("second-b2c"), "{screen}");
    assert!(
        !screen.contains("first-a1b"),
        "the one that was named is the one that is left:\n{screen}"
    );
    assert!(
        crate::store::list(root.path()).unwrap().len() == 2,
        "and a line that narrows starts nothing"
    );
}

#[test]
fn axis_narrows_the_list_by_state_from_the_find_line() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);

    // The tokens belong to the find line now, which narrows as they are typed.
    let mut keys = vec![KeyCode::Char('/')];
    keys.extend(word("s:working"));
    keys.push(KeyCode::Enter);
    keys.push(KeyCode::Char('q'));

    let (code, screen) = held(root.path(), &keys);
    assert_eq!(code, exit::OK);
    assert!(screen.contains("s:working"), "{screen}");
    assert!(
        !screen.contains("first-a1b") && !screen.contains("second-b2c"),
        "neither of them is working:\n{screen}"
    );
    assert!(
        crate::store::list(root.path()).unwrap().len() == 2,
        "and a line that narrows starts nothing"
    );
}

#[test]
fn view_carries_the_waiting_count_in_what_the_terminal_is_called() {
    let waiting = watching(vec![
        stopped_on_a_question("ask-a1b"),
        stopped_on_a_question("ask-b2c"),
        reading(
            "busy-c3d",
            Phase::Working,
            State {
                state: Phase::Working,
                ..State::default()
            },
        ),
    ]);
    assert_eq!(
        paint::title(&waiting.list),
        "amx · 2 waiting",
        "the one question a tab bar can answer from across the room"
    );

    let quiet = watching(vec![reading(
        "busy-a1b",
        Phase::Working,
        State {
            state: Phase::Working,
            ..State::default()
        },
    )]);
    assert_eq!(
        paint::title(&quiet.list),
        "amx",
        "and a fleet with nothing waiting says nothing about a count"
    );
}

#[test]
fn view_says_what_to_call_the_terminal_when_it_changes_and_not_otherwise() {
    let root = TempDir::new().unwrap();
    let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
    let mut said = Said::default();

    watch(
        root.path(),
        &Config::default(),
        None,
        &Scope::default(),
        &mut terminal,
        &mut Script(vec![Typed::Key(KeyEvent::from(KeyCode::Down))].into_iter()),
        None,
        None,
        &mut said,
        Painting::default(),
    )
    .unwrap();

    assert_eq!(
        said.0,
        ["amx"],
        "said once and not again on every frame that did not change it"
    );
}

/// A config binding one key to a command, and one spelling nothing can press.
fn binding() -> Config {
    Config {
        keys: BTreeMap::from([
            ("alt+g".to_string(), "lazygit".to_string()),
            ("shift+z".to_string(), "never runs".to_string()),
        ]),
        ..Config::default()
    }
}

#[test]
fn view_says_the_spellings_it_could_not_read_on_the_frame_it_opens_on() {
    let root = TempDir::new().unwrap();
    let refused = "keys: `shift+z` is no key the view can read";

    let opening = drawn_under(root.path(), &binding(), Vec::new());
    assert!(
        opening.contains(refused),
        "a key somebody bound and nothing can press is worth the sentence \
             it takes to say so:\n{opening}"
    );

    let after = drawn_under(
        root.path(),
        &binding(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Down))],
    );
    assert!(
        !after.contains("no key the view can read"),
        "and it is said the once: the file is read when the view opens, \
             and a person who has read the sentence is done with it:\n{after}"
    );
}

#[test]
fn view_shows_the_keys_somebody_bound_where_it_shows_the_ones_it_binds() {
    let root = TempDir::new().unwrap();
    // The keys overlay paged to its end: a screen this small holds a few keys
    // at a time, and the user's bindings follow amx's own.
    let mut script = vec![Typed::Key(KeyEvent::from(KeyCode::Char('?')))];
    script.extend((0..paint::HELP.len()).map(|_| Typed::Key(KeyEvent::from(KeyCode::PageDown))));
    let keys = drawn_under(root.path(), &binding(), script);

    assert!(
        keys.contains("alt+g"),
        "the key the config file bound is on the screen that answers what \
             the keys are:\n{keys}"
    );
    assert!(
        keys.contains("lazygit"),
        "against the command it runs, which is the whole of what it is:\n{keys}"
    );
}

#[test]
fn view_closes_when_somebody_closes_it() {
    let root = TempDir::new().unwrap();
    assert_eq!(held(root.path(), &[KeyCode::Char('q')]).0, exit::OK);
}

#[test]
fn view_opened_about_a_directory_draws_that_directory_alone() {
    let root = TempDir::new().unwrap();
    finished_in(root.path(), "here-a1b", "wrote the parser", 60, "/srv/app");
    finished_in(
        root.path(),
        "deeper-b2c",
        "wrote the tests",
        90,
        "/srv/app/importer",
    );
    // The one a plain string comparison would have grouped with them.
    finished_in(root.path(), "alike-c3d", "read the log", 120, "/srv/app2");
    finished_in(root.path(), "far-d4e", "cut a release", 150, "/srv/other");

    let scope = Scope::of(Some(Path::new("/srv/app"))).unwrap();
    let (code, screen) = drawn_about(
        root.path(),
        &scope,
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
        None,
    );

    assert_eq!(code, exit::OK);
    for drawn in ["here-a1b", "deeper-b2c"] {
        assert!(screen.contains(drawn), "{drawn} is under it:\n{screen}");
    }
    for other in ["alike-c3d", "far-d4e"] {
        assert!(
            !screen.contains(other),
            "{other} is somebody else's afternoon:\n{screen}"
        );
    }
    assert!(
        screen.contains("2 done"),
        "and the count is of what was drawn, not of the machine:\n{screen}"
    );
}

#[test]
fn view_ends_when_there_is_nobody_at_the_terminal() {
    let root = TempDir::new().unwrap();
    let (code, screen) = held(root.path(), &[]);
    assert_eq!(code, exit::OK);
    assert!(screen.contains("no agents"), "{screen}");
}

#[test]
fn view_walks_the_agents_and_peeks_at_the_one_under_the_cursor() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    finished(root.path(), "second-b2c", "wrote the tests", 120);

    // Down onto the older one and open its card. The view is closed with
    // ctrl+c, because q is text on the card's line.
    let (code, screen) = pressing(
        root.path(),
        vec![
            KeyEvent::from(KeyCode::Down),
            KeyEvent::from(KeyCode::Char(' ')),
            ctrl('c'),
        ],
    );
    assert_eq!(code, exit::OK);
    assert!(screen.contains(" ∙ second-b2c"), "{screen}");
    assert!(
        screen
            .lines()
            .any(|line| line.starts_with("∙ second-b2c · claude ┈")),
        "an agent with no pane left is read from its record, onto a card \
             at the foot under a rule of its own: {screen}"
    );
    assert!(
        screen.contains("\nwrote the tests"),
        "with what it said standing under that rule, off the same edge: \
             {screen}"
    );
}

#[test]
fn view_says_it_cannot_reach_an_agent_rather_than_going_quiet() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);

    let (_, screen) = held(root.path(), &[KeyCode::Enter, KeyCode::Char('q')]);
    assert!(
        screen.contains("no session was ever recorded for first-a1b"),
        "an agent with nothing behind it to pick up is nowhere to be \
             taken, and the view says which is missing: {screen}"
    );
}

#[test]
fn a_line_being_typed_has_the_keys_of_the_list_in_it() {
    // Each of these is a list key, but while a line is open they are text.
    let root = TempDir::new().unwrap();
    let mut keys = vec![KeyCode::Char('n')];
    keys.extend(word("drop the queue and quit"));
    keys.push(KeyCode::Char('q'));

    let (code, screen) = held(root.path(), &keys);
    assert_eq!(code, exit::OK, "and the view did not close on any of them");
    assert!(screen.contains("drop the queue and quitq"), "{screen}");
}

#[test]
fn composer_takes_a_pasted_task_as_one_edit_and_dispatches_none_of_it() {
    let root = TempDir::new().unwrap();
    let (code, screen) = driving(
        root.path(),
        vec![Typed::Paste(
            "port the importer\nand its tests\n".to_string(),
        )],
    );

    assert_eq!(code, exit::OK);
    assert!(screen.contains("❯ port the importer"), "{screen}");
    assert!(
        screen.contains("  and its tests"),
        "every line of it is on the line being typed: {screen}"
    );
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "and the newlines in it are text, not enters"
    );
}

#[test]
fn composer_folds_a_long_paste_on_the_lines_a_paragraph_is_written_on() {
    let config = Config::default();
    let long = (1..=20)
        .map(|n| format!("row-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let line = |screen: &Screen| match &screen.mode {
        Mode::Typing(composer) => (composer.text.clone(), composer.whole()),
        _ => panic!("the line is not open"),
    };
    let typing = |asking| Screen {
        mode: Mode::Typing(Composer::new(asking)),
        ..Screen::default()
    };

    // At the list, a paste opens a task line: twenty rows fold into one marker,
    // and the line sends all of them.
    let mut screen = Screen::default();
    screen.pasted(&long, &config);
    assert_eq!(
        line(&screen),
        ("[Pasted text #1]".to_string(), long.clone())
    );

    // A reply is the other line long pastes fold on.
    let mut screen = typing(Asking::Reply);
    screen.pasted(&long, &config);
    assert_eq!(
        line(&screen),
        ("[Pasted text #1]".to_string(), long.clone())
    );

    // A name and a find line are single words, so a marker on either is
    // useless.
    for asking in [
        Asking::Name {
            id: "ask-a1b".to_string(),
        },
        Asking::Find,
    ] {
        let mut screen = typing(asking);
        screen.pasted(&long, &config);
        assert_eq!(line(&screen), (long.clone(), long.clone()));
    }
}

#[test]
fn composer_adds_a_paste_to_the_line_somebody_was_already_typing() {
    let root = TempDir::new().unwrap();
    let mut script = vec![Typed::Key(KeyEvent::from(KeyCode::Char('n')))];
    script.extend(
        word("port ")
            .into_iter()
            .map(|code| Typed::Key(KeyEvent::from(code))),
    );
    // Carriage returns in a paste read as newlines.
    script.push(Typed::Paste("the importer\rand its tests".to_string()));

    let (_, screen) = driving(root.path(), script);
    assert!(screen.contains("❯ port the importer"), "{screen}");
    assert!(screen.contains("  and its tests"), "{screen}");
}

#[test]
fn composer_walks_its_cursor_and_types_where_it_is_left_standing() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = Screen::default();
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let line = |screen: &Screen| match &screen.mode {
        Mode::Typing(composer) => (composer.text.clone(), composer.at),
        _ => panic!("the line is not open"),
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for key in word("port the importer") {
        press(&mut screen, KeyEvent::from(key));
    }
    assert_eq!(line(&screen).1, 17, "typing leaves the cursor after it");

    // The arrows move a character, home/end and ctrl+a/ctrl+e move to the ends,
    // and ctrl with an arrow moves a word.
    for (key, at) in [
        (KeyEvent::from(KeyCode::Left), 16),
        (KeyEvent::from(KeyCode::Right), 17),
        (KeyEvent::from(KeyCode::Home), 0),
        (KeyEvent::from(KeyCode::End), 17),
        (ctrl('a'), 0),
        (ctrl('e'), 17),
        (KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL), 9),
        (KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL), 5),
        (KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL), 8),
    ] {
        press(&mut screen, key);
        assert_eq!(line(&screen).1, at, "{key:?}");
    }

    // Typing inserts at the cursor.
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    assert_eq!(line(&screen), ("port then importer".to_string(), 9));
}

#[test]
fn composer_enters_a_word_already_spelled_the_way_the_choice_spells_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let line = |screen: &Screen| match &screen.mode {
        Mode::Typing(composer) => (composer.text.clone(), composer.at),
        _ => panic!("the line is not open"),
    };
    // Suggestions set by hand: the catalog is read from the home directory,
    // which a test must not write to, and the test is about what the key does
    // with them.
    let banded = |screen: &mut Screen, word: std::ops::Range<usize>, spelled: &[&str]| {
        let Mode::Typing(composer) = &mut screen.mode else {
            panic!("the line is not open");
        };
        composer.suggest = Some(act::Suggest {
            word,
            entries: spelled
                .iter()
                .map(|spelled| crate::catalog::Entry {
                    spelled: spelled.to_string(),
                    kind: crate::catalog::Kind::Skill,
                    about: String::new(),
                })
                .collect(),
            chosen: 0,
        });
    };

    // On a word short of the chosen suggestion, enter completes it like tab and
    // the line stays open.
    let mut screen = Screen::default();
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for key in word("agent:claude /rev") {
        press(&mut screen, KeyEvent::from(key));
    }
    banded(&mut screen, 13..17, &["/review", "/revise"]);
    press(&mut screen, KeyEvent::from(KeyCode::Enter));
    assert_eq!(
        line(&screen),
        ("agent:claude /review ".to_string(), 21),
        "enter on a word still being written finishes the word"
    );
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "and starts nothing"
    );

    // On a word already spelled as its only suggestion, enter acts on the line.
    // A three-character task, so enter asks before starting, which a test can
    // observe.
    let mut screen = Screen::default();
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for key in word("agent:claude /go") {
        press(&mut screen, KeyEvent::from(key));
    }
    banded(&mut screen, 13..16, &["/go"]);
    press(&mut screen, KeyEvent::from(KeyCode::Enter));
    match &screen.mode {
        Mode::Confirming(Asked::Slight { task, .. }) => assert_eq!(task, "/go"),
        _ => panic!("enter on a finished word is enter on the line"),
    }
}

#[test]
fn composer_asks_for_the_agents_when_tab_is_pressed_on_an_empty_task_line() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let line = |screen: &Screen| match &screen.mode {
        Mode::Typing(composer) => composer.text.clone(),
        _ => panic!("the line is not open"),
    };
    let typing = |asking| Screen {
        mode: Mode::Typing(Composer::new(asking)),
        ..Screen::default()
    };

    // On an empty line, tab writes the `@` mark that opens suggestions.
    let mut screen = Screen::default();
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    press(&mut screen, KeyEvent::from(KeyCode::Tab));
    assert_eq!(line(&screen), "@");

    // On a line with a word, tab has that word; one with no suggestions is left
    // as typed.
    let mut screen = Screen::default();
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for key in word("port") {
        press(&mut screen, KeyEvent::from(key));
    }
    press(&mut screen, KeyEvent::from(KeyCode::Tab));
    assert_eq!(line(&screen), "port");

    // The other lines do not read the mark, so tab does what it did before.
    for asking in [
        Asking::Reply,
        Asking::Name {
            id: "ask-a1b".to_string(),
        },
        Asking::Find,
    ] {
        let mut screen = typing(asking);
        press(&mut screen, KeyEvent::from(KeyCode::Tab));
        assert_eq!(line(&screen), "", "and puts nothing on them");
    }
}

#[test]
fn composer_completes_a_file_of_the_project_the_line_was_opened_under() {
    // A line opened under a project heading runs in that project, so it
    // suggests that project's files; the view's own directory has none of
    // these.
    let root = TempDir::new().unwrap();
    let there = TempDir::new().unwrap();
    std::fs::write(there.path().join("quenched.md"), "").unwrap();
    let config = Config::default();
    let mut screen = Screen::default();
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    let Mode::Typing(composer) = &mut screen.mode else {
        panic!("the line is not open");
    };
    composer.under = Some(there.path().to_path_buf());

    for key in word("read @quen") {
        press(&mut screen, KeyEvent::from(key));
    }
    press(&mut screen, KeyEvent::from(KeyCode::Tab));
    assert_eq!(
        screen.banded().expect("the line").text,
        "read @quenched.md ",
        "the word is completed out of the project under the heading"
    );
}

#[test]
fn composer_takes_back_what_the_cursor_is_standing_after() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = Screen::default();
    let press = |screen: &mut Screen, key| {
        screen.act(key, root.path(), &config, None).unwrap();
    };
    let line = |screen: &Screen| match &screen.mode {
        Mode::Typing(composer) => (composer.text.clone(), composer.at),
        _ => panic!("the line is not open"),
    };

    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for key in word("port thee importer") {
        press(&mut screen, KeyEvent::from(key));
    }

    // Backspace deletes the character behind the cursor, delete the one under
    // it.
    press(
        &mut screen,
        KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
    );
    press(&mut screen, KeyEvent::from(KeyCode::Left));
    press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    assert_eq!(line(&screen), ("port the importer".to_string(), 8));
    press(&mut screen, KeyEvent::from(KeyCode::Delete));
    assert_eq!(line(&screen), ("port theimporter".to_string(), 8));

    // The word behind the cursor goes whole with either ctrl+w or
    // alt+backspace.
    press(&mut screen, ctrl('w'));
    assert_eq!(line(&screen), ("port importer".to_string(), 5));
    press(
        &mut screen,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
    );
    assert_eq!(line(&screen), ("importer".to_string(), 0));

    press(&mut screen, KeyEvent::from(KeyCode::Backspace));
    assert_eq!(
        line(&screen),
        ("importer".to_string(), 0),
        "a key pressed at the front of the line takes nothing"
    );
}

#[test]
fn composer_lands_a_paste_where_the_cursor_stands() {
    let root = TempDir::new().unwrap();
    let mut script = vec![Typed::Key(KeyEvent::from(KeyCode::Char('n')))];
    script.extend(
        word("port importer")
            .into_iter()
            .map(|code| Typed::Key(KeyEvent::from(code))),
    );
    script.push(Typed::Key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::CONTROL,
    )));
    script.push(Typed::Paste("the ".to_string()));

    let (_, screen) = driving(root.path(), script);
    assert!(
        screen.contains("❯ port the importer"),
        "a paste is one edit, and it lands where the block is: {screen}"
    );
}

#[test]
fn composer_takes_a_newline_from_the_key_that_makes_one_and_stays_open() {
    let root = TempDir::new().unwrap();
    let mut keys = vec![KeyEvent::from(KeyCode::Char('n'))];
    keys.extend(word("port the importer").into_iter().map(KeyEvent::from));
    keys.push(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    keys.extend(word("and its tests").into_iter().map(KeyEvent::from));

    let (code, screen) = pressing(root.path(), keys);
    assert_eq!(code, exit::OK);
    assert!(screen.contains("❯ port the importer"), "{screen}");
    assert!(screen.contains("  and its tests"), "{screen}");
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "and the enter that makes a newline is the one that starts nothing"
    );
}

#[test]
fn composer_keeps_a_line_a_dial_refused_where_it_was_typed() {
    let root = TempDir::new().unwrap();
    let mut keys = vec![KeyCode::Char('n')];
    keys.extend(word("p:nonsense port it"));
    keys.push(KeyCode::Enter);

    let (code, screen) = held(root.path(), &keys);
    assert_eq!(code, exit::OK);
    assert!(
        screen.contains("❯ p:nonsense port it"),
        "a line nothing was made from is a line somebody is still \
             writing: {screen}"
    );
    assert!(screen.contains("p:nonsense: claude takes"), "{screen}");
    assert!(crate::store::list(root.path()).unwrap().is_empty());
}

#[test]
fn composer_alt_n_enters_the_line_the_way_enter_does_and_goes_with_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = Screen::default();
    let press = |screen: &mut Screen, key: KeyEvent| {
        screen.act(key, root.path(), &config, None).unwrap();
    };

    // A line the dials refuse is refused whichever key entered it, and nothing
    // is started.
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for code in word("p:nonsense port it") {
        press(&mut screen, KeyEvent::from(code));
    }
    press(&mut screen, alt('n'));
    assert_eq!(
        screen
            .banded()
            .expect("the line stays where it was typed")
            .text,
        "p:nonsense port it"
    );
    assert!(crate::store::list(root.path()).unwrap().is_empty());
    assert!(
        matches!(screen.notice, Some(Notice::Refused(_))),
        "and the reason under it is said as something that did not \
             happen rather than as advice"
    );

    // A task barely long enough to be one asks first, whichever key entered it.
    let mut screen = Screen::default();
    press(&mut screen, KeyEvent::from(KeyCode::Char('n')));
    for code in word("fix") {
        press(&mut screen, KeyEvent::from(code));
    }
    press(&mut screen, alt('n'));
    let Mode::Confirming(Asked::Slight { follow, .. }) = &screen.mode else {
        panic!("three letters were started without a question")
    };
    assert!(*follow, "and the answer takes whoever asked to the agent");
}

#[test]
fn acts_the_view_reaches_an_agent_it_started_by_reading_the_record_again() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    let mut screen = Screen::default();

    // Read from the record: the list is a reading old and does not know the new
    // agent.
    screen.landing(root.path(), "first-a1b", None).unwrap();
    let Some(Notice::Refused(said)) = &screen.notice else {
        panic!("nothing was said about where the agent went")
    };
    // Nothing was recorded to resume this one, so the message names what is
    // missing.
    assert!(said.contains("first-a1b"), "{said}");
    assert!(said.contains("session"), "{said}");
}

#[test]
fn acts_enter_on_an_agent_with_nothing_to_continue_says_which_is_missing() {
    // The pane is gone, but what blocks enter is that the agent never had a
    // session to resume, so that is what the message says.
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);
    let view = derive::view(root.path(), "first-a1b", now()).unwrap();

    let Reach::Say(Notice::Refused(said)) = reach(root.path(), None, &view).unwrap() else {
        panic!("an agent with nothing to continue was reached anyway")
    };
    assert!(
        !said.contains("no pane any more"),
        "which is a fact about the pane, not a reason: {said}"
    );
    assert!(said.contains("amx new"), "and what to do instead: {said}");
}

#[test]
fn composer_ctrl_g_takes_the_line_to_the_editor_and_leaves_it_open() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = Screen::default();

    // On the list, ctrl+g opens a task line and goes straight to the editor.
    let doing = screen.act(ctrl('g'), root.path(), &config, None).unwrap();
    assert!(matches!(doing, Doing::Edit));
    let line = screen.banded().expect("a line for the editor to fill");
    assert_eq!(line.label(), "TASK");
    assert!(line.text.is_empty());

    // On a line already being typed, that line goes to the editor and comes
    // back to the same composer.
    let Mode::Typing(composer) = &mut screen.mode else {
        panic!("no line to edit")
    };
    composer.insert("port the importer");
    let doing = screen.act(ctrl('g'), root.path(), &config, None).unwrap();
    assert!(matches!(doing, Doing::Edit));
    assert_eq!(
        screen.banded().expect("the line is still there").text,
        "port the importer"
    );
}

#[test]
fn composer_asks_once_before_starting_an_agent_on_a_task_of_three_letters() {
    let root = TempDir::new().unwrap();
    let mut keys = vec![KeyCode::Char('n')];
    keys.extend(word("fix"));
    keys.push(KeyCode::Enter);

    let (code, asked) = held(root.path(), &keys);
    assert_eq!(code, exit::OK);
    assert!(
        asked.contains("start an agent on \"fix\"? y"),
        "the question quotes what would be started:\n{asked}"
    );
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "and the question is all that has happened"
    );

    // Any key but y keeps the line exactly as typed.
    keys.push(KeyCode::Char('n'));
    let (_, kept) = held(root.path(), &keys);
    assert!(kept.contains("❯ fix"), "{kept}");
    assert!(kept.contains("nothing was started"), "{kept}");
    assert!(crate::store::list(root.path()).unwrap().is_empty());
}

#[test]
fn a_line_nobody_entered_does_nothing_at_all() {
    let root = TempDir::new().unwrap();
    let mut keys = vec![KeyCode::Char('n')];
    keys.extend(word("port the importer"));
    keys.push(KeyCode::Esc);
    keys.push(KeyCode::Char('q'));

    let (code, screen) = held(root.path(), &keys);
    assert_eq!(code, exit::OK, "and q is the list's key again");
    assert!(
        !screen.contains("port the importer"),
        "the line is gone with it: {screen}"
    );
    assert!(
        crate::store::list(root.path()).unwrap().is_empty(),
        "and nothing was started"
    );
}

/// A mouse event, as crossterm hands one to the view.
fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// A left click: the press records the position and the release acts, since a
/// click and the start of a drag look the same until the button comes up.
fn click(screen: &mut Screen, column: u16, row: u16, root: &Path) -> Result<Doing> {
    screen.moused(
        mouse(MouseEventKind::Down(MouseButton::Left), column, row),
        root,
        None,
    )?;
    screen.moused(
        mouse(MouseEventKind::Up(MouseButton::Left), column, row),
        root,
        None,
    )
}

/// [`click`] as the two events a script hands the loop.
fn clicking(column: u16, row: u16) -> [Typed; 2] {
    [
        Typed::Mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row)),
        Typed::Mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row)),
    ]
}

/// Draws the screen so the mouse map is a real frame's.
///
/// Twelve rows by default: two of header, one blank, and the list from row
/// three, a heading and then its agents.
fn a_frame(screen: &mut Screen) {
    a_frame_of(screen, (60, 12));
}

/// [`a_frame`] at a given size.
fn a_frame_of(screen: &mut Screen, size: (u16, u16)) {
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
    terminal.draw(|frame| paint::draw(frame, screen)).unwrap();
}

#[test]
fn mouse_click_selects_the_row_and_toggles_the_heading_under_the_pointer() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    a_frame(&mut screen);
    assert_eq!(screen.list.selected().unwrap().id(), "done-a1b");

    // The second agent's row, two under the heading on row 3. The cursor lands
    // before the click tries to reach the window; with no record to resume, the
    // reach itself is left to the e2e tests.
    let _ = click(&mut screen, 5, 5, root.path());
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");

    // A click on the heading shuts the group, and another opens it.
    a_frame(&mut screen);
    click(&mut screen, 5, 3, root.path()).unwrap();
    assert_eq!(
        screen.list.items().len(),
        1,
        "the rows are behind the count"
    );
    a_frame(&mut screen);
    click(&mut screen, 5, 3, root.path()).unwrap();
    assert_eq!(screen.list.items().len(), 3);
}

#[test]
fn mouse_click_on_the_fold_unfolds_it_and_elsewhere_does_nothing() {
    let root = TempDir::new().unwrap();
    // Two finished agents past the fold: a heading, the rows the group shows
    // and the fold under them, on a screen tall enough for all of it.
    let height = (rows::FOLD_AT + 10) as u16;
    let mut screen = watching(
        (0..rows::FOLD_AT + 2)
            .map(|n| finished_saying(&format!("done-{n:02}"), "an answer"))
            .collect(),
    );
    a_frame_of(&mut screen, (60, height));
    assert_eq!(
        screen.list.items().len(),
        rows::FOLD_AT + 2,
        "a heading, the drawn rows and the fold"
    );

    // The fold is the row under the drawn agents; the list starts on row 3.
    click(&mut screen, 5, (rows::FOLD_AT + 4) as u16, root.path()).unwrap();
    assert_eq!(
        screen.list.items().len(),
        rows::FOLD_AT + 3,
        "the fold gave its rows back"
    );

    // A click past the end of the list lands on nothing and moves nothing.
    let before = screen.list.selected().unwrap().id().to_string();
    a_frame_of(&mut screen, (60, height));
    click(&mut screen, 5, (rows::FOLD_AT + 7) as u16, root.path()).unwrap();
    assert_eq!(screen.list.selected().unwrap().id(), before);
}

#[test]
fn mouse_click_on_a_row_reaches_for_the_agents_window_like_enter() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);

    // On the 50x10 screen the heading is row 2 and the agent row 3. The click
    // lands the cursor and then reaches for the window as enter does; the agent
    // has no session to resume, and that refusal shows the click got that far.
    let script = clicking(5, 3)
        .into_iter()
        .chain([Typed::Key(KeyEvent::from(KeyCode::Char('q')))])
        .collect();
    let (code, drawn) = driving(root.path(), script);
    assert_eq!(code, exit::OK);
    assert!(
        drawn.contains("no session was ever recorded"),
        "the click read the row as enter does:\n{drawn}"
    );
}

#[test]
fn mouse_hover_tints_a_name_and_moves_no_cursor() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    a_frame(&mut screen);
    let resting = |screen: &mut Screen, column, row| {
        screen
            .moused(mouse(MouseEventKind::Moved, column, row), root.path(), None)
            .unwrap();
    };

    resting(&mut screen, 5, 5);
    assert_eq!(screen.hover, Some(2), "the second agent's line is hovered");
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "done-a1b",
        "and the keyboard's cursor did not move"
    );

    // A heading takes the hover like a row; off the list nothing is tinted.
    resting(&mut screen, 5, 3);
    assert_eq!(screen.hover, Some(0), "the heading over them is hovered");
    resting(&mut screen, 5, 0);
    assert_eq!(screen.hover, None);
}

#[test]
fn ctrl_x_is_read_on_the_row_or_heading_under_the_pointer() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    a_frame(&mut screen);
    assert_eq!(screen.list.selected().unwrap().id(), "done-a1b");

    // With the pointer on the other row, the press lands the cursor there and
    // arms that row.
    screen
        .moused(mouse(MouseEventKind::Moved, 5, 5), root.path(), None)
        .unwrap();
    screen
        .pressed(ctrl('x'), root.path(), &config, None)
        .unwrap();
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");
    assert_eq!(screen.armed(), ["done-b2c"]);

    // On the heading, the press belongs to the group.
    screen
        .moused(mouse(MouseEventKind::Moved, 5, 3), root.path(), None)
        .unwrap();
    screen
        .pressed(ctrl('x'), root.path(), &config, None)
        .unwrap();
    assert!(screen.list.on_heading());
    assert_eq!(screen.armed().len(), 2, "{:?}", screen.armed());
}

#[test]
fn space_opens_the_card_on_the_row_under_the_pointer() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    // Tall enough that opening the card does not move either row, so the
    // pointer still names the same row.
    a_frame_of(&mut screen, (60, 24));
    assert_eq!(screen.list.selected().unwrap().id(), "done-a1b");

    let resting = |screen: &mut Screen, row| {
        screen
            .moused(mouse(MouseEventKind::Moved, 5, row), root.path(), None)
            .unwrap();
    };
    let press = |screen: &mut Screen, code| {
        screen
            .pressed(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
        a_frame_of(screen, (60, 24));
    };
    let carded = |screen: &Screen| screen.card.as_ref().map(|card| card.id.clone());

    // The pointer rests on the row the cursor is not on: space moves the cursor
    // there and opens that row's card.
    resting(&mut screen, 5);
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");
    assert_eq!(carded(&screen).as_deref(), Some("done-b2c"));

    // The pointer moves to the other row with the card up: space opens that row
    // instead of closing the card.
    resting(&mut screen, 4);
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(screen.list.selected().unwrap().id(), "done-a1b");
    assert_eq!(carded(&screen).as_deref(), Some("done-a1b"));

    // With the pointer on the cursor's row, space toggles the card.
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(carded(&screen), None, "space closed the card");

    // With no pointer on the list, space acts on the cursor as before.
    resting(&mut screen, 5);
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");
    assert_eq!(carded(&screen).as_deref(), Some("done-b2c"));
    resting(&mut screen, 0);
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(carded(&screen), None);
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(carded(&screen).as_deref(), Some("done-b2c"));
}

#[test]
fn a_key_press_retires_the_pointer_and_the_next_movement_brings_it_back() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
        finished_saying("done-c3d", "the third answer"),
    ]);
    a_frame_of(&mut screen, (60, 24));

    let resting = |screen: &mut Screen, row| {
        screen
            .moused(mouse(MouseEventKind::Moved, 5, row), root.path(), None)
            .unwrap();
    };
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
        a_frame_of(screen, (60, 24));
    };
    let carded = |screen: &Screen| screen.card.as_ref().map(|card| card.id.clone());

    // The pointer is parked on the last row and the keyboard takes over: `j`
    // moves the cursor and the pointer stops counting.
    resting(&mut screen, 6);
    assert_eq!(screen.hover, Some(3), "the third agent's line is hovered");
    press(&mut screen, KeyCode::Char('j'));
    assert_eq!(screen.hover, None, "the key press retired the pointer");
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");

    // So space opens the cursor's row, not the one under the idle pointer.
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");
    assert_eq!(carded(&screen).as_deref(), Some("done-b2c"));

    // The next movement makes the pointer count again.
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(carded(&screen), None, "space closed the card");
    resting(&mut screen, 6);
    assert_eq!(screen.hover, Some(3));
    press(&mut screen, KeyCode::Char(' '));
    assert_eq!(screen.list.selected().unwrap().id(), "done-c3d");
    assert_eq!(carded(&screen).as_deref(), Some("done-c3d"));
}

#[test]
fn going_into_an_agent_retires_the_pointer() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    a_frame(&mut screen);
    screen
        .moused(mouse(MouseEventKind::Moved, 5, 5), root.path(), None)
        .unwrap();
    assert_eq!(screen.hover, Some(2));

    // Mouse capture was off while the terminal was lent, so the hover line is
    // stale.
    screen.went_into("done-b2c".to_string());
    assert_eq!(screen.lent.as_deref(), Some("done-b2c"));
    assert_eq!(screen.hover, None);
}

#[test]
fn going_into_an_agent_puts_away_the_card_it_was_gone_into_from() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![finished_saying("done-a1b", "the answer")]);
    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    assert!(screen.card.is_some(), "the card somebody went in from");

    // The card showed the pane from outside, and the user is now inside it;
    // left up, it would be a stale picture on return with ctrl+z.
    screen.went_into("done-a1b".to_string());
    assert!(screen.card.is_none(), "and it went in with them");
    assert!(matches!(screen.look, Look::Away));
    assert!(matches!(screen.mode, Mode::List), "and the line with it");
}

#[test]
fn mouse_moves_that_queued_up_cost_one_frame_between_them() {
    let root = TempDir::new().unwrap();
    finished(root.path(), "first-a1b", "wrote the parser", 60);

    // Pointer movements arrive one per cell, faster than the view draws. Only
    // where the pointer stops matters, so a run of them costs one frame instead
    // of redrawing the list for each.
    let mut storm: Vec<Typed> = (3..9)
        .map(|row| Typed::Mouse(mouse(MouseEventKind::Moved, 5, row)))
        .collect();
    storm.push(Typed::Key(KeyEvent::from(KeyCode::Char('q'))));

    let quiet = frames(
        root.path(),
        vec![Typed::Key(KeyEvent::from(KeyCode::Char('q')))],
    );
    assert_eq!(
        frames(root.path(), storm),
        quiet + 1,
        "six movements cost the one frame the pointer's resting place is drawn on"
    );
}

#[test]
fn mouse_moves_collapse_to_where_the_pointer_came_to_rest() {
    // The run is read to its end and whatever ended it is handed back, so a key
    // queued behind the movements is not lost.
    let mut keys = Script(
        vec![
            Typed::Mouse(mouse(MouseEventKind::Moved, 5, 6)),
            Typed::Mouse(mouse(MouseEventKind::Moved, 5, 7)),
            Typed::Key(KeyEvent::from(KeyCode::Char('q'))),
        ]
        .into_iter(),
    );

    let (rest, ended) = at_rest(&mut keys, mouse(MouseEventKind::Moved, 5, 5));
    assert_eq!((rest.column, rest.row), (5, 7), "the last of the run");
    assert!(
        matches!(ended, Some(Typed::Key(key)) if key.code == KeyCode::Char('q')),
        "and the key that ended it is kept for the next pass"
    );

    // A click is not a movement, so nothing is read past it.
    let mut alone = Script(vec![Typed::Key(KeyEvent::from(KeyCode::Char('n')))].into_iter());
    let click = mouse(MouseEventKind::Down(MouseButton::Left), 5, 5);
    let (pressed, ended) = at_rest(&mut alone, click);
    assert_eq!(pressed.kind, MouseEventKind::Down(MouseButton::Left));
    assert!(
        ended.is_none(),
        "and nothing behind it was taken off the queue"
    );
}

#[test]
fn a_late_shade_reply_never_reaches_the_list_and_typing_still_does() {
    // A terminal slower than the shade probe answers into the key loop, where
    // its `:` and `/` would open a line. The reply is dropped whole; the same
    // keys typed by hand, and alt and `]` alone, get through.
    let mut script: Vec<Typed> = vec![Typed::Key(alt(']'))];
    script.extend(
        word("11;rgb:ffff/ffff/ffff")
            .into_iter()
            .map(|code| Typed::Key(code.into())),
    );
    script.push(Typed::Key(ctrl('g')));
    script.extend([':', '/'].map(|c| Typed::Key(KeyCode::Char(c).into())));
    script.push(Typed::Key(alt(']')));
    script.push(Typed::Nothing);
    let mut keys = Unanswered::new(Script(script.into_iter()));

    let mut reached = Vec::new();
    loop {
        match keys.next(Duration::ZERO) {
            Typed::Key(key) => reached.push(key),
            Typed::Gone => break,
            _ => {}
        }
    }
    let typed: Vec<KeyEvent> = vec![
        KeyCode::Char(':').into(),
        KeyCode::Char('/').into(),
        alt(']'),
    ];
    assert_eq!(reached, typed);
}

/// More finished agents than the band holds, so the list scrolls and the
/// cursor can leave the screen.
fn a_tall_wall() -> Vec<View> {
    (0..20)
        .map(|n| finished_saying(&format!("row-{n:02}-a1b"), "did what it was asked"))
        .collect()
}

#[test]
fn mouse_wheel_scrolls_the_wall_and_leaves_the_cursor_where_it_was() {
    let root = TempDir::new().unwrap();
    let mut screen = watching(a_tall_wall());
    a_frame(&mut screen);
    let on = screen.list.cursor();

    // Three wheel steps with the pointer on a row: the window moves three rows,
    // the cursor stays (now above the band), and the hover goes with the rows
    // that moved.
    screen
        .moused(mouse(MouseEventKind::Moved, 5, 5), root.path(), None)
        .unwrap();
    assert!(screen.hover.is_some(), "the pointer is on a row");
    for _ in 0..3 {
        screen
            .moused(mouse(MouseEventKind::ScrollDown, 5, 5), root.path(), None)
            .unwrap();
    }
    a_frame(&mut screen);
    assert_eq!(screen.wall.top.get(), 3, "three lines, three rows");
    assert_eq!(screen.list.cursor(), on, "and no cursor moved");
    assert_eq!(screen.hover, None, "the rows moved under the pointer");

    // A click lands on the row the frame drew, not the unscrolled one: the band
    // starts on row 3, which is the window's top item. Reaching the window
    // after the landing fails for lack of a record, which is not under test
    // here.
    let _ = click(&mut screen, 5, 3, root.path());
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "row-02-a1b",
        "the third item, which is what the first drawn line says"
    );
    a_frame(&mut screen);
    assert_eq!(
        screen.wall.top.get(),
        3,
        "and the cursor it landed on is on the screen, so nothing followed"
    );

    // Wheel-up past the top stops at the top.
    for _ in 0..9 {
        screen
            .moused(mouse(MouseEventKind::ScrollUp, 5, 5), root.path(), None)
            .unwrap();
    }
    a_frame(&mut screen);
    assert_eq!(screen.wall.top.get(), 0);
}

#[test]
fn keys_move_the_cursor_and_the_window_comes_after_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(a_tall_wall());
    let items = screen.list.items().len();
    let press = |screen: &mut Screen, code| {
        screen
            .act(KeyEvent::from(code), root.path(), &config, None)
            .unwrap();
    };
    a_frame(&mut screen);

    // G goes to the end of the list and the window to its last page, so the
    // wheel cannot go further down.
    press(&mut screen, KeyCode::Char('G'));
    a_frame(&mut screen);
    assert_eq!(screen.list.cursor(), items - 1);
    let bottom = screen.wall.top.get();
    assert!(bottom > 0, "the end of this list is off the first page");
    screen.scrolled(false);
    a_frame(&mut screen);
    assert_eq!(screen.wall.top.get(), bottom, "the last page is the last");

    // gg goes to the top, window included.
    press(&mut screen, KeyCode::Char('g'));
    press(&mut screen, KeyCode::Char('g'));
    a_frame(&mut screen);
    assert_eq!(screen.list.cursor(), 0);
    assert_eq!(screen.wall.top.get(), 0);

    // The band height follows from the last page: `j` one line past the last
    // drawn row moves the window by one, and earlier moves do not move it.
    let visible = items - bottom;
    while screen.list.cursor() < visible - 1 {
        press(&mut screen, KeyCode::Char('j'));
    }
    a_frame(&mut screen);
    assert_eq!(screen.wall.top.get(), 0, "the last drawn line is still one");
    press(&mut screen, KeyCode::Char('j'));
    a_frame(&mut screen);
    assert_eq!(screen.list.cursor(), visible);
    assert_eq!(screen.wall.top.get(), 1, "one row of cursor, one of window");

    // `k` back over the first drawn line is the same the other way: one row
    // past it moves the window by one.
    while screen.list.cursor() > 1 {
        press(&mut screen, KeyCode::Char('k'));
    }
    a_frame(&mut screen);
    assert_eq!(
        screen.wall.top.get(),
        1,
        "the first drawn line is still one"
    );
    press(&mut screen, KeyCode::Char('k'));
    a_frame(&mut screen);
    assert_eq!(screen.list.cursor(), 0);
    assert_eq!(screen.wall.top.get(), 0);
}

#[test]
fn mouse_wheel_pages_the_card_under_the_pointer_and_scrolls_the_list_beside_it() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let long: String = (0..40).map(|n| format!("said {n}\n")).collect();
    let mut screen = watching(vec![
        finished_saying("done-a1b", &long),
        finished_saying("done-b2c", "the second answer"),
    ]);
    // Twenty rows, so the card opened below leaves both rows visible.
    a_frame_of(&mut screen, (60, 20));
    let wheel = |screen: &mut Screen, kind, column, row| {
        screen
            .moused(mouse(kind, column, row), root.path(), None)
            .unwrap();
    };

    // No card: the wheel scrolls the window, and two rows in a twenty-row band
    // have nowhere to scroll, so the cursor stays put.
    wheel(&mut screen, MouseEventKind::ScrollDown, 5, 5);
    a_frame_of(&mut screen, (60, 20));
    assert_eq!(screen.list.selected().unwrap().id(), "done-a1b");
    assert_eq!(screen.wall.top.get(), 0);

    // With a card over the bottom of the band, the wheel pages it when the
    // pointer is over it and leaves it alone otherwise.
    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    a_frame_of(&mut screen, (60, 20));
    wheel(&mut screen, MouseEventKind::ScrollDown, 5, 9);
    assert!(
        screen.scroll.away.get() > 0,
        "wheel-down over the card read on into the answer"
    );
    wheel(&mut screen, MouseEventKind::ScrollUp, 5, 9);
    assert_eq!(screen.scroll.away.get(), 0, "and wheel-up came back");

    a_frame_of(&mut screen, (60, 20));
    wheel(&mut screen, MouseEventKind::ScrollDown, 5, 4);
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "done-a1b",
        "over the list the wheel moves no cursor"
    );
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-a1b"),
        "and takes the card nowhere"
    );
    assert_eq!(screen.scroll.away.get(), 0, "nor pages it");
}

#[test]
fn mouse_clicks_are_the_lists_alone_while_a_line_is_being_typed() {
    let root = TempDir::new().unwrap();
    let config = Config::default();
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    screen.mode = Mode::Typing(Composer::new(Asking::Task));
    a_frame(&mut screen);

    click(&mut screen, 5, 5, root.path()).unwrap();
    screen
        .moused(mouse(MouseEventKind::ScrollDown, 5, 5), root.path(), None)
        .unwrap();
    assert_eq!(
        screen.list.selected().unwrap().id(),
        "done-a1b",
        "a line being typed keeps the keys, and the mouse with them"
    );

    // The card's line takes no pointer, so with a card open a click still lands
    // on its row and the card follows the cursor there.
    let mut screen = watching(vec![
        finished_saying("done-a1b", "the first answer"),
        finished_saying("done-b2c", "the second answer"),
    ]);
    screen
        .act(
            KeyEvent::from(KeyCode::Char(' ')),
            root.path(),
            &config,
            None,
        )
        .unwrap();
    assert!(screen.answering().is_some(), "the card's line is up");
    a_frame(&mut screen);
    let _ = click(&mut screen, 5, 5, root.path());
    assert_eq!(screen.list.selected().unwrap().id(), "done-b2c");
    assert_eq!(
        screen.card.as_ref().map(|card| card.id.as_str()),
        Some("done-b2c"),
        "with the card on the agent that was clicked"
    );
}

#[test]
fn the_keys_are_on_the_screen_for_the_asking() {
    let root = TempDir::new().unwrap();
    let (_, screen) = held(root.path(), &[KeyCode::Char('?'), KeyCode::Char('q')]);
    // The key opens the overlay and the overlay says how to leave it. How much
    // it fits on a short terminal is tested with the overlay.
    assert!(screen.contains("↑ ↓"), "{screen}");
    assert!(screen.contains("any key goes back"), "{screen}");
}

/// A terminal query that answers `answer` and counts how often it is asked.
fn answering(answer: Option<&str>, asked: &AtomicUsize) -> impl FnOnce() -> Option<String> {
    move || {
        asked.fetch_add(1, Ordering::Relaxed);
        answer.map(str::to_string)
    }
}

#[test]
fn theme_named_by_hand_still_asks_once_and_keeps_both_colours() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("agents");
    let asked = AtomicUsize::new(0);
    let named = Painting::named(
        "solarized",
        answering(
            Some("\x1b]10;rgb:e5e5/e0e0/dcdc\x1b\\\x1b]11;rgb:2121/1b1b/1b1b\x1b\\"),
            &asked,
        ),
        &root,
    );
    assert_eq!(named, "solarized", "a name somebody wrote is not overruled");
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("background")).unwrap(),
        "fg=#e5e0dc,bg=#211b1b\n"
    );
}

#[test]
fn theme_a_terminal_answering_the_background_alone_keeps_it_alone() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("agents");
    let asked = AtomicUsize::new(0);
    Painting::named(
        "default",
        answering(Some("\x1b]11;rgb:2323/1f1f/1f1f\x07"), &asked),
        &root,
    );
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("background")).unwrap(),
        "bg=#231f1f\n"
    );
}

#[test]
fn theme_auto_reads_its_shade_off_the_same_answer() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("agents");
    let asked = AtomicUsize::new(0);
    let named = Painting::named(
        crate::theme::AUTO,
        answering(Some("\x1b]11;rgb:ffff/ffff/ffff\x1b\\"), &asked),
        &root,
    );
    assert_eq!(named, "light");
    assert_eq!(asked.load(Ordering::Relaxed), 1, "one question for both");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("background")).unwrap(),
        "bg=#ffffff\n"
    );
}

#[test]
fn theme_a_silent_terminal_leaves_the_kept_colours_as_they_were() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("agents");
    let kept = dir.path().join("background");
    std::fs::write(&kept, "#010203\n").unwrap();
    // No answer, the empty answer `asked` returns on timeout, and a foreground
    // with no background.
    for silence in [None, Some(""), Some("\x1b]10;rgb:ffff/ffff/ffff\x07")] {
        let asked = AtomicUsize::new(0);
        let named = Painting::named("default", answering(silence, &asked), &root);
        assert_eq!(named, "default");
        assert_eq!(asked.load(Ordering::Relaxed), 1);
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "#010203\n");
    }
}
