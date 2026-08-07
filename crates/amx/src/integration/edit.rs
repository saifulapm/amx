//! Reading and writing an agent's configuration file without disturbing it.
//!
//! The rule V10 is held to is "preserve foreign entries byte for byte; only
//! amx-owned entries are ever touched". A `serde_json::Value` round trip cannot
//! keep that promise: `serde_json::Map` is a `BTreeMap` unless the crate is
//! built with `preserve_order`, so writing a settings file back would
//! alphabetise every key in it — a diff amx is not entitled to make in a file
//! it does not own, and one the user would find the next time their own tool
//! wrote to it. Enabling `preserve_order` workspace-wide is not the fix either:
//! it would reorder every `json!` literal in the tree, including the ones the
//! golden suites of other tasks compare byte for byte.
//!
//! So this module has its own document type, [`Json`], whose objects are a
//! `Vec` of pairs. It is not a JSON parser — `serde_json` still does the
//! parsing and the printing; the only thing added is a `Deserialize` that
//! keeps the order its visitor was handed the keys in, and a `Serialize` that
//! writes them back the same way. Foreign keys keep their positions, foreign
//! values keep their shapes, and a re-read of the file yields the same document
//! it did before.
//!
//! What is *not* preserved: whitespace, indentation and the exact spelling of
//! numbers, because the file is reprinted from the parse. That is the honest
//! boundary of this approach, and it is why nothing here ever writes a file
//! whose parsed content it did not change — see [`Plan::install`].
//!
//! [`Plan::install`]: super::Plan::install

use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq as _, Serializer};
use serde::{Deserialize, Serialize};

/// A JSON document that remembers the order its keys arrived in.
#[derive(Clone, PartialEq, Debug)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// Any JSON number, carried in `serde_json`'s own representation so an
    /// integer stays an integer on the way back out.
    Number(serde_json::Number),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<Json>),
    /// An object, in document order.
    Object(Object),
}

impl Json {
    /// An empty object — what a configuration file amx is about to create is.
    #[must_use]
    pub fn object() -> Self {
        Self::Object(Object::default())
    }

    /// This value as an object, or `None` if it is something else.
    #[must_use]
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    /// See [`as_object`](Self::as_object).
    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    /// This value as an array, or `None` if it is something else.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// See [`as_array`](Self::as_array).
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Json>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// This value as a string, or `None` if it is something else.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }
}

/// A JSON object, as an ordered list of members.
///
/// Duplicate keys never reach it: [`Json`]'s deserializer refuses a document
/// that has any, for the same reason Claude Code's own settings validator does
/// — with two `"hooks"` keys, "the hooks amx installed" and "the hooks the
/// agent reads" are different objects, and an installer that silently picked
/// one would be reporting on a file it had not really edited.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Object(Vec<(String, Json)>);

impl Object {
    /// The value stored under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// See [`get`](Self::get).
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        self.0
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// The value under `key`, inserting `default` at the end if it is absent.
    ///
    /// Appending rather than sorting is the whole point: a `hooks` key amx adds
    /// to a settings file lands after everything the user already had, where a
    /// reader looking for what changed will find it.
    pub fn entry(&mut self, key: &str, default: Json) -> &mut Json {
        if self.get(key).is_none() {
            self.0.push((key.to_owned(), default));
        }
        // Invariant: the key was just inserted if it was missing.
        match self.get_mut(key) {
            Some(value) => value,
            None => unreachable!("the key was inserted above"),
        }
    }

    /// Drop `key`, if it is there.
    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(name, _)| name != key);
    }

    /// Every member, in document order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Json)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }

    /// See [`iter`](Self::iter).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Json)> {
        self.0
            .iter_mut()
            .map(|(name, value)| (name.as_str(), value))
    }

    /// Every key, in document order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }

    /// Whether the object has no members.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(String, Json)> for Object {
    fn from_iter<I: IntoIterator<Item = (String, Json)>>(members: I) -> Self {
        Self(members.into_iter().collect())
    }
}

