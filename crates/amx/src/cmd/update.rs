//! `amx update check|apply` — self-update (D-M3-8).
//!
//! The mechanisms are herdr's, mined and not copied: channel manifests as JSON
//! with a per-platform asset URL and sha256, fetched with a curl subprocess so
//! no HTTP dependency enters the tree, verified before install, and
//! package-manager installs *redirected* — print brew's or mise's or nix's own
//! upgrade command, never write into their tree. They live one to a module
//! under [`crate::update`]; this file is the verb, which is to say the order
//! they run in and the sentences the user reads.
//!
//! # The honesty this verb owes (R-M3-4)
//!
//! There is no release pipeline, no prebuilt binary and no published manifest.
//! `check` against the default channel therefore reports that plainly and exits
//! zero: "nothing is published" is an answer, and dressing it up as an error
//! would be as wrong as dressing it up as a service. Nothing in this file
//! implies a hosted channel exists.
//!
//! # The dormant half
//!
//! D-M3-8 finishes the story: `apply` asks the running session for
//! `session.handoff` and then reconnect-polls until the successor answers.
//! [`hand_over`] is that code, written and compiled — and not reached, because
//! [`HANDOFF_AFTER_INSTALL`] is `false` while `session.handoff` is still W06's
//! unwired seam. **Flipping that one constant to `true` is the whole of turning
//! it on** (W14, once W06 has merged). Until then `apply` installs the binary
//! and says, in as many words, that the running session was not swapped.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use amx_client::net::{self, Session};
use amx_core::{Config, Ctx, Env};
use amx_proto::control::session::{HandoffParams, HandoffReply};
use amx_proto::hello::Welcome;
use amx_server::session::probe::probe;
use anyhow::{Context as _, bail};
use clap::ArgMatches;

use crate::cmd::attach::client_info;
use crate::cmd::call;
use crate::ctx_of;
use crate::update::manifest::{DEFAULT_CHANNEL, Manifest, Standing, current, platform};
use crate::update::{fetch, install, pm};

/// Whether a successful `apply` hands the running session over.
///
/// `false` until W06 lands the server half: `session.handoff` is in the method
/// table but answers with a seam error, so calling it would turn a successful
/// install into a failed command. [`hand_over`] below is the whole of the glue
/// and is typechecked on every build; W14 flips this to `true`.
pub const HANDOFF_AFTER_INSTALL: bool = false;

/// The clap id of `--channel`, which both verbs take.
const CHANNEL: &str = "channel";

/// How long `apply` waits for the successor to answer, once a handoff is
/// accepted. herdr's own budget for the whole staged protocol is 30 s a stage;
/// this is the caller's view of all of them plus the socket changing hands.
const HANDOFF_PATIENCE: Duration = Duration::from_secs(90);

/// How long to wait between reconnect attempts while the socket is changing
/// hands. Short: the window between the exporter's unlink and the importer's
/// bind is milliseconds in the ordinary case (D-M3-6 point 4).
const RECONNECT_TICK: Duration = Duration::from_millis(100);

/// Run `amx update`.
///
/// # Errors
///
/// If the subcommand is missing, the channel URL is one amx will not fetch, the
/// manifest is malformed, a download fails its checksum, or the install cannot
/// be written. A channel with nothing published is *not* an error.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, None)?;
    match sub.subcommand() {
        Some(("check", args)) => check(&ctx, args).await,
        Some(("apply", args)) => apply(env, root, &ctx, args).await,
        _ => bail!("`amx update` needs a verb: `check` or `apply`"),
    }
}

// ------------------------------------------------------------------- check

