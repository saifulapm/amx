//! `amx doctor` — what amx needs from this machine, and what is missing.
//!
//! Six things have to be true before an agent can run: a tmux new enough to
//! address panes by id, a vendor command to run, a config amx can read, amx's
//! hooks wired into the vendor's settings, a state root amx can keep an agent
//! in, and no agent already stopped at a screen the vendor puts in front of
//! the work. Each check that fails says what to do about it, because a check
//! that only says "no" leaves somebody guessing at a machine they thought was
//! fine.
//!
//! What two of them are worth depends on the vendor, and the table is what
//! says: one that reports nothing has no wiring to be missing, and one with no
//! folder-trust screen has no question amx could offer to answer. A check that
//! asked for a repair nobody can make would send somebody looking for a fault
//! in their own machine.
//!
//! A seventh is asked only where there is something to ask it of. When a tmux
//! server is already running, and the machine can say where a process is
//! standing, doctor checks that the directory that server is standing in still
//! exists. A server holds the directory it was started in for as long as it
//! lives, and once that goes, every pane it forks starts somewhere that is not
//! there and dies at once. No server yet is not a fault, and neither is a
//! platform amx cannot ask, so both go unsaid rather than answered green.
//!
//! `--fix` does the one repair amx can make safely: wiring the hooks, after
//! asking.

use anyhow::Result;
use std::ffi::OsStr;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::derive::View;
use crate::store::Phase;
use crate::vendor::{Capability, Vendor};
use crate::{derive, exit, install, registry, rules, store, tmux};

/// One thing amx looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    /// What amx found, said plainly.
    pub found: String,
    /// What to do about it, when there is something to do.
    pub remedy: Option<String>,
}

impl Check {
    fn ok(name: &'static str, found: impl Into<String>) -> Check {
        Check {
            name,
            found: found.into(),
            remedy: None,
        }
    }

    fn wrong(name: &'static str, found: impl Into<String>, remedy: impl Into<String>) -> Check {
        Check {
            name,
            found: found.into(),
            remedy: Some(remedy.into()),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.remedy.is_none()
    }
}

/// What amx found on the machine, gathered before anything is judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings {
    /// The installed tmux, or `None` when there is none.
    pub tmux: Option<(u32, u32)>,
    /// The configured vendor command, and where it resolved to.
    pub vendor: String,
    pub vendor_path: Option<PathBuf>,
    /// The config file, and anything amx had to say about reading it.
    pub config: PathBuf,
    pub config_warnings: Vec<String>,
    /// The vendor's settings, which of amx's events they wire, and why they
    /// could not be read if they could not.
    pub settings: PathBuf,
    pub wired: Vec<String>,
    pub settings_error: Option<String>,
    /// The hook command this amx would install.
    pub command: String,
    /// Where every agent's record is kept, and why amx cannot use it when it
    /// cannot.
    pub state_root: PathBuf,
    pub state_error: Option<String>,
    /// The agents that never got past the vendor's own setup.
    pub parked: Vec<Parked>,
    /// The tmux server amx would put an agent on, when one is already running
    /// and this machine can say where it is standing.
    pub server: Option<StandingServer>,
}

/// The server amx would use, and where its own process is standing.
///
/// The socket comes along because the remedy is a command line, and a restart
/// aimed at the wrong server is worse than no advice at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingServer {
    pub socket: tmux::Socket,
    pub cwd: tmux::ServerCwd,
}

/// An agent stopped at a screen the vendor puts in front of the work, and
/// which screen it is. Nobody but the person at the keyboard can get it past
/// one, so this is a check that names names rather than one amx can fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    pub id: String,
    pub screen: Setup,
}

/// What is in the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// The folder-trust question, which amx has a measured rule for.
    Trust,
    /// A screen no rule claims, under a record that has never left `starting`.
    Unread,
}

impl Setup {
    /// What is in the way, worded for the line that names the agent it stopped.
    ///
    /// Whose question it is comes off the table rather than out of a string
    /// here, because the screen is the vendor's. Where no vendor in the table
    /// draws one, the question is still described and simply not named.
    fn says(self, trusting: Option<&Vendor>) -> String {
        match (self, trusting) {
            (Setup::Trust, Some(vendor)) => format!("{}'s folder-trust question", vendor.name),
            (Setup::Trust, None) => "a folder-trust question".to_string(),
            (Setup::Unread, _) => "an opening screen amx has no rule for".to_string(),
        }
    }
}

