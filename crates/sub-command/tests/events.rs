//! Event subscription across two connections to one Command API server.
//!
//! This is the property the MCP bridge and the pop-out windows rely on: a
//! client that subscribes learns about a change another client made, without
//! polling anything (docs/PLAN.md §4).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use sub_command::events::{EVENTS_CHANGED, EVENTS_SUBSCRIBE, EVENTS_UNSUBSCRIBE};
use sub_command::transport::{Client, Server};
use sub_command::{Dispatcher, Endpoint};
use sub_edit::Engine;
use sub_model::Project;

/// A directory nothing else in this test binary uses.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-events-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An engine and a server bound to a private endpoint.
fn serving(name: &str) -> (Engine, Server) {
    let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
    let endpoint = Endpoint::in_directory(temp_dir(name), "test").unwrap();
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let server = Server::bind(endpoint, dispatcher).unwrap();
    (engine, server)
}

#[test]
fn a_subscriber_sees_a_change_made_by_another_client() {
    let (engine, server) = serving("two-clients");
    let mut watcher = Client::connect(server.endpoint()).unwrap();
    let mut editor = Client::connect(server.endpoint()).unwrap();

    let subscription = watcher.invoke(EVENTS_SUBSCRIBE, None).unwrap()["subscription"]
        .as_str()
        .unwrap()
        .to_owned();

    let applied = editor
        .invoke("bin.create", Some(json!({ "name": "Footage" })))
        .unwrap();
    assert_eq!(applied["revision"], 1);

    let notification = watcher.recv_notification().unwrap().unwrap();
    assert_eq!(notification.method, EVENTS_CHANGED);
    let params = notification.params.unwrap();
    assert_eq!(params["subscription"], subscription.as_str());
    assert_eq!(params["event"]["command"], "bin.create");
    assert_eq!(params["event"]["revision"], 1);
    assert_eq!(params["event"]["entity"], "bin");

    // The editor never subscribed, so nothing was pushed at it: its own next
    // request is answered normally.
    assert_eq!(
        editor.invoke("project.revision", None).unwrap()["revision"],
        1
    );

    drop(watcher);
    drop(editor);
    server.shutdown().unwrap();
    engine.shutdown().unwrap();
}

#[test]
fn unsubscribing_stops_the_notifications() {
    let (engine, server) = serving("unsubscribe");
    let mut watcher = Client::connect(server.endpoint()).unwrap();
    let mut editor = Client::connect(server.endpoint()).unwrap();

    let subscription = watcher.invoke(EVENTS_SUBSCRIBE, None).unwrap()["subscription"].clone();
    editor
        .invoke("bin.create", Some(json!({ "name": "Footage" })))
        .unwrap();
    let first = watcher.recv_notification().unwrap().unwrap();
    assert_eq!(first.method, EVENTS_CHANGED);

    let stopped = watcher
        .invoke(
            EVENTS_UNSUBSCRIBE,
            Some(json!({ "subscription": subscription })),
        )
        .unwrap();
    assert_eq!(stopped["subscription"], subscription);

    editor
        .invoke("bin.create", Some(json!({ "name": "Music" })))
        .unwrap();
    // The second change is not delivered: the watcher's next reply is its own
    // response, with no notification queued in front of it.
    assert_eq!(
        watcher.invoke("project.revision", None).unwrap()["revision"],
        2
    );
    assert!(watcher.next_notification().is_none());

    // A subscription can only be stopped once.
    let error = watcher
        .invoke(
            EVENTS_UNSUBSCRIBE,
            Some(json!({ "subscription": subscription })),
        )
        .unwrap_err();
    assert_eq!(error.code.as_str(), "command.unknown_subscription");

    drop(watcher);
    drop(editor);
    server.shutdown().unwrap();
    engine.shutdown().unwrap();
}

#[test]
fn a_disconnected_subscriber_stops_costing_the_engine_anything() {
    let (engine, server) = serving("disconnect");
    let mut watcher = Client::connect(server.endpoint()).unwrap();
    watcher.invoke(EVENTS_SUBSCRIBE, None).unwrap();
    assert_eq!(engine.handle().subscriber_count(), 1);

    drop(watcher);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while engine.handle().subscriber_count() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the subscription outlived its connection",
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    server.shutdown().unwrap();
    engine.shutdown().unwrap();
}
