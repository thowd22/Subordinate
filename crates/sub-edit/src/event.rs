//! The change events the engine broadcasts.
//!
//! Every command the engine applies produces one [`ChangeEvent`]: what kind of
//! entity changed, which one, and how. Subscribers — the UI, the Command API's
//! event subscriptions, the MCP bridge — use them to invalidate exactly what
//! they cache instead of rebuilding a whole panel per edit (docs/PLAN.md §4).
//!
//! The event is derived from the command's [`CommandEnvelope`], not from a
//! per-command implementation, so a command kind added by a plugin describes
//! itself the same way a built-in one does. The derivation is mechanical and
//! relies on two conventions the built-in command set already keeps:
//!
//! - a kind is `<entity>.<verb>`, so `clip.trim_in` touches a clip;
//! - the parameter naming the entity is called after it — `clip`, `track`,
//!   `bin`, `media` — and is either the identifier itself or the whole entity
//!   with its `id` inside.
//!
//! ```
//! use sub_edit::{ChangeEvent, ChangeOrigin, ChangeType, CommandEnvelope, EntityKind};
//!
//! let envelope = CommandEnvelope::new(
//!     "track.rename",
//!     serde_json::json!({
//!         "sequence": "0199e1c0-0000-7000-8000-000000000001",
//!         "track": "0199e1c0-0000-7000-8000-000000000002",
//!         "name": "V2",
//!     }),
//! );
//! let event = ChangeEvent::from_envelope(7, &envelope, ChangeOrigin::Apply);
//! assert_eq!(event.entity, EntityKind::TRACK);
//! assert_eq!(event.change, ChangeType::Modified);
//! assert_eq!(
//!     event.id.as_deref(),
//!     Some("0199e1c0-0000-7000-8000-000000000002")
//! );
//! ```

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::command::CommandEnvelope;

/// The kind of entity a change touched, such as `clip` or `track`.
///
/// It is a string newtype rather than a closed enum because plugin command
/// kinds carry entity names this build does not know; the constants cover the
/// built-in model. Like an error code, an existing name is never given a new
/// meaning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityKind(Cow<'static, str>);

impl EntityKind {
    /// The project itself: its name and top-level settings.
    pub const PROJECT: Self = Self::from_static("project");
    /// A sequence (timeline).
    pub const SEQUENCE: Self = Self::from_static("sequence");
    /// A track of a sequence.
    pub const TRACK: Self = Self::from_static("track");
    /// A clip on a track.
    pub const CLIP: Self = Self::from_static("clip");
    /// A marker on a sequence or a clip.
    pub const MARKER: Self = Self::from_static("marker");
    /// A media item in the project's media list.
    pub const MEDIA: Self = Self::from_static("media");
    /// A bin in the media bin tree.
    pub const BIN: Self = Self::from_static("bin");
    /// An edit that restores several entities at once, such as the inverse of
    /// a ripple delete.
    pub const EDIT: Self = Self::from_static("edit");

    /// Builds a kind from a literal.
    #[must_use]
    pub const fn from_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// Builds a kind from a name produced at runtime, such as a plugin
    /// command's domain.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(Cow::Owned(name.into()))
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What happened to the entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    /// The entity now exists and did not before.
    Added,
    /// The entity no longer exists.
    Removed,
    /// The entity still exists with different contents.
    Modified,
}

impl ChangeType {
    /// The change an undo of this change performs: an add becomes a removal
    /// and the other way round; a modification stays one.
    #[must_use]
    pub const fn inverted(self) -> Self {
        match self {
            Self::Added => Self::Removed,
            Self::Removed => Self::Added,
            Self::Modified => Self::Modified,
        }
    }
}

/// Why the engine applied the command that produced the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOrigin {
    /// A command submitted by a client.
    Apply,
    /// A history step being undone.
    Undo,
    /// A history step being redone.
    Redo,
}

/// One change to one entity, broadcast after the engine applied it.
///
/// The JSON shape is stable: it is what the Command API's event subscriptions
/// send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// The revision the project reached; every event of one engine operation
    /// carries the same revision, and revisions only ever increase.
    pub revision: u64,
    /// The kind of entity that changed.
    pub entity: EntityKind,
    /// The identifier of the entity, when the command names one. Commands that
    /// mint an identifier — `track.add`, `sequence.create` — do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What happened to it.
    pub change: ChangeType,
    /// The stable kind of the command responsible, such as `clip.trim_in`.
    pub command: String,
    /// Whether the command was applied, undone or redone.
    pub origin: ChangeOrigin,
}

impl ChangeEvent {
    /// Derives the event a command envelope describes.
    ///
    /// `origin` only labels the event; the caller inverts the change type for
    /// an undo (see [`ChangeEvent::inverted`]), because it is the caller that
    /// knows which direction the history moved.
    #[must_use]
    pub fn from_envelope(revision: u64, envelope: &CommandEnvelope, origin: ChangeOrigin) -> Self {
        let (entity, verb) = split_kind(&envelope.kind);
        Self {
            revision,
            entity: EntityKind::new(entity),
            id: entity_id(entity, &envelope.params),
            change: change_type(verb),
            command: envelope.kind.clone(),
            origin,
        }
    }

