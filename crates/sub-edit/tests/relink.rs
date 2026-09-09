//! Relinking offline media against a real folder of real files.
//!
//! These tests judge behaviour, not code shape: files written to disk, a
//! project whose sources have moved, and what a search then proposes. The
//! rules under test are the ones the user notices — a renamed file is still
//! found by its bytes, a re-encoded file is only ever a name guess, two copies
//! of a take relink two items, and undoing a bulk relink puts every item back
//! in one press.

use std::path::{Path, PathBuf};

use sub_edit::History;
use sub_edit::relink::{
    MatchKind, RELINK_GROUP_LABEL, RelinkPlan, SearchOptions, match_offline, scan_folder,
};
use sub_model::{ContentHash, MediaId, MediaItem, MediaPath, Project};

/// A directory of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-edit-relink-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the temporary directory is creatable");
    dir
}

/// Writes `bytes` to `dir/relative`, creating the folders on the way.
fn write(dir: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().expect("a file has a parent"))
        .expect("the folder is creatable");
    std::fs::write(&path, bytes).expect("the file is writable");
    path
}

/// Adds an offline item for `relative`, hashed as `hash`, to `project`.
fn offline_item(project: &mut Project, relative: &str, hash: Option<ContentHash>) -> MediaId {
    let mut item = MediaItem::new(MediaPath::new(relative).expect("a valid relative path"));
    item.hash = hash;
    item.offline = true;
    let id = item.id;
    project.root_bin.media.push(id);
    project.media.push(item);
    id
}

#[test]
fn a_folder_search_is_recursive_and_bounded_by_depth_and_count() {
    let dir = temp_dir("scan");
    write(&dir, "top.mov", b"top");
    write(&dir, "day1/a.mov", b"a");
    write(&dir, "day1/card2/b.mov", b"b");
    write(&dir, "day1/card2/deep/c.mov", b"c");

    let all = scan_folder(&dir, SearchOptions::default());
    let names: Vec<String> = all
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["top.mov", "a.mov", "b.mov", "c.mov"]);

    // Depth zero is the root folder alone.
    let shallow = scan_folder(&dir, SearchOptions::new().with_max_depth(0));
    assert_eq!(shallow.len(), 1);
    assert!(shallow[0].ends_with("top.mov"));

    // One level below the root reaches day1 but not card2.
    let one = scan_folder(&dir, SearchOptions::new().with_max_depth(1));
    assert_eq!(one.len(), 2);

    // The file budget stops the walk wherever it runs out.
    let capped = scan_folder(&dir, SearchOptions::new().with_max_files(2));
    assert_eq!(capped.len(), 2);

    // A folder that does not exist is empty, not a failure.
    assert!(scan_folder(&dir.join("nowhere"), SearchOptions::default()).is_empty());
}

#[test]
fn the_content_hash_finds_a_renamed_file_and_beats_a_same_named_impostor() {
    let dir = temp_dir("hash-first");
    // The real take, renamed and moved into a subfolder.
    let real = write(&dir, "archive/renamed.mov", b"the real take, byte for byte");
    // A different file that happens to carry the old name.
    write(
        &dir,
        "archive/interview.mov",
        b"a different recording entirely",
    );

    let hash = ContentHash::of_file(&real).expect("the file is readable");
    let mut project = Project::new("Doc cut");
    let media = offline_item(&mut project, "footage/interview.mov", Some(hash));

    let files = scan_folder(&dir, SearchOptions::default());
    let matches = match_offline(&project, &files);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].media, media);
    assert_eq!(matches[0].kind, MatchKind::Hash);
    assert_eq!(matches[0].file, real);
    assert_eq!(matches[0].hash, Some(hash));
    assert_eq!(MatchKind::Hash.label(), "hash");
}

#[test]
fn an_item_with_no_hash_match_falls_back_to_its_file_name() {
    let dir = temp_dir("name-second");
    let moved = write(
        &dir,
        "rushes/INTERVIEW.MOV",
        b"re-encoded, so the bytes differ",
    );
    write(&dir, "rushes/unrelated.mov", b"nothing to do with it");

    let mut project = Project::new("Doc cut");
    // Hashed at import, but no file in the folder hashes to it any more.
    let stale = ContentHash::from_bytes([9_u8; 32]);
    let media = offline_item(&mut project, "footage/interview.mov", Some(stale));

    let files = scan_folder(&dir, SearchOptions::default());
    let matches = match_offline(&project, &files);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].media, media);
    assert_eq!(matches[0].kind, MatchKind::Name, "the name is the fallback");
    assert_eq!(matches[0].file, moved, "case is ignored in the name search");
    assert_eq!(
        matches[0].hash,
        Some(ContentHash::of_file(&moved).expect("readable")),
        "a name match still records the file's real hash"
    );
    assert_eq!(MatchKind::Name.label(), "name");
}

