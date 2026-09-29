//! The models each harness offers.
//!
//! A harness's list comes from the person's config if it names one, else from
//! the vendor entry: the model dial's cycle, or a listing printed by the
//! vendor's own program (plain rows or JSON). Nothing here names a vendor.

use crate::config::Config;
use crate::paths;
use crate::vendor::{DEFAULT, Models, Vendor};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// How long a listing read from a vendor is reused.
///
/// Reading one runs the vendor's program, which costs about a second of a
/// spawn. Provider model lists change over days, so an hour is fresh enough.
const FRESH_FOR: Duration = Duration::from_secs(3600);

/// Every model `vendor` offers.
pub fn models_of(vendor: &Vendor, config: &Config) -> Vec<String> {
    let written = config.harness(vendor.name).models;
    if !written.is_empty() {
        return written;
    }
    match vendor.models {
        Models::Cycle => cycle(vendor),
        Models::Printed(argv) => printed(
            vendor.name,
            argv,
            rows,
            kept_at(vendor.name).as_deref(),
            SystemTime::now(),
        ),
        Models::Json(argv) => printed(
            vendor.name,
            argv,
            slugs,
            kept_at(vendor.name).as_deref(),
            SystemTime::now(),
        ),
    }
}

/// Whether `word` names one of `list`.
///
/// Matches the whole name or the part after the slash, since multi-provider
/// vendors write `provider/id` and people type the id. Substrings never match.
pub fn lists_model(list: &[String], word: &str) -> bool {
    list.iter()
        .any(|model| model == word || model.rsplit_once('/').is_some_and(|(_, id)| id == word))
}

/// The model dial's cycle without the default sentinel. A vendor with no model
/// dial offers nothing.
fn cycle(vendor: &Vendor) -> Vec<String> {
    let Some(dial) = vendor.model else {
        return Vec::new();
    };
    dial.cycle
        .iter()
        .filter(|word| **word != DEFAULT)
        .map(|word| word.to_string())
        .collect()
}

/// Where a harness's listing is cached, or `None` if there is nowhere to keep
/// one.
fn kept_at(name: &str) -> Option<PathBuf> {
    Some(paths::models_dir().ok()?.join(format!("{name}.txt")))
}

/// The listing `program` prints, from `cache` while it is fresh, otherwise by
/// running the program and parsing its output with `read`.
///
/// Empty when the program cannot run, fails, or prints nothing parseable. An
/// empty listing is not cached, so a vendor installed a minute later is found.
fn printed(
    program: &str,
    argv: &[&str],
    read: fn(&str) -> Vec<String>,
    cache: Option<&Path>,
    now: SystemTime,
) -> Vec<String> {
    if let Some(kept) = cache.and_then(|cache| fresh(cache, now)) {
        return kept;
    }
    let read = read_from(program, argv, read);
    if let Some(cache) = cache
        && !read.is_empty()
    {
        let _ = keep(cache, &read);
    }
    read
}

/// The cached listing, if written within [`FRESH_FOR`]. A file dated in the
/// future is treated as stale.
fn fresh(cache: &Path, now: SystemTime) -> Option<Vec<String>> {
    let written = std::fs::metadata(cache).ok()?.modified().ok()?;
    if now.duration_since(written).ok()? > FRESH_FOR {
        return None;
    }
    let kept: Vec<String> = std::fs::read_to_string(cache)
        .ok()?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

/// Run the listing command and parse what it printed.
///
/// Retried once on `ETXTBSY`: the program may still be open for writing, by a
/// package manager installing it or by a child another thread forked before
/// exec. Without the retry, that moment would cache an empty list for an hour.
fn read_from(program: &str, argv: &[&str], read: fn(&str) -> Vec<String>) -> Vec<String> {
    let run = || {
        Command::new(program)
            .args(argv)
            // Never give the vendor the terminal; it could wait on input.
            .stdin(Stdio::null())
            .output()
    };
    let mut listing = run();
    if listing
        .as_ref()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::ExecutableFileBusy)
    {
        std::thread::sleep(Duration::from_millis(20));
        listing = run();
    }
    match listing {
        Ok(listing) if listing.status.success() => read(&String::from_utf8_lossy(&listing.stdout)),
        _ => Vec::new(),
    }
}