/// Judge what was found.
///
/// Six of these are asked on every machine. The seventh is asked only where
/// there is something to ask it of: a tmux server already running, on a
/// platform that can say where a process is standing.
pub fn report(found: &Findings) -> Vec<Check> {
    let mut checks = vec![
        tmux_check(found),
        vendor_check(found),
        config_check(found),
        wiring_check(found, registry::entry(&found.vendor)),
        state_check(found),
    ];
    checks.extend(server_check(found));
    checks.push(setup_check(found, trusting()));
    checks
}

/// The vendor amx would answer a folder-trust screen for, when the table has
/// one.
fn trusting() -> Option<&'static Vendor> {
    registry::entries()
        .iter()
        .find(|vendor| vendor.can(Capability::Trust))
}

fn tmux_check(found: &Findings) -> Check {
    let (want_major, want_minor) = tmux::MINIMUM_VERSION;
    match found.tmux {
        Some((major, minor)) if (major, minor) >= tmux::MINIMUM_VERSION => {
            Check::ok("tmux", format!("{major}.{minor}"))
        }
        Some((major, minor)) => Check::wrong(
            "tmux",
            format!("{major}.{minor}"),
            format!(
                "amx addresses panes by id, which needs tmux {want_major}.{want_minor} or newer"
            ),
        ),
        None => Check::wrong(
            "tmux",
            "not installed",
            format!("install tmux {want_major}.{want_minor} or newer"),
        ),
    }
}

fn vendor_check(found: &Findings) -> Check {
    match &found.vendor_path {
        Some(path) => Check::ok("agent", format!("{} at {}", found.vendor, path.display())),
        None => Check::wrong(
            "agent",
            format!("`{}` is not on the PATH", program(&found.vendor)),
            format!(
                "install it, or set `agent` in {} to a command that is",
                found.config.display()
            ),
        ),
    }
}

fn config_check(found: &Findings) -> Check {
    if found.config_warnings.is_empty() {
        return Check::ok("config", found.config.display().to_string());
    }
    Check::wrong(
        "config",
        found.config_warnings.join("; "),
        format!("edit {}", found.config.display()),
    )
}

/// Whether amx's hooks are where this vendor's reports would come from.
///
/// A vendor that reports nothing is not a machine with something missing from
/// it: there are no entries to write, nothing for `--fix` to do, and what amx
/// has instead is the pane. A command amx has no entry for is measured neither
/// way and is judged as claude is — a wrapper somebody wrote around it reports
/// through the same settings file.
fn wiring_check(found: &Findings, vendor: Option<&Vendor>) -> Check {
    if let Some(vendor) = vendor.filter(|vendor| !vendor.can(Capability::Hooks)) {
        return Check::ok(
            "hooks",
            format!(
                "{} reports nothing amx can wire, so its pane is what amx reads",
                vendor.name
            ),
        );
    }

    if let Some(why) = &found.settings_error {
        // amx does not write settings it cannot read, so there is nothing
        // `--fix` can do here that would not risk the person's own file.
        return Check::wrong(
            "hooks",
            format!("{} cannot be read: {why}", found.settings.display()),
            format!("repair {} by hand", found.settings.display()),
        );
    }

    let missing: Vec<&str> = install::EVENTS
        .iter()
        .filter(|event| !found.wired.iter().any(|wired| wired == *event))
        .copied()
        .collect();

    if missing.is_empty() {
        return Check::ok(
            "hooks",
            format!(
                "all {} wired in {}",
                install::EVENTS.len(),
                found.settings.display()
            ),
        );
    }
    let settings = found.settings.display();
    let what = if missing.len() == install::EVENTS.len() {
        format!("none wired in {settings}")
    } else {
        format!("{} not wired in {settings}", missing.join(", "))
    };
    Check::wrong("hooks", what, "run `amx doctor --fix`")
}

/// Whether the server amx would use is still standing somewhere that exists.
///
/// A tmux server keeps the directory it was started in for as long as it
/// lives. Delete that directory and the server carries on holding it: every
/// pane forked afterwards starts in a place that is not there, and the vendor
/// exits before it draws a frame. From the outside that looks like an agent
/// that failed in under a second having said nothing, which is a long way from
/// the cause.
///
/// `None` where there is nothing to ask — no server yet, or no way to look —
/// because a check nobody could act on is the kind that sends a person hunting
/// a fault in their own machine.
fn server_check(found: &Findings) -> Option<Check> {
    let standing = found.server.as_ref()?;
    let (pid, where_) = (standing.cwd.pid, standing.cwd.path.display());
    Some(if standing.cwd.stale {
        Check::wrong(
            "server",
            format!("tmux server {pid}'s directory is gone: {where_}"),
            format!(
                "every pane it starts inherits that and dies at once; \
                 restart it: tmux {} kill-server",
                address(&standing.socket)
            ),
        )
    } else {
        Check::ok("server", format!("tmux server {pid} in {where_}"))
    })
}