#[test]
fn two_items_sharing_a_hash_claim_two_files() {
    let dir = temp_dir("two-copies");
    let first = write(&dir, "copies/one.mov", b"identical bytes in both copies");
    let second = write(&dir, "copies/two.mov", b"identical bytes in both copies");
    let hash = ContentHash::of_file(&first).expect("readable");
    assert_eq!(hash, ContentHash::of_file(&second).expect("readable"));

    let mut project = Project::new("Doc cut");
    let a = offline_item(&mut project, "footage/a.mov", Some(hash));
    let b = offline_item(&mut project, "footage/b.mov", Some(hash));

    let files = scan_folder(&dir, SearchOptions::default());
    let matches = match_offline(&project, &files);
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].media, a);
    assert_eq!(matches[1].media, b);
    assert_ne!(
        matches[0].file, matches[1].file,
        "a file is claimed by at most one item"
    );
    assert!([first, second].contains(&matches[0].file));
}

#[test]
fn an_online_project_proposes_nothing_and_an_unmatched_item_is_left_alone() {
    let dir = temp_dir("nothing");
    write(&dir, "footage/other.mov", b"not the one");

    let mut project = Project::new("Doc cut");
    // Offline, unhashed, and nothing in the folder carries its name.
    offline_item(&mut project, "footage/missing.mov", None);
    let files = scan_folder(&dir, SearchOptions::default());
    assert!(match_offline(&project, &files).is_empty());

    // Nothing offline at all: no reading, no matches.
    let mut online = Project::new("Doc cut");
    let item = MediaItem::new(MediaPath::new("footage/other.mov").expect("valid"));
    online.media.push(item);
    assert!(match_offline(&online, &files).is_empty());
    assert!(match_offline(&project, &[]).is_empty());
}

#[test]
fn a_bulk_relink_is_one_undo_step_and_clears_the_offline_flag() {
    let dir = temp_dir("bulk");
    let one = write(&dir, "recovered/one.mov", b"take one, moved but intact");
    let two = write(&dir, "recovered/two.mov", b"take two, moved but intact");
    let hash_one = ContentHash::of_file(&one).expect("readable");
    let hash_two = ContentHash::of_file(&two).expect("readable");

    let mut project = Project::new("Doc cut");
    let a = offline_item(&mut project, "footage/one.mov", Some(hash_one));
    let b = offline_item(&mut project, "footage/two.mov", Some(hash_two));

    let files = scan_folder(&dir, SearchOptions::default());
    let matches = match_offline(&project, &files);
    let plan = RelinkPlan::build(&dir, &matches);
    assert_eq!(plan.len(), 2);
    assert!(!plan.is_empty());
    assert!(plan.rejected.is_empty());

    let mut history = History::new();
    assert_eq!(
        plan.apply(&mut history, &mut project).expect("it applies"),
        2
    );

    for (id, expected) in [(a, "recovered/one.mov"), (b, "recovered/two.mov")] {
        let item = project.media_item(id).expect("the item kept its identity");
        assert_eq!(item.path.as_str(), expected);
        assert!(!item.offline, "a relinked item is online again");
        assert!(item.hash.is_some(), "the relink records the file's hash");
    }
    assert_eq!(project.offline_media().count(), 0);

    // One entry in the history, whatever the item count.
    assert_eq!(history.undo_label(), Some(RELINK_GROUP_LABEL));
    history.undo(&mut project).expect("one press undoes it all");
    assert_eq!(project.offline_media().count(), 2);
    assert_eq!(
        project.media_item(a).expect("still there").path.as_str(),
        "footage/one.mov"
    );
    assert!(history.undo_label().is_none(), "that was the only step");

    history.redo(&mut project).expect("and one press redoes it");
    assert_eq!(project.offline_media().count(), 0);
}

#[test]
fn a_file_outside_the_project_folder_is_refused_rather_than_stored() {
    let dir = temp_dir("outside");
    let elsewhere = temp_dir("outside-source");
    let moved = write(&elsewhere, "one.mov", b"footage that was left outside");
    let hash = ContentHash::of_file(&moved).expect("readable");

    let mut project = Project::new("Doc cut");
    let media = offline_item(&mut project, "footage/one.mov", Some(hash));

    let files = scan_folder(&elsewhere, SearchOptions::default());
    let matches = match_offline(&project, &files);
    assert_eq!(matches.len(), 1);

    let plan = RelinkPlan::build(&dir, &matches);
    assert!(
        plan.is_empty(),
        "nothing outside the project folder is stored"
    );
    assert_eq!(plan.rejected.len(), 1);
    assert_eq!(plan.rejected[0].0, media);
    assert_eq!(plan.rejected[0].1.code, sub_model::codes::INVALID_PATH);

    let mut history = History::new();
    assert_eq!(plan.apply(&mut history, &mut project).expect("no-op"), 0);
    assert!(history.undo_label().is_none(), "an empty plan is no step");
    assert!(project.media_item(media).expect("still there").offline);
}
