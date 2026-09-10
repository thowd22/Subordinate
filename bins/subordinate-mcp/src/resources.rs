//! MCP resources: the project as something an agent can simply read.
//!
//! A tool call is an action; a resource is a *thing*, and a client may attach
//! one to a conversation, cache it, and be told when it goes stale. The bridge
//! publishes three (docs/PLAN.md §7):
//!
//! - `project://current` — the whole open project, with the revision it was
//!   read at;
//! - `sequence://{id}` — one timeline;
//! - `media://{id}` — one media item.
//!
//! All three are the project file's own JSON, the OTIO-shaped model
//! `sub_model` serialises, taken out of one `project.get` round trip. There is
//! no second representation to keep in step: a resource is a projection of the
//! project, and the revision travels with it so a reader can tell two reads
//! apart.
//!
//! Every list and every read carries `ttlMs` and `cacheScope`, the caching
//! hints of the 2026-07-28 specification (SEP-2549). The times are short
//! because the project is edited underneath them, and a client that subscribes
//! ([`crate::watch`]) learns about a change long before the time is up.

use std::collections::BTreeSet;

use rmcp::model::{
    CacheScope, ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
    SubscriptionFilter,
};
use serde_json::{Value, json};
use sub_core::{SubError, SubResult};

use crate::backend::Backend;
use crate::codes;

/// The one project the editor has open.
pub const PROJECT_URI: &str = "project://current";
/// The URI scheme of a single sequence.
pub const SEQUENCE_SCHEME: &str = "sequence";
/// The URI scheme of a single media item.
pub const MEDIA_SCHEME: &str = "media";
/// What every resource this bridge serves is made of.
pub const MIME_TYPE: &str = "application/json";

/// How long a resource read may be treated as fresh, in milliseconds.
///
/// The project changes whenever the user or an agent edits it, so the hint is
/// deliberately short: it bounds how stale an unsubscribed reader can be,
/// while still collapsing the burst of reads a client makes when it first
/// attaches the project to a conversation.
pub const RESOURCE_TTL_MS: u64 = 5_000;

/// How long the *set* of resources may be treated as fresh, in milliseconds.
///
/// Sequences and media items come and go far more rarely than their contents
/// change, and a subscriber is told when they do.
pub const LIST_TTL_MS: u64 = 30_000;

/// How long the tool list may be treated as fresh, in milliseconds.
///
/// The tools are generated from a schema compiled into this binary, so they
/// cannot change while the process runs; the hint is an hour rather than
/// forever only so a client that keeps a session open across an upgrade
/// eventually looks again.
pub const TOOLS_TTL_MS: u64 = 3_600_000;

/// Who may cache a result made out of the user's project.
///
/// Project contents are the user's own, so only the requesting client may keep
/// them; the tool list, which is the same for every user of a build, is
/// [`CacheScope::Public`].
pub const PROJECT_CACHE_SCOPE: CacheScope = CacheScope::Private;

/// What a resource URI names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// The whole project: `project://current`.
    Project,
    /// One sequence, by identifier.
    Sequence(String),
    /// One media item, by identifier.
    Media(String),
}

impl Target {
    /// The URI that names this target.
    #[must_use]
    pub fn uri(&self) -> String {
        match self {
            Self::Project => PROJECT_URI.to_owned(),
            Self::Sequence(id) => sequence_uri(id),
            Self::Media(id) => media_uri(id),
        }
    }
}

/// The URI of the sequence with `id`.
#[must_use]
pub fn sequence_uri(id: &str) -> String {
    format!("{SEQUENCE_SCHEME}://{id}")
}

/// The URI of the media item with `id`.
#[must_use]
pub fn media_uri(id: &str) -> String {
    format!("{MEDIA_SCHEME}://{id}")
}

/// What a URI names, or `None` when this bridge does not serve it.
///
/// Only `project://current` is accepted for the project: a project URI with
/// anything else after the scheme names some other project, which this bridge
/// has no way to read.
#[must_use]
pub fn parse_uri(uri: &str) -> Option<Target> {
    let (scheme, rest) = uri.split_once("://")?;
    if rest.is_empty() {
        return None;
    }
    match scheme {
        "project" if rest == "current" => Some(Target::Project),
        SEQUENCE_SCHEME => Some(Target::Sequence(rest.to_owned())),
        MEDIA_SCHEME => Some(Target::Media(rest.to_owned())),
        _ => None,
    }
}