/// How a tmux command line names this socket.
fn address(socket: &tmux::Socket) -> String {
    match socket {
        tmux::Socket::Name(name) => format!("-L {name}"),
        tmux::Socket::Path(path) => format!("-S {}", path.display()),
    }
}

fn state_check(found: &Findings) -> Check {
    match &found.state_error {
        None => Check::ok("state", found.state_root.display().to_string()),
        Some(why) => Check::wrong(
            "state",
            why.clone(),
            "amx keeps every agent there, so until that is fixed it has nowhere to put one",
        ),
    }
}

fn setup_check(found: &Findings, trusting: Option<&Vendor>) -> Check {
    let Some(first) = found.parked.first() else {
        return Check::ok("setup", "no agent is stopped at the vendor's own setup");
    };

    let each: Vec<String> = found
        .parked
        .iter()
        .map(|agent| format!("{} at {}", agent.id, agent.screen.says(trusting)))
        .collect();
    let what = match each.as_slice() {
        [one] => one.clone(),
        many => format!("{} agents are stopped: {}", many.len(), many.join(", ")),
    };

    let remedy = match (first.screen, trusting) {
        // The one screen amx can take off the person's hands, once they have
        // said so: the config key is the consent the write stands behind. Only
        // offered for a vendor whose screen amx knows how to answer, because
        // the key does nothing for any other.
        (Setup::Trust, Some(_)) => format!(
            "answer it yourself: amx attach {}, or set trust = true in the \
             config and amx answers it for the trees it cuts",
            first.id
        ),
        _ => format!("answer it yourself: amx attach {}", first.id),
    };
    Check::wrong("setup", what, remedy)
}

/// Print the checks, offer the one repair amx can make, and answer with an
/// exit code: zero when there is nothing left to do.
pub fn run(
    found: &Findings,
    fix: bool,
    now: u64,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    let mut checks = report(found);
    for check in &checks {
        match &check.remedy {
            None => writeln!(out, "  ok  {:<7} {}", check.name, check.found)?,
            Some(remedy) => writeln!(
                out,
                "  no  {:<7} {}\n         {remedy}",
                check.name, check.found
            )?,
        }
    }

    if fix && fixable(&checks) {
        writeln!(
            out,
            "\n{}",
            install::consent_line(&found.settings, found.settings.exists())
        )?;
        write!(out, "go ahead? [y/N] ")?;
        out.flush()?;

        let mut answer = String::new();
        input.read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("y") {
            let report = install::install(&found.settings, &found.command, now)?;
            writeln!(out, "wired the hooks into {}", report.path.display())?;
            if let Some(backup) = report.backup {
                writeln!(out, "the file as it was is at {}", backup.display())?;
            }
            // Judge again: the machine is not what it was a moment ago.
            let (wired, settings_error) = wiring(&found.settings, &found.command);
            checks = report_with_wiring(found, wired, settings_error);
        } else {
            writeln!(out, "left {} alone", found.settings.display())?;
        }
    }

    Ok(if checks.iter().all(Check::is_ok) {
        exit::OK
    } else {
        exit::FAILURE
    })
}

/// Whether the one repair amx can make is the repair that is needed.
fn fixable(checks: &[Check]) -> bool {
    checks
        .iter()
        .any(|check| check.name == "hooks" && !check.is_ok())
        && checks.iter().all(|check| {
            check.name != "hooks" || check.remedy.as_deref() == Some("run `amx doctor --fix`")
        })
}

/// The same judgement, with the wiring as it is now rather than as it was.
fn report_with_wiring(
    found: &Findings,
    wired: Vec<String>,
    settings_error: Option<String>,
) -> Vec<Check> {
    report(&Findings {
        wired,
        settings_error,
        ..found.clone()
    })
}

/// Look at the machine.
pub fn gather(config: &Config) -> Result<Findings> {
    let settings = install::settings_file()?;
    let command = install::hook_command(&std::env::current_exe()?);
    let (wired, settings_error) = wiring(&settings, &command);
    let (_, config_warnings) = crate::config::load();
    let state_root = crate::paths::state_root()?;

    Ok(Findings {
        tmux: tmux::version().ok(),
        vendor: config.agent.clone(),
        vendor_path: on_path(program(&config.agent), std::env::var_os("PATH").as_deref()),
        config: crate::paths::config_file()?,
        config_warnings,
        settings,
        wired,
        settings_error,
        command,
        state_error: usable(&state_root),
        parked: parked(
            // A state root amx cannot read has no agents to report on, and the
            // check above is where that is said. Here it means none were found.
            &derive::views(&state_root, rules::bundled(), store::now()).unwrap_or_default(),
        ),
        state_root,
        server: standing_server(),
    })
}

