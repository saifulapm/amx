//! Agent ids.
//!
//! An id is `<stem>-<suffix>`: leading words of the task, lowercased and
//! hyphenated to at most [`MAX_STEM`] characters, plus three base36 characters
//! of entropy. It is unique under the state root because it *is* the agent's
//! directory name there.
//!
//! The charset — lowercase letters, digits, `-` — is checked wherever an id
//! becomes a path, not only where one is minted. Ids travel: an orchestrating
//! agent reads them out of another agent's output and hands them back to amx,
//! so `../../elsewhere` has to be refused at use.

use anyhow::{Result, bail};
use std::path::Path;

/// Longest stem an id may carry, before its `-<suffix>`.
pub const MAX_STEM: usize = 20;

/// Stem used when the task text yields no usable characters at all.
pub const FALLBACK_STEM: &str = "agent";

const SUFFIX_LEN: usize = 3;

/// How many suffixes to try before admitting the stem is crowded.
const MAX_ATTEMPTS: usize = 64;

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Derive an id stem from free-form task text.
pub fn stem_from_task(task: &str) -> String {
    let mut hyphenated = String::with_capacity(task.len());
    for ch in task.chars() {
        if ch.is_ascii_alphanumeric() {
            hyphenated.push(ch.to_ascii_lowercase());
        } else if !hyphenated.ends_with('-') {
            hyphenated.push('-');
        }
    }

    let trimmed = hyphenated.trim_matches('-');
    if trimmed.is_empty() {
        return FALLBACK_STEM.to_string();
    }
    if trimmed.len() <= MAX_STEM {
        return trimmed.to_string();
    }

    // Over the cap: keep whole leading words when the cap leaves room for one,
    // and hard-truncate only a single overlong word. `hyphenated` is ASCII by
    // construction, so slicing at MAX_STEM is always on a char boundary.
    let capped = &trimmed[..MAX_STEM];
    if trimmed.as_bytes()[MAX_STEM] == b'-' {
        // The cap landed on a word boundary — nothing was cut in half.
        return capped.to_string();
    }
    match capped.rsplit_once('-') {
        Some((words, _partial)) if !words.is_empty() => words.to_string(),
        _ => capped.to_string(),
    }
}

/// Mint an id for `task` that no directory under `state_root` holds yet.
pub fn generate(task: &str, state_root: &Path) -> Result<String> {
    let stem = stem_from_task(task);
    for _ in 0..MAX_ATTEMPTS {
        let id = format!("{stem}-{}", suffix());
        if !state_root.join(&id).exists() {
            return Ok(id);
        }
    }
    bail!(
        "no free id for {stem:?} under {} after {MAX_ATTEMPTS} tries",
        state_root.display()
    );
}

/// Whether `id` is an id at all: the charset law, asked where an id becomes a
/// path. It needs no filesystem, so readers can ask it anywhere.
pub fn is_valid(id: &str) -> bool {
    !id.is_empty() && id.chars().all(legal)
}

/// Validate a user-supplied `--name`: the same charset and the same
/// uniqueness as a minted id, but deliberately not length-capped — the cap
/// exists to keep *derived* stems readable, and a typed name is not derived.
pub fn validate_name(name: &str, state_root: &Path) -> Result<()> {
    if name.is_empty() {
        bail!("a name cannot be empty");
    }
    if let Some(bad) = name.chars().find(|c| !legal(*c)) {
        bail!("name {name:?} contains {bad:?}: names are lowercase letters, digits and '-'");
    }
    if state_root.join(name).exists() {
        bail!("name {name:?} is already taken");
    }
    Ok(())
}