/// The templates that describe the per-entity URIs.
///
/// `project://current` is a concrete resource and is listed rather than
/// templated; the other two are families whose members come and go with the
/// project.
#[must_use]
pub fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(format!("{SEQUENCE_SCHEME}://{{id}}"), "sequence")
            .with_title("Sequence")
            .with_description(
                "One sequence of the open project — its settings, tracks, clips and markers \
                 — as the project file stores it, with the revision it was read at.",
            )
            .with_mime_type(MIME_TYPE),
        ResourceTemplate::new(format!("{MEDIA_SCHEME}://{{id}}"), "media")
            .with_title("Media item")
            .with_description(
                "One media item of the open project — its path, hash, probe results and \
                 proxy state — with the revision it was read at.",
            )
            .with_mime_type(MIME_TYPE),
    ]
}

/// The part of a `subscriptions/listen` filter this bridge can honour.
///
/// Every resource it serves may be subscribed to; a URI it does not serve is
/// dropped from the filter rather than refused, and the acknowledgement tells
/// the client exactly what it got (the 2026-07-28 subscription stream).
#[must_use]
pub fn accepted_filter(requested: &SubscriptionFilter) -> SubscriptionFilter {
    let mut accepted = SubscriptionFilter::new();
    accepted.resources_list_changed = requested.resources_list_changed;
    accepted.resource_subscriptions = requested.resource_subscriptions.as_ref().map(|uris| {
        uris.iter()
            .filter(|uri| parse_uri(uri).is_some())
            .cloned()
            .collect()
    });
    accepted
}

/// The project as resources, read through the Command API.
#[derive(Debug, Clone)]
pub struct Resources {
    /// Where `project.get` is answered.
    backend: std::sync::Arc<Backend>,
}

impl Resources {
    /// Resources read from `backend`.
    #[must_use]
    pub fn new(backend: std::sync::Arc<Backend>) -> Self {
        Self { backend }
    }

    /// One `project.get` round trip: the revision and the whole project.
    ///
    /// # Errors
    ///
    /// Returns whatever the Command API returned.
    fn project(&self) -> SubResult<Value> {
        self.backend
            .invoke(sub_command::dispatch::PROJECT_GET, None)
    }

    /// Every resource the open project offers, in a stable order.
    ///
    /// # Errors
    ///
    /// Returns whatever `project.get` returned.
    pub fn list(&self) -> SubResult<Vec<Resource>> {
        let project = self.project()?;
        Ok(list_of(&project))
    }

    /// Reads one resource.
    ///
    /// # Errors
    ///
    /// Returns `mcp.unknown_resource` for a URI this bridge does not serve and
    /// for a sequence or media item the project does not have, and whatever
    /// `project.get` returned when the read itself fails.
    pub fn read(&self, uri: &str) -> SubResult<ReadResourceResult> {
        let target = parse_uri(uri).ok_or_else(|| unknown(uri))?;
        let answer = self.project()?;
        let contents = contents_of(&answer, &target).ok_or_else(|| unknown(uri))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(
                serde_json::to_string_pretty(&contents).unwrap_or_else(|_| contents.to_string()),
                uri,
            )
            .with_mime_type(MIME_TYPE),
        ])
        .with_ttl_ms(RESOURCE_TTL_MS)
        .with_cache_scope(PROJECT_CACHE_SCOPE))
    }
}

/// The error for a URI that names nothing this bridge can read.
fn unknown(uri: &str) -> SubError {
    SubError::new(codes::UNKNOWN_RESOURCE, "no such resource")
        .with_detail("uri", uri.to_owned())
        .with_detail("known", [PROJECT_URI, "sequence://{id}", "media://{id}"])
}

/// The model inside a `project.get` answer.
///
/// `project.get` returns the project as the project *file* holds it: the model
/// wrapped in a document that carries the schema version it was written at
/// (`sub_model::json`). Resources are made of the model, and the version goes
/// with the whole-project resource so a reader knows which schema it is
/// reading.
fn model(answer: &Value) -> &Value {
    &answer["project"]["project"]
}

