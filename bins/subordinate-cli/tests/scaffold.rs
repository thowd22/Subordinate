//! `subordinate-cli plugin new`: the one command a plugin starts from.
//!
//! The scaffold is only worth anything if what it writes builds and installs
//! with no edits (docs/PLAN.md §6.4), so these tests go through the built
//! binary rather than the module: the report is the JSON an agent parses, and
//! the crate on disk is what `cargo` and `plugin install` are handed next.
//!
//! The build itself is opt-in. Compiling a component needs the
//! `wasm32-wasip2` target, a `subordinate-sdk` to depend on and a warm cargo
//! registry, none of which every machine running `cargo test` has. Set
//! `SUBORDINATE_SCAFFOLD_BUILD=1` and `SUBORDINATE_SDK_PATH` to the SDK in
//! this checkout to run it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Runs the CLI and returns success, stdout and stderr.
fn run(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args(args)
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .output()
        .expect("subordinate-cli runs");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("utf-8 stdout"),
        String::from_utf8(output.stderr).expect("utf-8 stderr"),
    )
}

/// Runs the CLI and parses the JSON it printed.
fn json(args: &[&str]) -> Value {
    let (ok, stdout, stderr) = run(args);
    assert!(ok, "{args:?} failed: {stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{args:?} printed no JSON: {err}"))
}