/// The server amx would start an agent on, when one is already running.
///
/// The same resolution a spawn does, so doctor judges the server that would
/// actually be used rather than whichever one is easiest to find.
fn standing_server() -> Option<StandingServer> {
    let server = crate::spawn::server().ok()?;
    Some(StandingServer {
        socket: server.socket().clone(),
        cwd: server.cwd()?,
    })
}

/// The agents stopped at a screen the vendor draws before it will do anything
/// else, and which screen each of them is at.
///
/// Two shapes, because amx can name one of them and can only describe the
/// other. The folder-trust question has a rule measured off a live vendor, so
/// an agent stopped there is named for what it is. The login prompt has no
/// rule and cannot honestly be given one from here: measuring it means logging
/// a real claude out, and a string nobody read off a running vendor is exactly
/// what the ruleset's anchor law forbids.
///
/// So the second shape is described rather than named: a record that has never
/// left `starting` — the vendor has begun no turn, and `SessionStart` alone
/// does not move it — under a screen no rule claims. A vendor that changed its
/// opening screen reads the same way, and so does one still drawing its first
/// frame once the record has gone stale enough for the pane to be asked. That
/// is the cost of describing it, and it is the cheaper mistake: the remedy is
/// to attach and look, which is what a person would do anyway.
fn parked(views: &[View]) -> Vec<Parked> {
    views
        .iter()
        .filter_map(|view| {
            let screen = if view.phase() == Phase::Waiting
                && view.verdict.rule.as_deref() == Some("folder_trust")
            {
                Setup::Trust
            } else if view.state.state == Phase::Starting && view.phase() == Phase::Unknown {
                Setup::Unread
            } else {
                return None;
            };
            Some(Parked {
                id: view.id().to_string(),
                screen,
            })
        })
        .collect()
}

/// Why amx cannot use `root`, when it cannot.
///
/// An agent's directory is made with all of its missing parents at once, so
/// the directory that has to take that write is the nearest ancestor already
/// on disk: the root itself once amx has run here before, the directory above
/// it on a machine where it has not. The root is read as well as written once
/// it exists, because listing it is how every reader finds the agents.
///
/// The failure this exists for is quiet: a root that cannot be made and a
/// machine that has simply never run an agent both list as no agents at all,
/// and the difference only shows up as a spawn failing later.
fn usable(root: &Path) -> Option<String> {
    let mut dir = root;
    while !dir.exists() {
        dir = dir.parent()?;
    }

    let mut needs = nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::X_OK;
    if dir == root {
        needs |= nix::unistd::AccessFlags::R_OK;
    }

    let why = nix::unistd::access(dir, needs).err()?.desc();
    Some(if dir == root {
        format!("{} is not readable and writable: {why}", dir.display())
    } else {
        format!(
            "{} would be made in {}, which is not writable: {why}",
            root.display(),
            dir.display()
        )
    })
}

/// Run the verb against the machine.
pub fn from_env(config: &Config, fix: bool) -> Result<i32> {
    let found = gather(config)?;
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    run(&found, fix, crate::store::now(), &mut input, &mut out)
}

/// Which of amx's events the settings wire to `command`, and why they could
/// not be read if they could not.
fn wiring(settings: &Path, command: &str) -> (Vec<String>, Option<String>) {
    match std::fs::read_to_string(settings) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => (install::installed_events(&value, command), None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    }
}

/// The program a configured command runs, without its arguments.
fn program(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

/// Where a program resolves to, given a `PATH`.
fn on_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    // A command with a separator in it is a path, and a shell would not search
    // for it either.
    if program.contains('/') {
        let named = PathBuf::from(program);
        return runnable(&named).then_some(named);
    }

    let path = path?;
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| runnable(candidate))
}