/// The resource list a `project.get` answer describes.
fn list_of(answer: &Value) -> Vec<Resource> {
    let project = model(answer);
    let name = project["name"].as_str().unwrap_or("the project");
    let mut resources = vec![
        Resource::new(PROJECT_URI, "project")
            .with_title(name.to_owned())
            .with_description(
                "The whole open project as the project file stores it: media, bins and \
                 sequences, with the revision it was read at.",
            )
            .with_mime_type(MIME_TYPE),
    ];
    for sequence in entries(project, "sequences") {
        let Some(id) = sequence["id"].as_str() else {
            continue;
        };
        resources.push(
            Resource::new(sequence_uri(id), format!("sequence/{id}"))
                .with_title(title(sequence, "sequence"))
                .with_description("One sequence: its settings, tracks, clips and markers.")
                .with_mime_type(MIME_TYPE),
        );
    }
    for item in entries(project, "media") {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        resources.push(
            Resource::new(media_uri(id), format!("media/{id}"))
                .with_title(title(item, "media item"))
                .with_description("One media item: its path, hash, probe results and proxy state.")
                .with_mime_type(MIME_TYPE),
        );
    }
    resources
}

/// The array `field` of the project, or nothing when it is not one.
fn entries<'a>(project: &'a Value, field: &str) -> &'a [Value] {
    project[field].as_array().map_or(&[], Vec::as_slice)
}

/// An entity's display name, falling back to a description of its kind.
fn title(entity: &Value, kind: &str) -> String {
    entity["name"]
        .as_str()
        .map_or_else(|| format!("an unnamed {kind}"), ToOwned::to_owned)
}

/// The JSON one target's resource carries, or `None` when the project has no
/// such entity.
///
/// The revision travels with every resource: two reads that carry the same
/// revision read the same project, which is what lets a client tell a cached
/// resource is still current.
fn contents_of(answer: &Value, target: &Target) -> Option<Value> {
    let revision = &answer["revision"];
    let project = model(answer);
    match target {
        Target::Project => Some(json!({
            "revision": revision,
            "schema_version": answer["project"]["schema_version"],
            "project": project,
        })),
        Target::Sequence(id) => find(entries(project, "sequences"), id)
            .map(|sequence| json!({ "revision": revision, "sequence": sequence })),
        Target::Media(id) => find(entries(project, "media"), id)
            .map(|item| json!({ "revision": revision, "media": item })),
    }
}

/// The entity of `entities` whose `id` is `id`.
fn find<'a>(entities: &'a [Value], id: &str) -> Option<&'a Value> {
    entities
        .iter()
        .find(|entity| entity["id"].as_str() == Some(id))
}

/// What one change event made stale.
///
/// A change event names an entity kind and, usually, an identifier, but not
/// the sequence a track, clip or marker belongs to: the Command API's events
/// are derived from command parameters and carry no parentage
/// (`sub_edit::ChangeEvent`). Rather than guess, an edit inside a timeline
/// marks *every* sequence resource stale — [`Update::all_sequences`] — which
/// costs a subscriber one extra read of a timeline it is already watching and
/// never leaves it holding a stale one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Update {
    /// The revision the project reached.
    pub revision: u64,
    /// The resources known to be stale, by URI.
    pub uris: BTreeSet<String>,
    /// Whether every `sequence://` resource is stale.
    pub all_sequences: bool,
    /// Whether every resource of every kind is stale, which is what a
    /// subscriber that fell behind has to assume.
    pub all: bool,
    /// Whether the set of resources itself changed, so `resources/list` is
    /// worth reading again.
    pub list_changed: bool,
}

impl Update {
    /// What the `event` of an `events.changed` notification made stale.
    ///
    /// The event's JSON shape is the Command API's stable one; an entity kind
    /// this build does not know — a plugin's, say — invalidates the project
    /// resource and nothing more.
    #[must_use]
    pub fn from_event(event: &Value) -> Self {
        let revision = event["revision"].as_u64().unwrap_or_default();
        let entity = event["entity"].as_str().unwrap_or_default();
        let id = event["id"].as_str();
        let structural = matches!(event["change"].as_str(), Some("added" | "removed"));

        // Every change touches the project resource: it holds everything.
        let mut update = Self {
            revision,
            uris: BTreeSet::from([PROJECT_URI.to_owned()]),
            ..Self::default()
        };
        match entity {
            "sequence" => {
                if let Some(id) = id {
                    update.uris.insert(sequence_uri(id));
                }
                update.all_sequences = id.is_none();
                update.list_changed = structural;
            }
            "media" => {
                if let Some(id) = id {
                    update.uris.insert(media_uri(id));
                }
                update.list_changed = structural;
            }
            "track" | "clip" | "marker" | "edit" => update.all_sequences = true,
            _ => {}
        }
        update
    }