/// A directory of this test's own, removed and recreated on each run.
fn workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// The path as the CLI takes it.
fn arg(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn a_scaffold_is_a_crate_a_manifest_a_guide_and_a_fixture_project() {
    let directory = workspace("scaffold-command");
    let report = json(&[
        "plugin",
        "new",
        "--world",
        "command",
        "cut-silence",
        "--output",
        &arg(&directory),
    ]);

    assert_eq!(report["id"], "com.example.cut-silence");
    assert_eq!(report["world"], "command");
    assert_eq!(report["crate"], "cut-silence");

    let root = directory.join("cut-silence");
    assert_eq!(report["path"], arg(&root));
    for expected in [
        "Cargo.toml",
        "plugin.toml",
        "CLAUDE.md",
        "rust-toolchain.toml",
        "src/lib.rs",
        "fixture/fixture.sub",
    ] {
        assert!(root.join(expected).is_file(), "{expected} was not written");
    }

    // The three commands the scaffold is meant to be followed by, naming the
    // artefact cargo will actually produce.
    let steps = report["next_steps"].as_array().expect("next steps");
    assert!(
        steps[1]
            .as_str()
            .expect("an install step")
            .contains("cut_silence.wasm"),
        "{steps:?}",
    );

    // The fixture is a project this same CLI reads back.
    let fixture = root.join("fixture").join("fixture.sub");
    let opened = json(&["open", &arg(&fixture)]);
    assert_eq!(opened["project"]["name"], "Cut Silence");
    assert_eq!(opened["counts"]["sequences"], 1);
}

#[test]
fn every_templated_world_scaffolds_and_the_manifest_declares_it() {
    let directory = workspace("scaffold-worlds");
    for world in ["command", "effect", "analyzer", "mcp-tools"] {
        let report = json(&[
            "plugin",
            "new",
            "--world",
            world,
            "demo",
            "--output",
            &arg(&directory),
            "--force",
        ]);
        assert_eq!(report["world"], world);
        let manifest = std::fs::read_to_string(directory.join("demo").join("plugin.toml"))
            .expect("a manifest");
        assert!(
            manifest.contains(&format!("worlds = [\"{world}\"]")),
            "{world}: {manifest}",
        );
        let guide =
            std::fs::read_to_string(directory.join("demo").join("CLAUDE.md")).expect("a guide");
        assert!(guide.contains("## The test contract"), "{world}");
    }
}

#[test]
fn a_world_with_no_template_is_refused_with_the_ones_that_have_one() {
    let directory = workspace("scaffold-world");
    let (ok, _, stderr) = run(&[
        "plugin",
        "new",
        "--world",
        "panel",
        "demo",
        "--output",
        &arg(&directory),
    ]);
    assert!(!ok, "a panel scaffold should have been refused");
    assert!(stderr.contains("command"), "{stderr}");
    assert!(stderr.contains("mcp-tools"), "{stderr}");
    assert!(
        !directory.join("demo").exists(),
        "nothing should have been written",
    );
}

#[test]
fn plugin_new_without_a_world_or_a_name_says_which_is_missing() {
    let directory = workspace("scaffold-incomplete");
    let (ok, _, stderr) = run(&["plugin", "new", "demo", "--output", &arg(&directory)]);
    assert!(!ok);
    assert!(stderr.contains("--world"), "{stderr}");

    let (ok, _, stderr) = run(&["plugin", "new", "--world", "command"]);
    assert!(!ok);
    assert!(stderr.contains("name"), "{stderr}");
}

/// A command run with none of the toolchain overrides `cargo test` sets, so
/// rustup picks the toolchain from the `rust-toolchain.toml` in the working
/// directory rather than from the toolchain that built this test.
fn toolchain_free(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .env_remove("RUSTDOC");
    command
}

/// The channel the scaffold pinned in its `rust-toolchain.toml`.
fn pinned_channel(root: &Path) -> String {
    let toolchain =
        std::fs::read_to_string(root.join("rust-toolchain.toml")).expect("a pinned toolchain");
    let Some(channel) = toolchain
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = "))
    else {
        panic!("no channel in {toolchain}")
    };
    channel.trim().trim_matches('"').to_owned()
}

/// What rustup resolves in the scaffold directory, when rustup is what runs
/// cargo here at all.
fn active_toolchain(root: &Path) -> Option<String> {
    let output = toolchain_free("rustup")
        .current_dir(root)
        .args(["show", "active-toolchain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.trim().to_owned())
}

/// The SDK checkout a build test depends on by path, when one was named.
fn sdk_path() -> Option<PathBuf> {
    std::env::var_os("SUBORDINATE_SCAFFOLD_BUILD")?;
    std::env::var_os("SUBORDINATE_SDK_PATH").map(PathBuf::from)
}

#[test]
fn a_scaffold_builds_and_installs_with_no_edits() {
    let Some(sdk) = sdk_path() else {
        eprintln!(
            "skipped: set SUBORDINATE_SCAFFOLD_BUILD=1 and SUBORDINATE_SDK_PATH to run the build",
        );
        return;
    };
    let directory = workspace("scaffold-build");
    for (world, name) in [
        ("command", "built-command"),
        ("effect", "built-effect"),
        ("analyzer", "built-analyzer"),
        ("mcp-tools", "built-tools"),
    ] {
        json(&[
            "plugin",
            "new",
            "--world",
            world,
            name,
            "--output",
            &arg(&directory),
            "--sdk-path",
            &arg(&sdk),
        ]);
        let root = directory.join(name);

        // The scaffold pins its own toolchain, and that pin is the whole point
        // of this build: rustup must select it from the crate directory even
        // when the machine's default toolchain is older than the crate's
        // rust-version. So the build goes through the plain `cargo` shim with
        // every toolchain override this test inherited stripped off, exactly as
        // the guide prints it.
        let pinned = pinned_channel(&root);
        if let Some(active) = active_toolchain(&root) {
            assert!(
                active.starts_with(&pinned),
                "{world}: rustup chose {active}, not the pinned {pinned}",
            );
        }
        let built = toolchain_free("cargo")
            .current_dir(&root)
            .args(["build", "--release", "--target", "wasm32-wasip2"])
            // Whatever host flags built this test are for the host triple:
            // a component is linked by wasm-component-ld, which takes none of
            // them.
            .env_remove("RUSTFLAGS")
            .output()
            .expect("cargo runs");
        assert!(
            built.status.success(),
            "{world} did not build: {}",
            String::from_utf8_lossy(&built.stderr),
        );

        // And the component it produced installs, under the id the manifest
        // gave it, into a plugin directory of this test's own.
        let wasm = root
            .join("target")
            .join("wasm32-wasip2")
            .join("release")
            .join(format!("{}.wasm", name.replace('-', "_")));
        assert!(wasm.is_file(), "{world}: {} is missing", wasm.display());
        let plugin_dir = directory.join("installed");
        let installed = json(&[
            "plugin",
            "install",
            &arg(&wasm),
            "--dev",
            "--dir",
            &arg(&plugin_dir),
        ]);
        assert_eq!(installed["id"], format!("com.example.{name}"));
        let listed = json(&["plugin", "list", "--dir", &arg(&plugin_dir)]);
        assert!(
            listed["plugins"]
                .as_array()
                .expect("plugins")
                .iter()
                .any(|plugin| plugin["id"] == format!("com.example.{name}")),
            "{world}: {listed}",
        );
    }
}