    /// The same event with its change type inverted, used when the history
    /// walks backwards.
    #[must_use]
    pub fn inverted(mut self) -> Self {
        self.change = self.change.inverted();
        self
    }
}

/// Splits `clip.trim_in` into `("clip", "trim_in")`. A kind without a dot is
/// all entity and no verb, which reads as a modification.
fn split_kind(kind: &str) -> (&str, &str) {
    kind.split_once('.').unwrap_or((kind, ""))
}

/// Maps a command verb to what it does to the entity.
///
/// Creation verbs (`add`, `insert`, `create`, `import`, and `split`, which
/// mints the tail clip) add, `remove` and `delete` verbs remove, everything
/// else modifies.
fn change_type(verb: &str) -> ChangeType {
    let head = verb.split('_').next().unwrap_or(verb);
    match head {
        "add" | "insert" | "create" | "import" | "split" => ChangeType::Added,
        "remove" | "delete" => ChangeType::Removed,
        _ => ChangeType::Modified,
    }
}

/// Finds the identifier of the entity `entity` in a command's parameters.
///
/// Tries the field named after the entity, then `<entity>_id`, then the
/// generic `id` and `item` (`media.import` carries the whole item). A field
/// holding the entity itself is looked into for its `id`.
fn entity_id(entity: &str, params: &Value) -> Option<String> {
    let object = params.as_object()?;
    let owned = format!("{entity}_id");
    for field in [entity, owned.as_str(), "id", "item"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        if let Some(text) = value.as_str() {
            return Some(text.to_owned());
        }
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            return Some(id.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(kind: &str, params: Value) -> ChangeEvent {
        ChangeEvent::from_envelope(1, &CommandEnvelope::new(kind, params), ChangeOrigin::Apply)
    }

    #[test]
    fn the_entity_is_the_domain_of_the_command_kind() {
        assert_eq!(event("clip.trim_in", json!({})).entity, EntityKind::CLIP);
        assert_eq!(event("bin.rename", json!({})).entity, EntityKind::BIN);
        assert_eq!(
            event("plugin_thing.tweak", json!({})).entity,
            EntityKind::new("plugin_thing")
        );
    }

    #[test]
    fn verbs_map_to_change_types() {
        assert_eq!(event("track.add", json!({})).change, ChangeType::Added);
        assert_eq!(event("media.import", json!({})).change, ChangeType::Added);
        assert_eq!(event("clip.split", json!({})).change, ChangeType::Added);
        assert_eq!(event("track.remove", json!({})).change, ChangeType::Removed);
        assert_eq!(
            event("sequence.delete", json!({})).change,
            ChangeType::Removed
        );
        assert_eq!(
            event("track.set_muted", json!({})).change,
            ChangeType::Modified
        );
        assert_eq!(event("clip.move", json!({})).change, ChangeType::Modified);
        assert_eq!(event("mystery", json!({})).change, ChangeType::Modified);
    }

    #[test]
    fn the_id_comes_from_the_field_named_after_the_entity() {
        assert_eq!(
            event("clip.remove", json!({ "clip": "abc", "track": "def" })).id,
            Some("abc".to_owned())
        );
        assert_eq!(
            event(
                "clip.add",
                json!({ "track": "def", "clip": { "id": "ghi" } })
            )
            .id,
            Some("ghi".to_owned())
        );
        assert_eq!(
            event("media.import", json!({ "item": { "id": "jkl" } })).id,
            Some("jkl".to_owned())
        );
        assert_eq!(
            event("marker.add", json!({ "marker_id": "mno" })).id,
            Some("mno".to_owned())
        );
        assert_eq!(event("track.add", json!({ "name": "V2" })).id, None);
        assert_eq!(event("track.add", json!("not an object")).id, None);
    }

    #[test]
    fn undoing_a_change_inverts_it() {
        assert_eq!(ChangeType::Added.inverted(), ChangeType::Removed);
        assert_eq!(ChangeType::Removed.inverted(), ChangeType::Added);
        assert_eq!(ChangeType::Modified.inverted(), ChangeType::Modified);
        let undone = event("track.add", json!({})).inverted();
        assert_eq!(undone.change, ChangeType::Removed);
    }

    #[test]
    fn events_serialise_to_the_documented_shape() {
        let event = event("track.remove", json!({ "track": "abc" }));
        let text = serde_json::to_value(&event).unwrap();
        assert_eq!(
            text,
            json!({
                "revision": 1,
                "entity": "track",
                "id": "abc",
                "change": "removed",
                "command": "track.remove",
                "origin": "apply",
            })
        );
    }
}