    /// The update a subscriber that fell behind has to assume: everything it
    /// holds is stale, because it cannot know what it missed.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            all: true,
            list_changed: true,
            ..Self::default()
        }
    }

    /// Whether this update made the resource at `uri` stale.
    #[must_use]
    pub fn touches(&self, uri: &str) -> bool {
        self.all
            || self.uris.contains(uri)
            || (self.all_sequences && matches!(parse_uri(uri), Some(Target::Sequence(_))))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MEDIA_SCHEME, PROJECT_URI, SEQUENCE_SCHEME, Target, Update, accepted_filter, contents_of,
        list_of, media_uri, parse_uri, sequence_uri, templates,
    };
    use rmcp::model::SubscriptionFilter;
    use serde_json::{Value, json};

    /// A `project.get` answer with one sequence and one media item.
    fn answer() -> Value {
        json!({
            "revision": 7,
            "project": {
                "schema_version": 1,
                "project": {
                    "id": "p1",
                    "name": "Doc cut",
                    "media": [{ "id": "m1", "name": "b-roll.mov" }],
                    "sequences": [{ "id": "s1", "name": "Timeline 1", "tracks": [] }],
                },
            },
        })
    }

    #[test]
    fn a_uri_names_the_project_a_sequence_or_a_media_item() {
        assert_eq!(parse_uri(PROJECT_URI), Some(Target::Project));
        assert_eq!(
            parse_uri("sequence://s1"),
            Some(Target::Sequence("s1".to_owned())),
        );
        assert_eq!(
            parse_uri("media://m1"),
            Some(Target::Media("m1".to_owned())),
        );
        assert_eq!(sequence_uri("s1"), "sequence://s1");
        assert_eq!(media_uri("m1"), "media://m1");
        assert_eq!(Target::Project.uri(), PROJECT_URI);
        assert_eq!(Target::Sequence("s1".to_owned()).uri(), "sequence://s1");
    }

    #[test]
    fn a_uri_this_bridge_does_not_serve_is_not_parsed() {
        for uri in [
            "project://other",
            "project://",
            "sequence://",
            "clip://c1",
            "file:///tmp/project.json",
            "sequence:s1",
            "",
        ] {
            assert_eq!(parse_uri(uri), None, "{uri}");
        }
    }

    #[test]
    fn the_list_is_the_project_and_one_resource_per_entity() {
        let resources = list_of(&answer());
        let uris: Vec<&str> = resources.iter().map(|entry| entry.uri.as_str()).collect();
        assert_eq!(uris, [PROJECT_URI, "sequence://s1", "media://m1"]);
        assert_eq!(resources[0].title.as_deref(), Some("Doc cut"));
        assert_eq!(resources[1].title.as_deref(), Some("Timeline 1"));
        assert_eq!(resources[2].title.as_deref(), Some("b-roll.mov"));
        for resource in &resources {
            assert_eq!(resource.mime_type.as_deref(), Some("application/json"));
            assert!(resource.description.is_some());
        }
    }

    #[test]
    fn a_resource_carries_the_entity_and_the_revision_it_was_read_at() {
        let answer = answer();
        let project = contents_of(&answer, &Target::Project).expect("the project");
        assert_eq!(project["revision"], 7);
        assert_eq!(project["schema_version"], 1);
        assert_eq!(project["project"]["name"], "Doc cut");

        let sequence = contents_of(&answer, &Target::Sequence("s1".to_owned()))
            .expect("the sequence is in the project");
        assert_eq!(sequence["revision"], 7);
        assert_eq!(sequence["sequence"]["name"], "Timeline 1");

        let media = contents_of(&answer, &Target::Media("m1".to_owned()))
            .expect("the media item is in the project");
        assert_eq!(media["media"]["name"], "b-roll.mov");

        assert!(contents_of(&answer, &Target::Sequence("absent".to_owned())).is_none());
        assert!(contents_of(&answer, &Target::Media("absent".to_owned())).is_none());
    }

    #[test]
    fn the_templates_describe_the_per_entity_uris() {
        let templates = templates();
        let patterns: Vec<&str> = templates
            .iter()
            .map(|template| template.uri_template.as_str())
            .collect();
        assert_eq!(patterns, ["sequence://{id}", "media://{id}"]);
        for template in &templates {
            assert!(
                template.uri_template.starts_with(SEQUENCE_SCHEME)
                    || template.uri_template.starts_with(MEDIA_SCHEME)
            );
            assert_eq!(template.mime_type.as_deref(), Some("application/json"));
        }
    }

    #[test]
    fn an_edit_inside_a_timeline_makes_every_sequence_stale() {
        let update = Update::from_event(&json!({
            "revision": 4,
            "entity": "clip",
            "id": "c1",
            "change": "modified",
            "command": "clip.trim_in",
            "origin": "apply",
        }));
        assert_eq!(update.revision, 4);
        assert!(update.all_sequences);
        assert!(!update.list_changed);
        assert!(update.touches(PROJECT_URI));
        assert!(update.touches("sequence://anything"));
        assert!(!update.touches("media://m1"));
    }

    #[test]
    fn a_named_entity_makes_exactly_its_own_resource_stale() {
        let update = Update::from_event(&json!({
            "revision": 2,
            "entity": "media",
            "id": "m1",
            "change": "modified",
            "command": "media.relink",
            "origin": "apply",
        }));
        assert!(update.touches("media://m1"));
        assert!(update.touches(PROJECT_URI));
        assert!(!update.touches("media://m2"));
        assert!(!update.touches("sequence://s1"));
        assert!(!update.list_changed);
    }

    #[test]
    fn adding_or_removing_an_entity_changes_the_list_itself() {
        for (entity, change) in [
            ("media", "added"),
            ("media", "removed"),
            ("sequence", "added"),
            ("sequence", "removed"),
        ] {
            let update = Update::from_event(&json!({
                "revision": 1,
                "entity": entity,
                "change": change,
                "command": format!("{entity}.{change}"),
                "origin": "apply",
            }));
            assert!(update.list_changed, "{entity} {change}");
        }
        let renamed = Update::from_event(&json!({
            "revision": 1,
            "entity": "sequence",
            "id": "s1",
            "change": "modified",
            "command": "sequence.rename",
            "origin": "apply",
        }));
        assert!(!renamed.list_changed);
        assert!(renamed.touches("sequence://s1"));
    }

    #[test]
    fn a_sequence_the_event_does_not_name_makes_every_sequence_stale() {
        // `sequence.create` mints the identifier, so the event carries none.
        let update = Update::from_event(&json!({
            "revision": 3,
            "entity": "sequence",
            "change": "added",
            "command": "sequence.create",
            "origin": "apply",
        }));
        assert!(update.all_sequences);
        assert!(update.list_changed);
        assert!(update.touches("sequence://s1"));
    }

    #[test]
    fn an_entity_kind_this_build_does_not_know_invalidates_the_project_alone() {
        let update = Update::from_event(&json!({
            "revision": 9,
            "entity": "widget",
            "id": "w1",
            "change": "modified",
            "command": "widget.tweak",
            "origin": "apply",
        }));
        assert!(update.touches(PROJECT_URI));
        assert!(!update.all_sequences);
        assert!(!update.list_changed);
        assert!(!update.touches("sequence://s1"));
    }

    #[test]
    fn an_event_that_is_not_an_event_still_invalidates_the_project() {
        let update = Update::from_event(&json!({}));
        assert_eq!(update.revision, 0);
        assert!(update.touches(PROJECT_URI));
    }

    #[test]
    fn a_subscription_filter_keeps_only_the_uris_this_bridge_serves() {
        let requested = SubscriptionFilter::builder()
            .resources_list_changed()
            .resource_subscriptions([PROJECT_URI, "sequence://s1", "clip://c1", "file:///x"])
            .build();
        let accepted = accepted_filter(&requested);
        assert_eq!(accepted.resources_list_changed, Some(true));
        assert_eq!(
            accepted.resource_subscriptions.as_deref(),
            Some([PROJECT_URI.to_owned(), "sequence://s1".to_owned()].as_slice()),
        );

        // A client that asks for nothing is accepted as asking for nothing.
        let nothing = accepted_filter(&SubscriptionFilter::new());
        assert_eq!(nothing.resource_subscriptions, None);
        assert_eq!(nothing.resources_list_changed, None);
    }
}