/// Report what the channel has, and say so plainly when it has nothing.
async fn check(ctx: &Ctx, args: &ArgMatches) -> anyhow::Result<ExitCode> {
    let exe = current_exe()?;
    let install = pm::classify(&exe);
    let channel = channel_of(ctx, args.get_one::<String>(CHANNEL).map(String::as_str))?;
    let running = current();

    println!("amx {running} at {}", exe.display());
    if let (Some(manager), Some(hint)) = (install.manager(), install.upgrade_hint()) {
        println!("installed by {manager}: {hint} rather than `amx update apply`");
    }
    println!("channel {channel}");

    let manifest = match fetch::manifest(&channel).await? {
        fetch::Fetched::Body(bytes) => Manifest::parse(&bytes)?,
        fetch::Fetched::Missing(missing) => {
            println!("{missing}");
            if channel == DEFAULT_CHANNEL {
                println!("{UNPUBLISHED}");
            }
            return Ok(ExitCode::SUCCESS);
        }
    };

    match manifest.standing(running) {
        Standing::Newer => {
            println!("{} is available", manifest.version);
            if let Some(notes) = &manifest.notes {
                println!("{notes}");
            }
            match manifest.asset(&platform()) {
                Ok(asset) => println!("asset {}", asset.url),
                Err(err) => println!("but {err:#}"),
            }
            if !install.is_managed() {
                println!("run `amx update apply` to install it");
            }
        }
        Standing::Same => println!("{running} is the newest published version"),
        Standing::Older => println!(
            "the channel publishes {}, which is older than this build; \
             `amx update apply` installs nothing",
            manifest.version
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// What `check` adds when the default channel has nothing, and only then.
const UNPUBLISHED: &str = "amx has no release pipeline yet: no manifest, no prebuilt binaries and \
                           no preview channel are published, so this is the expected answer \
                           rather than a failure.";

// ------------------------------------------------------------------- apply

/// Install the newest amx, and say exactly what happened to the running session.
async fn apply(
    env: &Env,
    root: &ArgMatches,
    ctx: &Ctx,
    args: &ArgMatches,
) -> anyhow::Result<ExitCode> {
    let exe = current_exe()?;
    let install = pm::classify(&exe);
    if let (Some(manager), Some(hint)) = (install.manager(), install.upgrade_hint()) {
        // Nothing has been fetched, staged or written at this point, and the
        // order is deliberate: a redirect that had already downloaded something
        // would be a write into a managed install's neighbourhood.
        bail!(
            "{} was installed by {manager}, so amx will not replace it: {hint}. \
             Nothing was written.",
            exe.display()
        );
    }

    let channel = channel_of(ctx, args.get_one::<String>(CHANNEL).map(String::as_str))?;
    let running = current();
    println!("amx {running} at {}", exe.display());
    println!("channel {channel}");

    let manifest = match fetch::manifest(&channel).await? {
        fetch::Fetched::Body(bytes) => Manifest::parse(&bytes)?,
        fetch::Fetched::Missing(missing) => {
            println!("{missing}");
            if channel == DEFAULT_CHANNEL {
                println!("{UNPUBLISHED}");
            }
            println!("nothing was installed");
            return Ok(ExitCode::SUCCESS);
        }
    };

    match manifest.standing(running) {
        Standing::Newer => {}
        Standing::Same => {
            println!("{running} is already the newest published version; nothing was installed");
            return Ok(ExitCode::SUCCESS);
        }
        Standing::Older => {
            println!(
                "the channel publishes {}, which is older than this build; nothing was installed",
                manifest.version
            );
            return Ok(ExitCode::SUCCESS);
        }
    }

    let key = platform();
    let asset = manifest.asset(&key)?;
    println!("downloading {} for {key}", manifest.version);
    let staged = install::stage(&ctx.state_dir, asset).await?;
    println!("sha256 {} verified", asset.sha256);
    install::install(staged, &exe)?;
    println!("installed {} at {}", manifest.version, exe.display());

    if HANDOFF_AFTER_INSTALL {
        hand_over(env, root, ctx, &exe).await?;
    } else {
        println!("{DORMANT}");
    }
    Ok(ExitCode::SUCCESS)
}

/// What `apply` says while [`HANDOFF_AFTER_INSTALL`] is `false`.
///
/// One sentence of fact and one of instruction. The user has a new binary on
/// disk and an old one still serving their panes, and the only thing worse than
/// that gap is not being told about it.
const DORMANT: &str = "the running session was not handed over: this build does not trigger \
                       `session.handoff` yet, so panes keep running on the old binary until the \
                       session is restarted (`amx session stop`, then `amx`).";

// ---------------------------------------------------------------- the swap

/// Hand the running session over to the binary just installed (D-M3-8).
///
/// **Dormant.** Reached only when [`HANDOFF_AFTER_INSTALL`] is `true`, which it
/// is not while `session.handoff` is W06's unwired seam.
///
/// The shape, once it runs: the call is accepted rather than completed — the
/// exporter retires its gateway partway through the protocol, so this very
/// connection dies by design — and completion is observed by reconnecting. A
/// `Welcome` carrying the *same* `SessionId` and a *different* server version
/// is the successor, and is success. The old version answering again means the
/// abort path ran and the session was rolled back to it, which `session report`
/// explains.
///
/// # Errors
///
/// If no session is running, the call is refused, or nothing answers the socket
/// before [`HANDOFF_PATIENCE`] expires.
async fn hand_over(env: &Env, root: &ArgMatches, ctx: &Ctx, binary: &Path) -> anyhow::Result<()> {
    if !probe(&ctx.socket)
        .context("probe the session socket")?
        .is_running()
    {
        println!("no session is running, so there was nothing to hand over");
        return Ok(());
    }

    let before = welcome(ctx).await?;
    let params = serde_json::to_value(HandoffParams {
        binary: binary.to_path_buf(),
        timeout_ms: None,
    })
    .context("encode the handoff parameters")?;
    let reply: HandoffReply =
        serde_json::from_value(call::one_shot(env, root, "session.handoff", params).await?)
            .context("read the handoff reply")?;
    if !reply.accepted {
        bail!(
            "the session refused the handoff: {}",
            reply.reason.as_deref().unwrap_or("no reason given")
        );
    }
    println!("handoff accepted; waiting for the successor");

    let deadline = tokio::time::Instant::now() + HANDOFF_PATIENCE;
    loop {
        if let Ok(now) = welcome(ctx).await {
            if now.session == before.session && now.server.version != before.server.version {
                println!(
                    "session {} is now served by amx {}",
                    ctx.session, now.server.version
                );
                return Ok(());
            }
            if now.session != before.session {
                bail!(
                    "a different session answered {}; the handoff did not complete",
                    ctx.socket.display()
                );
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "no successor answered {} within {} seconds; `amx session report` says how far \
                 the handoff got",
                ctx.socket.display(),
                HANDOFF_PATIENCE.as_secs()
            );
        }
        tokio::time::sleep(RECONNECT_TICK).await;
    }
}

/// Connect to the session and read its welcome.
async fn welcome(ctx: &Ctx) -> anyhow::Result<Welcome> {
    let stream = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    let (_session, welcome) = Session::attach(stream, client_info(), false, None)
        .await
        .context("negotiate with the session")?;
    Ok(welcome)
}

// ------------------------------------------------------------------ inputs

/// Which channel to read: the flag, then the config, then the built-in default.
///
/// The config file is read here rather than asked of a server, because `amx
/// update` must work with no session running at all — and it is read leniently,
/// through the same [`amx_core::config::reload`] every server uses, so a
/// half-edited file costs the user a default rather than a failure.
fn channel_of(ctx: &Ctx, flag: Option<&str>) -> anyhow::Result<String> {
    if let Some(url) = flag {
        fetch::check_scheme(url)?;
        return Ok(url.to_owned());
    }
    let configured = match std::fs::read_to_string(&ctx.config_path) {
        Ok(text) => {
            amx_core::config::reload(&Config::default(), &text)
                .0
                .update
                .channel
        }
        Err(_) => None,
    };
    let url = configured.unwrap_or_else(|| DEFAULT_CHANNEL.to_owned());
    fetch::check_scheme(&url)?;
    Ok(url)
}

/// Where this binary is.
///
/// The one piece of process state this verb cannot be handed as a value: the
/// path amx is going to replace is a fact about the running process. On Linux
/// it is `/proc/self/exe` resolved, which is also what makes the
/// package-manager shapes in [`pm`] recognisable through a `PATH` symlink.
fn current_exe() -> anyhow::Result<PathBuf> {
    std::env::current_exe().context("find the running amx binary")
}
