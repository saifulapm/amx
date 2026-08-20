//! `amx doctor` — what amx needs from this machine, and what is missing.
//!
//! Four things have to be true before an agent can run: a tmux new enough to
//! address panes by id, a vendor command to run, a config amx can read, and
//! amx's hooks wired into the vendor's settings. Each check that fails says
//! what to do about it, because a check that only says "no" leaves somebody
//! guessing at a machine they thought was fine.
//!
//! `--fix` does the one repair amx can make safely: wiring the hooks, after
//! asking.

use anyhow::Result;
use std::ffi::OsStr;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::{exit, install, tmux};

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
}

/// Judge what was found.
pub fn report(found: &Findings) -> Vec<Check> {
    vec![
        tmux_check(found),
        vendor_check(found),
        config_check(found),
        hooks_check(found),
    ]
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

fn hooks_check(found: &Findings) -> Check {
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
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

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
        }
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
        assert_eq!(checks.len(), 4, "tmux, the vendor, the config, the hooks");

        let (code, printed) = said(&healthy(), false);
        assert_eq!(code, exit::OK);
        assert!(printed.contains("tmux"), "{printed}");
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
        assert_eq!(install::installed_events(&written, COMMAND).len(), 5);
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
}