/// Models from a plain listing, as `provider/id`: a header line, then one row
/// per model whose first two columns are provider and id.
fn rows(listing: &str) -> Vec<String> {
    listing
        .lines()
        .skip(1)
        .filter_map(|row| {
            let mut columns = row.split_whitespace();
            let provider = columns.next()?;
            let model = columns.next()?;
            Some(format!("{provider}/{model}"))
        })
        .collect()
}

/// Models from a JSON listing: the `slug` of each entry in `models` whose
/// `visibility` is `list`, in order. Hidden models are left out, and anything
/// that does not parse as that shape yields nothing.
fn slugs(listing: &str) -> Vec<String> {
    let Ok(listing) = serde_json::from_str::<serde_json::Value>(listing) else {
        return Vec::new();
    };
    listing["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| model["visibility"] == "list")
        .filter_map(|model| model["slug"].as_str())
        .map(str::to_string)
        .collect()
}

/// Cache a listing for the next spawn.
fn keep(cache: &Path, list: &[String]) -> std::io::Result<()> {
    if let Some(dir) = cache.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(cache, list.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HarnessConfig;
    use crate::registry;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// A config naming one harness's models.
    fn told(harness: &str, models: &[&str]) -> Config {
        Config {
            harnesses: BTreeMap::from([(
                harness.to_string(),
                HarnessConfig {
                    models: models.iter().map(|model| model.to_string()).collect(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            )]),
            ..Config::default()
        }
    }

    /// The registry entry for a harness.
    fn entry(name: &str) -> &'static Vendor {
        registry::entry(name).unwrap_or_else(|| panic!("an entry for {name}"))
    }

    /// A stand-in vendor that prints `listing` when called with
    /// `--list-models` and fails otherwise.
    fn a_vendor_printing(dir: &Path, name: &str, listing: &str) -> String {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"$1\" = \"--list-models\" ] || exit 3\ncat <<'LISTING'\n{listing}LISTING\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// Two models under a header, as a vendor prints them.
    const LISTING: &str = "provider  model          context\n\
                           openai     gpt-5          400K\n\
                           anthropic  claude-opus-5  1M\n";

    #[test]
    fn a_harness_runs_the_models_its_own_table_names() {
        // The config's list wins over whatever the vendor offers.
        let config = told("claude", &["only-this-one"]);
        assert_eq!(models_of(entry("claude"), &config), ["only-this-one"]);
    }

    #[test]
    fn a_harness_with_no_table_runs_the_models_its_dial_cycles() {
        // The sentinel means "no model flag", so it is never listed.
        let list = models_of(entry("claude"), &Config::default());
        assert!(list.contains(&"haiku".to_string()), "{list:?}");
        assert!(!list.iter().any(|model| model == DEFAULT), "{list:?}");
        assert_eq!(
            list.len(),
            entry("claude").model.unwrap().cycle.len() - 1,
            "the cycle less the sentinel and nothing else: {list:?}"
        );
    }

    #[test]
    fn a_word_matches_a_model_whole_or_after_its_providers_slash() {
        let list = vec!["openai/gpt-5".to_string(), "haiku".to_string()];
        assert!(lists_model(&list, "openai/gpt-5"));
        assert!(lists_model(&list, "gpt-5"), "the id a person types");
        assert!(lists_model(&list, "haiku"));
        assert!(!lists_model(&list, "openai"), "the provider is not a model");
        assert!(!lists_model(&list, "gpt"), "nor is part of a name");
        assert!(!lists_model(&list, "gpt-5-mini"));
        assert!(!lists_model(&[], "haiku"));
    }

    #[test]
    fn a_printed_listing_is_read_past_its_header_as_a_provider_and_an_id() {
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);
        let cache = dir.path().join("models/pi.txt");

        let list = printed(
            &vendor,
            &["--list-models"],
            rows,
            Some(&cache),
            SystemTime::now(),
        );

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
        assert!(cache.exists(), "and it is kept for the next spawn");
    }

    #[test]
    fn a_listing_read_inside_the_hour_is_the_one_already_kept() {
        // The second call asks the wrong way, so only the cache can answer.
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);
        let cache = dir.path().join("models/pi.txt");
        printed(
            &vendor,
            &["--list-models"],
            rows,
            Some(&cache),
            SystemTime::now(),
        );

        let list = printed(&vendor, &["--wrong"], rows, Some(&cache), SystemTime::now());

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
    }

    #[test]
    fn a_listing_older_than_an_hour_is_read_again() {
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);
        let cache = dir.path().join("models/pi.txt");
        keep(&cache, &["openai/gpt-4".to_string()]).unwrap();

        let later = SystemTime::now() + FRESH_FOR + Duration::from_secs(1);
        let list = printed(&vendor, &["--list-models"], rows, Some(&cache), later);

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
        assert_eq!(
            fresh(&cache, SystemTime::now()),
            Some(vec![
                "openai/gpt-5".to_string(),
                "anthropic/claude-opus-5".to_string()
            ]),
            "and what was read stands in place of what was there"
        );
    }

    #[test]
    fn a_listing_nobody_could_read_is_no_models_and_nothing_kept() {
        // Every failure yields no models and caches nothing.
        let dir = TempDir::new().unwrap();
        let cache = dir.path().join("models/pi.txt");
        let now = SystemTime::now();

        let missing = dir.path().join("no-such-vendor");
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);
        let says_nothing = a_vendor_printing(dir.path(), "prints-nothing", "");

        for asked in [
            printed(
                &missing.to_string_lossy(),
                &["--list-models"],
                rows,
                Some(&cache),
                now,
            ),
            printed(&vendor, &["--every-model"], rows, Some(&cache), now),
            printed(&says_nothing, &["--list-models"], rows, Some(&cache), now),
        ] {
            assert!(asked.is_empty(), "{asked:?}");
        }
        assert!(!cache.exists(), "nothing worth keeping was read");
    }

    /// Models as codex 0.157.1's `debug models` prints them, trimmed: two
    /// listed, one hidden.
    const JSON: &str = r#"{"models":[{"slug":"gpt-6-astra","display_name":"GPT-6-Astra","visibility":"list","priority":1},{"slug":"gpt-5.5","display_name":"GPT-5.5","visibility":"list","priority":9},{"slug":"codex-auto-review","display_name":"Codex Auto Review","visibility":"hide","priority":30}]}"#;

    #[test]
    fn a_json_listing_offers_the_slugs_it_lists_and_not_the_ones_it_hides() {
        assert_eq!(slugs(JSON), ["gpt-6-astra", "gpt-5.5"]);
        for malformed in ["", "not json", "{}", r#"{"models":{}}"#, "[1,2]"] {
            assert!(slugs(malformed).is_empty(), "{malformed:?}");
        }
        assert_eq!(
            slugs(r#"{"models":[{"visibility":"list"},{"slug":"x","visibility":"list"}]}"#),
            ["x"],
            "a model with no slug is no model"
        );
    }

    #[test]
    fn a_json_listing_is_asked_for_by_its_words_and_kept_like_a_printed_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prints-json");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"$1 $2\" = \"debug models\" ] || exit 3\ncat <<'LISTING'\n{JSON}\nLISTING\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vendor = path.to_string_lossy().into_owned();
        let cache = dir.path().join("models/codex.txt");

        let list = printed(
            &vendor,
            &["debug", "models"],
            slugs,
            Some(&cache),
            SystemTime::now(),
        );

        assert_eq!(list, ["gpt-6-astra", "gpt-5.5"]);
        assert_eq!(
            printed(
                &vendor,
                &["--wrong"],
                slugs,
                Some(&cache),
                SystemTime::now()
            ),
            ["gpt-6-astra", "gpt-5.5"],
            "and read inside the hour out of what was kept"
        );
    }

    #[test]
    fn a_harness_with_nowhere_to_keep_a_listing_reads_it_every_time() {
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);

        let list = printed(&vendor, &["--list-models"], rows, None, SystemTime::now());

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
    }
}