/// Whether this is a file that could be run.
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::second::SECOND;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// The vendor amx would answer a folder-trust screen for.
    fn a_trusting_vendor() -> &'static Vendor {
        trusting().expect("a vendor with a trust screen amx has measured")
    }

    const COMMAND: &str = "/home/dev/.cargo/bin/amx _hook";

    fn healthy() -> Findings {
        Findings {
            tmux: Some((3, 5)),
            vendor: "claude".to_string(),
            vendor_path: Some(PathBuf::from("/usr/local/bin/claude")),
            config: PathBuf::from("/home/dev/.config/amx/config.toml"),
            config_warnings: Vec::new(),
            settings: PathBuf::from("/home/dev/.claude/settings.json"),
            wired: install::EVENTS.iter().map(|e| e.to_string()).collect(),
            settings_error: None,
            command: COMMAND.to_string(),
            state_root: PathBuf::from("/home/dev/.local/state/amx/agents"),
            state_error: None,
            parked: Vec::new(),
            server: None,
        }
    }

    /// An agent as a reader hands it over. The record is deserialised rather
    /// than built field by field, because that is how a real one arrives and a
    /// field added to `Meta` tomorrow should not land here.
    fn view(id: &str, recorded: Phase, seen: Phase, rule: Option<&str>) -> derive::View {
        derive::View {
            meta: serde_json::from_value(serde_json::json!({
                "id": id,
                "task": "fix the login bug",
                "dir": "/srv/app",
                "socket": {"name": "amx"},
                "pane": "%1",
                "created": 1,
            }))
            .expect("the record amx writes at spawn"),
            state: store::State {
                state: recorded,
                ..store::State::default()
            },
            verdict: derive::Verdict {
                phase: seen,
                evidence: derive::Evidence::Screen,
                rule: rule.map(str::to_string),
                age: 40,
                worked: 40,
            },
        }
    }

    /// A directory nobody but its owner may write to is the whole of these
    /// tests, and root is exempt from the permission bits.
    fn not_root() -> bool {
        if nix::unistd::Uid::effective().is_root() {
            eprintln!("skipping: running as root, which every directory lets in");
            return false;
        }
        true
    }

    fn check(found: &Findings, name: &str) -> Check {
        report(found)
            .into_iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("no check named {name}"))
    }

    fn said(found: &Findings, fix: bool) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(found, fix, 1, &mut "".as_bytes(), &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn doctor_says_nothing_is_wrong_when_nothing_is() {
        let checks = report(&healthy());
        assert!(checks.iter().all(Check::is_ok), "{checks:#?}");
        assert_eq!(
            checks.len(),
            6,
            "tmux, the vendor, the config, the hooks, the state root, setup"
        );

        let (code, printed) = said(&healthy(), false);
        assert_eq!(code, exit::OK);
        assert!(printed.contains("tmux"), "{printed}");
    }

    /// A server standing in `path`, deleted or not.
    fn standing(path: &str, stale: bool) -> StandingServer {
        StandingServer {
            socket: crate::tmux::Socket::Name("default".to_string()),
            cwd: crate::tmux::ServerCwd {
                pid: 27267,
                path: PathBuf::from(path),
                stale,
            },
        }
    }

    #[test]
    fn doctor_says_nothing_about_a_server_it_cannot_see() {
        // No server running, or a platform with no way to look: either way
        // there is nothing here to report and nothing to repair.
        let found = healthy();
        assert!(found.server.is_none());
        assert!(
            !report(&found).iter().any(|check| check.name == "server"),
            "no line at all, rather than a green one nobody measured"
        );
        assert_eq!(said(&found, false).0, exit::OK);
    }

    #[test]
    fn doctor_passes_a_server_standing_somewhere_that_is_still_there() {
        let mut found = healthy();
        found.server = Some(standing("/home/dev/src/app", false));

        let server = check(&found, "server");
        assert!(server.is_ok(), "{server:?}");
        assert!(
            server.found.contains("/home/dev/src/app"),
            "{}",
            server.found
        );
        assert_eq!(said(&found, false).0, exit::OK);
    }

    #[test]
    fn doctor_names_a_server_whose_directory_was_deleted() {
        // The failure this check exists for: doctor was green while every
        // agent died in under a second, because nothing asked this.
        let mut found = healthy();
        found.server = Some(standing("/tmp/no-git-test", true));

        let server = check(&found, "server");
        assert!(
            server.found.contains("27267"),
            "which server: {}",
            server.found
        );
        assert!(
            server.found.contains("/tmp/no-git-test"),
            "and where it is stuck: {}",
            server.found
        );

        let remedy = server.remedy.as_deref().unwrap();
        assert!(
            remedy.contains("tmux -L default kill-server"),
            "a command that works on this server, not a general one: {remedy}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn the_restart_names_the_socket_the_server_is_actually_on() {
        // A remedy that said `-L default` for a server reached by path would
        // send somebody to restart the wrong one.
        let mut found = healthy();
        found.server = Some(StandingServer {
            socket: crate::tmux::Socket::Path(PathBuf::from("/run/user/1000/tmux/sock")),
            ..standing("/tmp/gone", true)
        });

        let remedy = check(&found, "server").remedy.unwrap();
        assert!(
            remedy.contains("tmux -S /run/user/1000/tmux/sock kill-server"),
            "{remedy}"
        );
    }

    #[test]
    fn doctor_names_the_floor_when_tmux_is_too_old() {
        let mut found = healthy();
        found.tmux = Some((3, 0));

        let tmux = check(&found, "tmux");
        let remedy = tmux.remedy.as_deref().unwrap();
        assert!(remedy.contains("3.2"), "{remedy}");
        assert!(tmux.found.contains("3.0"), "{}", tmux.found);
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn doctor_names_tmux_when_there_is_none() {
        let mut found = healthy();
        found.tmux = None;

        let tmux = check(&found, "tmux");
        assert!(tmux.remedy.as_deref().unwrap().contains("tmux"));
    }

    #[test]
    fn doctor_points_at_the_config_when_the_vendor_is_not_there() {
        let mut found = healthy();
        found.vendor_path = None;
        found.vendor = "claude --model opus".to_string();

        let vendor = check(&found, "agent");
        let remedy = vendor.remedy.as_deref().unwrap();
        assert!(remedy.contains("agent"), "the config key to set: {remedy}");
        assert!(vendor.found.contains("claude"), "{}", vendor.found);
    }

    #[test]
    fn doctor_repeats_what_the_config_said_about_itself() {
        let mut found = healthy();
        found.config_warnings = vec!["ignoring unknown key `wardrobe`".to_string()];

        let config = check(&found, "config");
        assert!(config.found.contains("wardrobe"), "{}", config.found);
        assert!(
            config.remedy.as_deref().unwrap().contains("config.toml"),
            "the file to edit is named"
        );
    }

    #[test]
    fn doctor_names_the_events_that_are_not_wired() {
        let mut found = healthy();
        found.wired = vec!["Stop".to_string(), "SessionStart".to_string()];

        let hooks = check(&found, "hooks");
        assert!(hooks.found.contains("Notification"), "{}", hooks.found);
        assert!(hooks.found.contains("PreToolUse"), "{}", hooks.found);
        assert!(
            !hooks.found.contains("Stop,"),
            "not the ones that are: {}",
            hooks.found
        );
        assert!(hooks.remedy.as_deref().unwrap().contains("--fix"));
    }

    #[test]
    fn doctor_says_a_vendor_that_reports_nothing_leaves_the_pane_to_read() {
        // Hooks are a vendor's own doing, and one that has none is not a
        // machine with something missing from it: there is nothing to wire and
        // nothing to fix, and what amx has instead is the pane.
        let mut found = healthy();
        found.vendor = SECOND.name.to_string();
        found.wired = Vec::new();

        let hooks = wiring_check(&found, Some(&SECOND));
        assert!(
            hooks.is_ok(),
            "nothing here is anybody's to repair: {hooks:?}"
        );
        assert!(hooks.found.contains(SECOND.name), "{}", hooks.found);
        assert!(hooks.found.contains("pane"), "{}", hooks.found);

        // The vendor amx was written against still answers for its wiring, and
        // so does a command amx has no entry for: nothing measured is not a
        // measurement, and a wrapper around claude reports through the same
        // settings file.
        for measured in [crate::registry::entry("claude"), None] {
            let hooks = wiring_check(&found, measured);
            assert!(!hooks.is_ok(), "{hooks:?}");
            assert!(hooks.remedy.as_deref().unwrap().contains("--fix"));
        }
    }

    #[test]
    fn doctor_reports_settings_it_cannot_read_without_offering_to_write_them() {
        let mut found = healthy();
        found.wired = Vec::new();
        found.settings_error = Some("expected value at line 1 column 3".to_string());

        let hooks = check(&found, "hooks");
        let remedy = hooks.remedy.as_deref().unwrap();
        assert!(hooks.found.contains("line 1"), "{}", hooks.found);
        assert!(
            !remedy.contains("--fix"),
            "amx cannot fix this one: {remedy}"
        );
        assert!(remedy.contains("settings.json"), "{remedy}");
    }

    #[test]
    fn doctor_fix_wires_the_hooks_once_somebody_agrees() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(&settings, "{\"model\": \"opus\"}\n").unwrap();

        let mut found = healthy();
        found.settings = settings.clone();
        found.wired = Vec::new();

        let mut out = Vec::new();
        let code = run(&found, true, 1, &mut "y\n".as_bytes(), &mut out).unwrap();
        let printed = String::from_utf8(out).unwrap();

        assert_eq!(code, exit::OK, "nothing is wrong any more: {printed}");
        assert!(printed.contains("will add"), "it asked first: {printed}");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            install::installed_events(&written, COMMAND).len(),
            install::EVENTS.len()
        );
        assert_eq!(written["model"], "opus");
    }

    #[test]
    fn doctor_fix_writes_nothing_when_nobody_agrees() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let before = "{\"model\": \"opus\"}\n";
        std::fs::write(&settings, before).unwrap();

        let mut found = healthy();
        found.settings = settings.clone();
        found.wired = Vec::new();

        let mut out = Vec::new();
        let code = run(&found, true, 1, &mut "n\n".as_bytes(), &mut out).unwrap();

        assert_eq!(code, exit::FAILURE, "the hooks are still not wired");
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), before);
        assert!(
            String::from_utf8(out).unwrap().contains("left"),
            "it says so"
        );
    }

    #[test]
    fn doctor_fix_leaves_a_wired_machine_alone() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        install::install(&settings, COMMAND, 1).unwrap();
        let before = std::fs::read_to_string(&settings).unwrap();

        let mut found = healthy();
        found.settings = settings.clone();

        let mut out = Vec::new();
        assert_eq!(
            run(&found, true, 2, &mut "".as_bytes(), &mut out).unwrap(),
            exit::OK
        );
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), before);
    }

    #[test]
    fn doctor_fix_will_not_touch_settings_it_cannot_read() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let broken = "{ not json";
        std::fs::write(&settings, broken).unwrap();

        let mut found = healthy();
        found.settings = settings.clone();
        found.wired = Vec::new();
        found.settings_error = Some("expected value".to_string());

        let mut out = Vec::new();
        let code = run(&found, true, 1, &mut "y\n".as_bytes(), &mut out).unwrap();
        assert_eq!(code, exit::FAILURE);
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), broken);
    }

    #[test]
    fn doctor_finds_a_program_the_way_a_shell_would() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let claude = second.path().join("claude");
        std::fs::write(&claude, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();

        let path = std::ffi::OsString::from(format!(
            "{}:{}",
            first.path().display(),
            second.path().display()
        ));
        assert_eq!(on_path("claude", Some(&path)), Some(claude.clone()));
        assert_eq!(on_path("nowhere", Some(&path)), None);
        assert_eq!(on_path("claude", None), None);

        // A command that is a path is not looked for on the PATH at all.
        assert_eq!(
            on_path(&claude.to_string_lossy(), None),
            Some(claude.clone())
        );
        assert_eq!(on_path("/nowhere/claude", None), None);
    }

    #[test]
    fn doctor_will_not_run_a_file_that_is_not_executable() {
        let dir = TempDir::new().unwrap();
        let claude = dir.path().join("claude");
        std::fs::write(&claude, "text").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o644)).unwrap();

        let path = std::ffi::OsString::from(dir.path().to_string_lossy().to_string());
        assert_eq!(on_path("claude", Some(&path)), None);
    }

    #[test]
    fn doctor_reads_the_program_out_of_a_command_with_arguments() {
        assert_eq!(program("claude"), "claude");
        assert_eq!(program("claude --model opus"), "claude");
        assert_eq!(
            program("/usr/local/bin/claude --resume"),
            "/usr/local/bin/claude"
        );
    }

    #[test]
    fn doctor_names_an_unwritable_state_root_instead_of_saying_no_agents() {
        // A state root nobody can write to and a machine that has simply never
        // run an agent look the same from a listing: both say "no agents".
        let mut found = healthy();
        found.state_root = PathBuf::from("/srv/amx/agents");
        found.state_error =
            Some("/srv/amx/agents cannot be written to: Permission denied".to_string());

        let state = check(&found, "state");
        assert!(state.found.contains("/srv/amx/agents"), "{}", state.found);
        assert!(state.found.contains("Permission denied"), "{}", state.found);
        assert!(
            state.remedy.is_some(),
            "and something to do about it: {state:?}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn a_state_root_that_is_not_there_yet_is_judged_by_the_directory_it_would_go_in() {
        let dir = TempDir::new().unwrap();
        assert_eq!(usable(&dir.path().join("state/amx/agents")), None);
    }

    #[test]
    fn a_state_root_nobody_can_write_to_names_it_and_says_why() {
        if !not_root() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("agents");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();

        let why = usable(&root).expect("read only, so a new agent has nowhere to go");
        assert!(why.contains(&root.display().to_string()), "{why}");
        assert!(why.to_lowercase().contains("permission denied"), "{why}");
    }

    #[test]
    fn a_state_root_that_cannot_be_made_names_the_directory_that_refused() {
        if !not_root() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let closed = dir.path().join("state");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o500)).unwrap();

        // The root is two levels below a directory that will not take it, so
        // the name in the answer is the directory that actually refused.
        let why = usable(&closed.join("amx/agents")).expect("nowhere to make it");
        assert!(why.contains(&closed.display().to_string()), "{why}");
    }

    #[test]
    fn doctor_names_the_agent_stopped_at_the_vendors_trust_question() {
        let mut found = healthy();
        found.parked = vec![Parked {
            id: "fix-auth-2k3".to_string(),
            screen: Setup::Trust,
        }];

        let setup = check(&found, "setup");
        assert!(setup.found.contains("fix-auth-2k3"), "{}", setup.found);
        assert!(setup.found.contains("trust"), "{}", setup.found);
        assert!(
            setup.found.contains(a_trusting_vendor().name),
            "whose screen it is comes off the table: {}",
            setup.found
        );
        let remedy = setup.remedy.as_deref().unwrap();
        assert!(remedy.contains("amx attach fix-auth-2k3"), "{remedy}");
        assert!(
            remedy.contains("trust = true"),
            "the key that makes it never happen again is named: {remedy}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);

        // The offer to answer it is only amx's to make for a vendor whose
        // screen amx knows: told there is none, it leaves the question to
        // whoever is at the keyboard.
        let setup = setup_check(&found, None);
        let remedy = setup.remedy.as_deref().unwrap();
        assert!(remedy.contains("amx attach fix-auth-2k3"), "{remedy}");
        assert!(!remedy.contains("trust = true"), "{remedy}");
    }

    #[test]
    fn doctor_names_an_agent_the_vendor_never_let_start() {
        // What a login prompt looks like from out here, and amx says only what
        // it can see: a screen no rule claims, from an agent that has never
        // reported anything.
        let mut found = healthy();
        found.parked = vec![Parked {
            id: "port-cli-b91".to_string(),
            screen: Setup::Unread,
        }];

        let setup = check(&found, "setup");
        assert!(setup.found.contains("port-cli-b91"), "{}", setup.found);
        assert!(
            setup.remedy.as_deref().unwrap().contains("amx attach"),
            "somebody has to look at it: {setup:?}"
        );
    }

    #[test]
    fn doctor_names_every_agent_stopped_at_setup_and_one_to_start_with() {
        let mut found = healthy();
        found.parked = vec![
            Parked {
                id: "fix-auth-2k3".to_string(),
                screen: Setup::Trust,
            },
            Parked {
                id: "port-cli-b91".to_string(),
                screen: Setup::Unread,
            },
        ];

        let setup = check(&found, "setup");
        assert!(setup.found.contains('2'), "how many: {}", setup.found);
        assert!(setup.found.contains("fix-auth-2k3"), "{}", setup.found);
        assert!(setup.found.contains("port-cli-b91"), "{}", setup.found);
        assert!(
            setup
                .remedy
                .as_deref()
                .unwrap()
                .contains("amx attach fix-auth-2k3"),
            "one of them to start with: {setup:?}"
        );
    }

    #[test]
    fn only_an_agent_that_never_got_started_is_stopped_at_setup() {
        let views = vec![
            view("works-a1b", Phase::Working, Phase::Working, Some("spinner")),
            view(
                "asks-b2c",
                Phase::Working,
                Phase::Waiting,
                Some("permission_prompt"),
            ),
            view("waits-c3d", Phase::Idle, Phase::Idle, Some("idle_prompt")),
            view(
                "trust-d4e",
                Phase::Starting,
                Phase::Waiting,
                Some("folder_trust"),
            ),
            view("login-e5f", Phase::Starting, Phase::Unknown, None),
            // Interrupted mid-turn onto a screen no rule claims. The vendor let
            // this one start, so it is not stopped at setup.
            view("lost-f6g", Phase::Working, Phase::Unknown, None),
            // Started, drawn, and sitting at its prompt with nothing to do.
            view(
                "fresh-g7h",
                Phase::Starting,
                Phase::Idle,
                Some("idle_prompt"),
            ),
        ];

        assert_eq!(
            parked(&views),
            vec![
                Parked {
                    id: "trust-d4e".to_string(),
                    screen: Setup::Trust,
                },
                Parked {
                    id: "login-e5f".to_string(),
                    screen: Setup::Unread,
                },
            ]
        );
    }

    #[test]
    fn a_state_root_nobody_can_read_is_not_an_empty_one() {
        if !not_root() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("agents");
        std::fs::create_dir(&root).unwrap();
        // Write and search but no read: a listing of it fails outright, which
        // is the other way a full state root passes for an empty one.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o300)).unwrap();

        let why = usable(&root);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(why.is_some(), "a root amx cannot list is a root to report");
    }
}
