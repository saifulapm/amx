//! Path derivation: every path a session uses comes from the passed [`Env`].
//!
//! 04 §2's "no env-var globals, no test mutexes" only holds if the environment
//! is a value, so the interesting property of each derivation is *not* which
//! directory it picks but that it picks it from the argument. `config.toml` is
//! the M1 addition: it is per user rather than per session, which is exactly
//! the shape that would tempt an implementation to reach for the process
//! environment.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;

use amx_core::ctx::CONFIG_NAME;
use amx_core::{Ctx, Env, SessionName};

fn env_at(root: &str) -> Env {
    let root = PathBuf::from(root);
    Env {
        runtime_dir: Some(root.join("run")),
        state_home: Some(root.join("state")),
        config_home: Some(root.join("config")),
        home: Some(root.join("home")),
        session: None,
        tmpdir: None,
    }
}

fn ctx_for(name: &str, env: &Env) -> Ctx {
    Ctx::for_session(SessionName::new(name).expect("a valid session name"), env)
        .expect("derive the context")
}

#[test]
fn config_path_derives_from_passed_env_not_process_env() {
    let env = env_at("/amx-test-ctx/passed");
    // The passed roots are fabricated absolute paths no real environment
    // names, so the process environment is provably a different answer: an
    // implementation that reached for `XDG_CONFIG_HOME` behind the argument's
    // back could not land where the assertion below requires.
    assert_ne!(
        Env::from_process().config_home,
        env.config_home,
        "the fixture must not coincide with the process environment",
    );

    let ctx = ctx_for("work", &env);
    assert_eq!(
        ctx.config_path,
        PathBuf::from("/amx-test-ctx/passed/config/amx").join(CONFIG_NAME),
    );

    // Two environments in one process resolve to two config files, with no
    // mutex between them — the property W9 is about.
    let other = env_at("/amx-test-ctx/other");
    assert_ne!(ctx_for("work", &other).config_path, ctx.config_path);
}

#[test]
fn config_path_is_per_user_not_per_session() {
    let env = env_at("/amx-test-ctx/shared");
    assert_eq!(
        ctx_for("work", &env).config_path,
        ctx_for("play", &env).config_path,
        "configuration is shared by every session this user runs",
    );
    assert_ne!(
        ctx_for("work", &env).state_dir,
        ctx_for("play", &env).state_dir,
        "state, unlike config, is per session",
    );
}

#[test]
fn config_home_falls_back_to_home_dot_config() {
    let env = Env {
        config_home: None,
        ..env_at("/amx-test-ctx/fallback")
    };
    assert_eq!(
        ctx_for("work", &env).config_path,
        PathBuf::from("/amx-test-ctx/fallback/home/.config/amx").join(CONFIG_NAME),
    );
}

#[test]
fn a_relative_or_absent_config_root_is_refused() {
    let relative = Env {
        config_home: Some(PathBuf::from("config")),
        ..env_at("/amx-test-ctx/relative")
    };
    assert!(
        Ctx::for_session(SessionName::default(), &relative).is_err(),
        "a relative root would resolve against the process's cwd",
    );

    let nothing = Env {
        config_home: None,
        home: None,
        ..env_at("/amx-test-ctx/nothing")
    };
    assert!(Ctx::for_session(SessionName::default(), &nothing).is_err());
}