/// One character of the id charset.
fn legal(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

/// Three base36 characters that tell two ids with the same stem apart.
///
/// `RandomState` is seeded by the OS and advances per call, so ids differ
/// within one process as well as between runs. There is no RNG crate here and
/// none is wanted: a suffix avoids collisions, it is not a secret.
fn suffix() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    hasher.write_u32(std::process::id());
    let mut bits = hasher.finish();

    let mut out = String::with_capacity(SUFFIX_LEN);
    for _ in 0..SUFFIX_LEN {
        out.push(BASE36[(bits % 36) as usize] as char);
        bits /= 36;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn legal_charset(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn a_stem_is_the_task_lowercased_and_hyphenated() {
        assert_eq!(stem_from_task("Fix the login bug"), "fix-the-login-bug");
    }

    #[test]
    fn runs_of_punctuation_collapse_to_one_hyphen_and_the_ends_are_trimmed() {
        assert_eq!(
            stem_from_task("  add: OAuth2 // support! "),
            "add-oauth2-support"
        );
    }

    #[test]
    fn a_stem_stops_at_a_word_boundary_within_the_cap() {
        // Whole leading words only: the cap never cuts a word in half.
        let stem = stem_from_task("port the payment gateway to the new api");
        assert_eq!(stem, "port-the-payment");
        assert!(stem.len() <= MAX_STEM);
    }

    #[test]
    fn a_single_overlong_word_is_truncated_because_no_boundary_exists() {
        let stem = stem_from_task("supercalifragilisticexpialidocious");
        assert_eq!(stem.len(), MAX_STEM);
        assert!(legal_charset(&stem));
    }

    #[test]
    fn a_cap_landing_on_a_boundary_keeps_every_whole_word() {
        // "twenty-characters-ok" is exactly MAX_STEM long, so nothing was cut.
        let stem = stem_from_task("twenty characters ok please");
        assert_eq!(stem, "twenty-characters-ok");
    }

    #[test]
    fn a_task_with_nothing_usable_falls_back() {
        assert_eq!(stem_from_task(""), FALLBACK_STEM);
        assert_eq!(stem_from_task("!!! ???"), FALLBACK_STEM);
        assert_eq!(stem_from_task("—"), FALLBACK_STEM);
    }

    #[test]
    fn non_ascii_text_still_yields_a_legal_stem() {
        assert!(legal_charset(&stem_from_task("café ☕ time")));
        assert!(legal_charset(&stem_from_task("修复登录问题")));
    }

    #[test]
    fn a_generated_id_is_its_stem_plus_a_suffix() {
        let root = TempDir::new().unwrap();
        let id = generate("Fix the login bug", root.path()).unwrap();
        assert!(legal_charset(&id));
        let (stem, suffix) = id.rsplit_once('-').unwrap();
        assert_eq!(stem, "fix-the-login-bug");
        assert_eq!(suffix.len(), SUFFIX_LEN);
    }

    #[test]
    fn a_generated_id_avoids_directories_that_already_exist() {
        let root = TempDir::new().unwrap();
        let first = generate("same task", root.path()).unwrap();
        fs::create_dir_all(root.path().join(&first)).unwrap();
        let second = generate("same task", root.path()).unwrap();
        assert_ne!(first, second);
        assert!(second.starts_with("same-task-"));
    }

    #[test]
    fn ids_that_could_address_another_directory_are_not_ids() {
        for bad in [
            "",
            "..",
            "../../elsewhere",
            "/etc/passwd",
            "with/slash",
            "Uppercase",
            "with space",
            "under_score",
            "esc\u{1b}[2J",
        ] {
            assert!(!is_valid(bad), "{bad:?} must not pass as an id");
        }
    }

    #[test]
    fn a_minted_id_passes_the_charset_law() {
        let root = TempDir::new().unwrap();
        assert!(is_valid(
            &generate("Fix the login bug", root.path()).unwrap()
        ));
        assert!(is_valid("agent-7f3"));
    }

    #[test]
    fn a_name_obeys_the_charset_and_uniqueness_but_not_the_cap() {
        let root = TempDir::new().unwrap();
        assert!(validate_name("nightly-import-rewrite-and-then-some", root.path()).is_ok());
        assert!(validate_name("", root.path()).is_err());
        assert!(validate_name("Has Caps", root.path()).is_err());
        assert!(validate_name("../escape", root.path()).is_err());

        fs::create_dir_all(root.path().join("taken")).unwrap();
        assert!(validate_name("taken", root.path()).is_err());
    }
}