// ------------------------------------------------------------------ the wire

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Number(value) => value.serialize(serializer),
            Self::String(value) => serializer.serialize_str(value),
            Self::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Object(object) => {
                let mut map = serializer.serialize_map(Some(object.0.len()))?;
                for (key, value) in &object.0 {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

/// The visitor that keeps the order.
struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Json, E> {
        serde_json::Number::from_f64(value)
            .map(Json::Number)
            .ok_or_else(|| de::Error::custom("a number JSON cannot represent"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Json, A::Error> {
        let mut items = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some(item) = access.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Json, A::Error> {
        let mut members: Vec<(String, Json)> = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some((key, value)) = access.next_entry::<String, Json>()? {
            if members.iter().any(|(held, _)| *held == key) {
                return Err(de::Error::custom(format!("duplicate key `{key}`")));
            }
            members.push((key, value));
        }
        Ok(Json::Object(Object(members)))
    }
}

// ------------------------------------------------------------------ the disk

/// Read `path`, or `Ok(None)` if there is no file there yet.
///
/// A file that exists and does not parse is an error, never an empty document:
/// overwriting a configuration amx could not read would destroy exactly the
/// settings the user cared enough to hand-write.
pub fn load(path: &Path) -> anyhow::Result<Option<Json>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context(format!("read {}", path.display())),
    };
    // An empty file is what `touch` leaves behind, and several agents ship one.
    // It is not a parse failure; it is a document with nothing in it.
    if text.trim().is_empty() {
        return Ok(Some(Json::object()));
    }
    let document: Json = serde_json::from_str(&text)
        .with_context(|| format!("{} is not JSON amx can edit", path.display()))?;
    if document.as_object().is_none() {
        bail!(
            "{} holds a JSON {}, not an object; amx will not rewrite it",
            path.display(),
            kind_of(&document)
        );
    }
    Ok(Some(document))
}

/// Write `document` to `path`, atomically.
///
/// Two details that are not decoration:
///
/// - **Symlinks are followed.** Claude Code's own guidance for editing its
///   settings says so outright ("If `~/.claude/settings.json` is a symlink,
///   update the target file instead"), and a rename over the link would replace
///   a user's link into their dotfiles repository with a plain file.
/// - **The temporary file is in the destination's directory**, so the rename
///   that publishes it is atomic; a temporary elsewhere would cross a
///   filesystem boundary and degrade to a copy, which is the half-written
///   configuration this is here to avoid.
pub fn store(path: &Path, document: &Json) -> anyhow::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(document).context("render the configuration")?
    );

    let temporary = temporary_beside(&target);
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, &target).with_context(|| {
        let _ = fs::remove_file(&temporary);
        format!("install {}", target.display())
    })?;
    Ok(())
}

/// A scratch path in `target`'s directory that no other amx will pick.
fn temporary_beside(target: &Path) -> PathBuf {
    let name = target.file_name().map_or_else(
        || "config".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    // The pid is process state, but it is the process's own identity rather
    // than its environment: `Env`'s read-once rule is about configuration, and
    // a scratch name has none.
    let scratch = format!(".{name}.amx-{}", std::process::id());
    target.with_file_name(scratch)
}

/// What a document is, for a message that has to say why it was refused.
fn kind_of(document: &Json) -> &'static str {
    match document {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

    use super::{Json, Object};

    fn parse(text: &str) -> Json {
        serde_json::from_str(text).expect("parse")
    }

    #[test]
    fn a_document_round_trips_with_its_key_order_intact() {
        let text = r#"{"zeta":1,"alpha":{"nested":true,"a":[1,2.5,"three",null]},"mid":"x"}"#;
        let document = parse(text);
        assert_eq!(serde_json::to_string(&document).unwrap(), text);
    }

    #[test]
    fn a_duplicate_key_is_refused_rather_than_silently_collapsed() {
        let err = serde_json::from_str::<Json>(r#"{"hooks":{},"hooks":{}}"#)
            .expect_err("duplicate keys are rejected");
        assert!(err.to_string().contains("duplicate key `hooks`"), "{err}");
    }

    #[test]
    fn entry_appends_a_new_key_after_the_ones_already_there() {
        let mut document = parse(r#"{"model":"opus","permissions":{}}"#);
        let object = document.as_object_mut().expect("an object");
        object.entry("hooks", Json::object());
        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            ["model", "permissions", "hooks"]
        );
    }

    #[test]
    fn an_object_collected_from_pairs_keeps_the_order_it_was_given() {
        let object: Object = [
            ("b".to_owned(), Json::Bool(true)),
            ("a".to_owned(), Json::Null),
        ]
        .into_iter()
        .collect();
        assert_eq!(object.keys().collect::<Vec<_>>(), ["b", "a"]);
    }
}
