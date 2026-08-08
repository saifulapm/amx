//! Seeding: streaming this binary to a host that has none, same platform only.
//!
//! The scope here is narrow on purpose, and the reason is D-M3-8's hosting
//! reality inherited whole (R-M3-4): **there is no channel to fetch from.** No
//! release pipeline exists, no prebuilt binaries exist, and the only amx
//! available to install on the far side is the one running this command. So a
//! host whose `uname` matches gets *these bytes*, after an explicit
//! confirmation; a host whose `uname` differs gets a refusal that names both
//! platforms and says why — because "cross-platform seeding" would mean
//! downloading a binary for the other platform, and there is nowhere to
//! download it from.
//!
//! Saying so is the feature. A stub that offered to install the wrong
//! architecture and left a file that cannot exec would be worse than the
//! refusal, and a stub that silently did nothing would be worse than both.
//!
//! # The install shape
//!
//! `cat` to a temporary name in the destination directory, `chmod`, then `mv`
//! over the target — the three-step shape every careful installer uses, for the
//! reason `rename(2)` exists: the destination is either the old file or the new
//! one and never a half-written one, and a running process holding the old
//! image is untouched by a directory entry moving.
//!
//! `~/.local/bin` and nowhere else. It needs no privileges, and it is the one
//! directory `crate::remote::ssh`'s bridge script looks in besides `PATH`, so a
//! seeded host attaches on the next try whether or not its non-interactive
//! shell has that directory on `PATH` — which on most systems it does not.

use std::fs::File;
use std::io::{BufRead as _, Write as _};
use std::process::{ExitCode, Stdio};

use anyhow::Context as _;

use super::ssh::{Missing, Platform, Remote};

/// Where a seeded amx lands, relative to the remote user's home.
pub const INSTALL_PATH: &str = "~/.local/bin/amx";

/// Answer a host that has no amx: offer to seed it, or refuse with the reason.
///
/// Returns success in both directions a *decision* can go — installed, or
/// declined — because neither is a failure of this command. Only a platform
/// mismatch and a broken install are errors.
///
/// # Errors
///
/// If the remote platform cannot be read, if it differs from this one, or if
/// the stream itself fails.
pub async fn offer(remote: &Remote, missing: &Missing) -> anyhow::Result<ExitCode> {
    let host = remote.host();
    println!("{host} has no amx on PATH or in ~/.local/bin.");
    // Anything ssh or the far side's shell said *besides* amx's own marker: a
    // warning about the host key, a banner, whatever else made the trip. The
    // marker itself is the sentence above, already said better.
    for line in missing
        .said
        .lines()
        .filter(|line| !line.contains(crate::remote::ssh::NO_REMOTE_AMX))
    {
        println!("  {line}");
    }

    let there = remote
        .platform()
        .await
        .context("read the remote platform")?;
    let here = Platform::local().context("read this machine's platform")?;

    anyhow::ensure!(
        there == here,
        "cannot seed {host}: it is {there} and this machine is {here}. The only \
         amx available to install is the one running now, and it is built for \
         {here}. Cross-platform seeding needs published binaries to download, \
         and no release channel exists yet (R-M3-4) — install amx on {host} the \
         way you installed it here.",
    );

    let exe = amx_server::session::daemon::current_exe().context("find this binary")?;
    println!(
        "It is {there}, the same as this machine, so the amx running now can be \
         streamed to it."
    );
    println!("  from {}", exe.display());
    println!("  to   {host}:{INSTALL_PATH}");
    if !confirmed("Install it there?")? {
        println!("Nothing was installed.");
        return Ok(ExitCode::SUCCESS);
    }

    let binary = File::open(&exe).with_context(|| format!("open {}", exe.display()))?;
    remote.feed(INSTALL_SCRIPT, Stdio::from(binary)).await?;

    println!("Installed amx at {host}:{INSTALL_PATH}.");
    println!("Run `amx --remote {host}` again to attach.");
    Ok(ExitCode::SUCCESS)
}

/// Ask `question` and read the answer from this terminal.
///
/// Anything but an explicit yes is a no, including end of input: seeding writes
/// an executable onto someone else's machine, and a default that installed
/// would be a default that installed by accident.
fn confirmed(question: &str) -> anyhow::Result<bool> {
    print!("{question} [y/N] ");
    std::io::stdout().flush().context("prompt")?;
    let mut answer = String::new();
    if std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("read the answer")?
        == 0
    {
        println!();
        return Ok(false);
    }
    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// What the far side runs while this binary streams into its stdin.
///
/// Every word of it is a literal — no session name, no host, no path from the
/// caller reaches it — so there is nothing here to quote and nothing to inject.
/// `$$` names the remote shell's own pid, which is what keeps two seedings of
/// the same host from colliding on the temporary file.
const INSTALL_SCRIPT: &str = "\
set -e
dir=\"$HOME/.local/bin\"
mkdir -p \"$dir\"
tmp=\"$dir/.amx.$$\"
trap 'rm -f \"$tmp\"' EXIT
cat > \"$tmp\"
chmod 755 \"$tmp\"
mv -f \"$tmp\" \"$dir/amx\"
";
