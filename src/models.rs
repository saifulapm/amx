//! Which models a harness runs.
//!
//! A model can name the harness it belongs to only because each harness can be
//! asked what it offers, and there are three places an answer comes from: the
//! table somebody wrote for it, the model dial's own cycle, and the listing the
//! vendor prints when it is asked for one. Nothing here knows a vendor's name.
//! Which of the three a harness answers out of is the entry's to say, and the
//! file's list is over both, because a person who has written down which models
//! are this harness's has said so.

use crate::config::Config;
use crate::paths;
use crate::vendor::{DEFAULT, Models, Vendor};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// How long a listing read out of a vendor stands before it is read again.
///
/// Reading one starts the vendor's own program, and that is a second of the
/// spawn somebody is waiting on. What a provider offers changes over days, so
/// an hour is soon enough that a model added this morning is found this
/// afternoon, and long enough that a person spawning all day pays for it once.
const FRESH_FOR: Duration = Duration::from_secs(3600);

/// Every model `vendor` is the one to run.
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
            kept_at(vendor.name).as_deref(),
            SystemTime::now(),
        ),
    }
}

/// Whether `word` names one of `list`.
///
/// Whole, or the part after the slash: a vendor that reaches several providers
/// writes a model as `provider/id`, and the id on its own is what somebody
/// types. Never a part of a word — a model whose name merely contains another
/// is a different model.
pub fn lists_model(list: &[String], word: &str) -> bool {
    list.iter()
        .any(|model| model == word || model.rsplit_once('/').is_some_and(|(_, id)| id == word))
}

/// The model dial's own cycle, less the sentinel.
///
/// A vendor that prints no list offers what a key already cycles through, and
/// the sentinel is the absence of a model rather than one of them. A vendor
/// with no model dial at all offers nothing, and a word is never its.
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

/// Where a harness's listing is kept, or `None` when amx has nowhere to keep
/// one — which costs a reading and nothing else.
fn kept_at(name: &str) -> Option<PathBuf> {
    Some(paths::models_dir().ok()?.join(format!("{name}.txt")))
}

/// The listing `program` prints: what `cache` holds while that still stands,
/// and the program's own answer otherwise.
///
/// Empty from a program that could not be run, would not answer, or printed
/// nothing amx can read as a model. A harness nobody can be told about claims
/// no models, which leaves the word to the next harness or to a refusal naming
/// them all. Nothing is kept from such a run either, so a vendor installed a
/// minute later is asked again rather than held to an hour of silence.
fn printed(program: &str, argv: &[&str], cache: Option<&Path>, now: SystemTime) -> Vec<String> {
    if let Some(kept) = cache.and_then(|cache| fresh(cache, now)) {
        return kept;
    }
    let read = read_from(program, argv);
    if let Some(cache) = cache
        && !read.is_empty()
    {
        let _ = keep(cache, &read);
    }
    read
}

/// What `cache` holds, while it was written inside the hour.
///
/// A file written later than now is one this machine's clock disagrees with,
/// and reading the vendor again is the cheaper of the two mistakes.
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

/// Run the listing the entry names and read what it printed.
fn read_from(program: &str, argv: &[&str]) -> Vec<String> {
    let listing = Command::new(program)
        .args(argv)
        // A listing is read, never talked to. A vendor handed the terminal
        // could sit there waiting on somebody who is waiting on it.
        .stdin(Stdio::null())
        .output();
    match listing {
        Ok(listing) if listing.status.success() => rows(&String::from_utf8_lossy(&listing.stdout)),
        _ => Vec::new(),
    }
}

/// The models a listing names, as `provider/id`.
///
/// A header line, then a row per model whose first two columns are the provider
/// and the id the model has there. What the columns past those say is what the
/// vendor knows about the model rather than its name.
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

/// Write a listing down where the next spawn will find it.
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

    /// A config whose file says which models one harness runs.
    fn told(harness: &str, models: &[&str]) -> Config {
        Config {
            harnesses: BTreeMap::from([(
                harness.to_string(),
                HarnessConfig {
                    models: models.iter().map(|model| model.to_string()).collect(),
                    args: Vec::new(),
                },
            )]),
            ..Config::default()
        }
    }

    /// The entry for a harness these tests are about.
    fn entry(name: &str) -> &'static Vendor {
        registry::entry(name).unwrap_or_else(|| panic!("an entry for {name}"))
    }

    /// A stand-in for a vendor that prints its models: the listing it was given
    /// when it is asked the way the entry says, and a refusal otherwise.
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

    /// Two models under a header, the way a vendor prints them.
    const LISTING: &str = "provider  model          context\n\
                           openai     gpt-5          400K\n\
                           anthropic  claude-opus-5  1M\n";

    #[test]
    fn a_harness_runs_the_models_its_own_table_names() {
        // The file is over the entry: somebody who has written the list down
        // has said which models are this harness's, whatever it offers.
        let config = told("claude", &["only-this-one"]);
        assert_eq!(models_of(entry("claude"), &config), ["only-this-one"]);
    }

    #[test]
    fn a_harness_with_no_table_runs_the_models_its_dial_cycles() {
        // And the sentinel is not one of them: it is the word for passing no
        // model at all, so no listing may hold it.
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

        let list = printed(&vendor, &["--list-models"], Some(&cache), SystemTime::now());

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
        assert!(cache.exists(), "and it is kept for the next spawn");
    }

    #[test]
    fn a_listing_read_inside_the_hour_is_the_one_already_kept() {
        // The point of keeping it: the vendor is a process, and the second
        // spawn of the morning should not pay for one. The program is asked
        // the wrong way here, so an answer from it would be no answer at all.
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);
        let cache = dir.path().join("models/pi.txt");
        printed(&vendor, &["--list-models"], Some(&cache), SystemTime::now());

        let list = printed(&vendor, &["--wrong"], Some(&cache), SystemTime::now());

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
    }

    #[test]
    fn a_listing_older_than_an_hour_is_read_again() {
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);
        let cache = dir.path().join("models/pi.txt");
        keep(&cache, &["openai/gpt-4".to_string()]).unwrap();

        let later = SystemTime::now() + FRESH_FOR + Duration::from_secs(1);
        let list = printed(&vendor, &["--list-models"], Some(&cache), later);

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
        // Three ways for it to go wrong, one answer: a harness that claims no
        // models, and no silence written down for the hour after it.
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
                Some(&cache),
                now,
            ),
            printed(&vendor, &["--every-model"], Some(&cache), now),
            printed(&says_nothing, &["--list-models"], Some(&cache), now),
        ] {
            assert!(asked.is_empty(), "{asked:?}");
        }
        assert!(!cache.exists(), "nothing worth keeping was read");
    }

    #[test]
    fn a_harness_with_nowhere_to_keep_a_listing_reads_it_every_time() {
        let dir = TempDir::new().unwrap();
        let vendor = a_vendor_printing(dir.path(), "prints-models", LISTING);

        let list = printed(&vendor, &["--list-models"], None, SystemTime::now());

        assert_eq!(list, ["openai/gpt-5", "anthropic/claude-opus-5"]);
    }
}
