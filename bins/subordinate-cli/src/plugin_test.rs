//! `plugin test <id>`: the headless proof that a plugin works.
//!
//! This is the last step of the developer loop (docs/PLAN.md §6.4) and the one
//! an agent leans on hardest: scaffold, build, install, *test*. It loads an
//! installed plugin's component, opens a fixture project, spawns the ordinary
//! engine and Command API dispatcher over it, and hands the pair to
//! [`sub_plugin::Harness`], which exercises every world the plugin's
//! `plugin.toml` declares.
//!
//! Nothing here is a special test path through the editor: the plugin talks to
//! the same [`Dispatcher`] the socket serves, so an edit it makes is one
//! undoable command like any other, and the harness proves exactly that by
//! undoing it again.
//!
//! The fixture is whichever of these exists first: `--fixture`, the
//! `fixture/fixture.sub` in the source tree a `--dev` install was linked to,
//! the one inside the installed plugin directory, or — for a plugin installed
//! without either — a fresh starter project written into a temporary
//! directory, so `plugin test` answers on a machine where nothing else has
//! been set up.
//!
//! A fixture may also declare the timeline it expects a command plugin to
//! leave behind, in a sidecar beside it: `fixture/fixture.expect.json` for
//! `fixture/fixture.sub` (TASK-129). Where one is there, the report carries a
//! `timeline_matches` check that fails naming every clip in the wrong place;
//! where there is none, nothing changes — the check is skipped.
//!
//! The answer is the harness's report as JSON, with the fixture and the
//! component it ran alongside. It carries `ok`, which is what the CLI turns
//! into an exit code: a failed check exits non-zero with the one-line summary
//! on stderr, so a script or an agent need not parse anything to know the run
//! failed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use sub_command::Dispatcher;
use sub_core::{SubError, SubResult, codes};
use sub_edit::Engine;
use sub_plugin::TimelineExpectation;
use sub_plugin::dev::{DevInstall, WASM_FILE_NAME};
use sub_plugin::manifest::PluginId;
use sub_plugin::registry::{InstalledPlugin, PluginRegistry};
use sub_plugin::{Harness, TestReport};

/// The fixture project a scaffolded plugin carries, relative to its crate
/// root.
const FIXTURE_PATH: &str = "fixture/fixture.sub";

/// How `plugin test` was asked to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// The project to run the plugin against, overriding the plugin's own
    /// fixture.
    pub fixture: Option<PathBuf>,
    /// The JSON arguments handed to a command plugin's `run` and to every MCP
    /// tool call. `{}` by default.
    pub args: Option<String>,
}

/// Loads one installed plugin and runs its declared tests against a fixture
/// project.
///
/// # Errors
///
/// `plugin.not_installed` when no plugin is installed under `id`,
/// `plugin.load_failed` when the installed directory holds no component,
/// `core.invalid_argument` when `--fixture` names a file that is not there,
/// `--args` is not JSON or the fixture's `*.expect.json` sidecar is not an
/// expectation document, `plugin.engine_failed` when this build has no wasm
/// compiler, and whatever loading the fixture project or spawning the engine
/// returns. A plugin that merely fails its checks is *not* an error: it comes
/// back as a report whose `ok` is false.
pub fn run(id: &str, dirs: &crate::plugin::Options, options: &Options) -> SubResult<Value> {
    let id = crate::plugin::parse_id(id)?;
    let registry = PluginRegistry::new(dirs.dirs()?);
    let installed = registry.scan()?.get(&id).cloned().ok_or_else(|| {
        SubError::new(
            sub_plugin::codes::NOT_INSTALLED,
            "no plugin is installed under that id",
        )
        .with_detail("id", id.as_str())
    })?;

    // A dev install's files point at the source tree it was built from; a copy
    // install is brought back in step before it is read.
    let dev = DevInstall::of(&installed);
    if let Some(install) = &dev {
        install.refresh()?;
    }
    let wasm = installed.directory.join(WASM_FILE_NAME);
    if !wasm.is_file() {
        return Err(SubError::new(
            sub_plugin::codes::LOAD_FAILED,
            "the installed plugin holds no plugin.wasm; build and install it again",
        )
        .with_detail("id", id.as_str())
        .with_detail("path", wasm.display().to_string()));
    }

    let args = args(options)?;
    let scratch = Scratch::new(&id);
    let fixture = fixture(&installed, dev.as_ref(), options, &scratch)?;
    let expectation = TimelineExpectation::beside(&fixture.path)?;
    let declared = expectation
        .as_ref()
        .map(|(path, _)| path.display().to_string());
    let report = against(&id, &installed, &wasm, &fixture.path, &args, expectation)?;

    let mut json = report.to_json()?;
    json["fixture"] = Value::String(fixture.path.display().to_string());
    json["fixture_source"] = Value::String(fixture.origin.to_owned());
    json["expectation"] = declared.map_or(Value::Null, Value::String);
    json["component"] = Value::String(wasm.display().to_string());
    json["directory"] = Value::String(installed.directory.display().to_string());
    json["location"] = Value::String(installed.location.as_str().to_owned());
    json["dev"] = Value::Bool(installed.dev);
    Ok(json)
}

/// Opens `fixture`, spawns the engine and the dispatcher over it, and runs
/// every check the plugin's worlds call for.
fn against(
    id: &PluginId,
    installed: &InstalledPlugin,
    wasm: &Path,
    fixture: &Path,
    args: &str,
    expectation: Option<(PathBuf, TimelineExpectation)>,
) -> SubResult<TestReport> {
    let (project, _) = crate::project::load(fixture)?;
    let engine = Engine::spawn(project)?;
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let tested = Harness::new(engine.handle().clone(), dispatcher)
        .map(|harness| harness.with_args(args))
        .map(|harness| match expectation {
            Some((_, expectation)) => harness.with_expectation(expectation),
            None => harness,
        })
        .and_then(|harness| harness.run(id, &installed.manifest, &installed.directory, wasm));
    let stopped = engine.shutdown();
    let report = tested?;
    stopped?;
    Ok(report)
}

/// The arguments the plugin is handed, checked as JSON before anything is
/// loaded.
fn args(options: &Options) -> SubResult<String> {
    let Some(args) = options.args.clone() else {
        return Ok(sub_plugin::harness::DEFAULT_ARGS.to_owned());
    };
    serde_json::from_str::<Value>(&args)
        .map_err(|err| {
            SubError::wrap(codes::INVALID_ARGUMENT, "--args must be JSON", &err)
                .with_detail("args", args.clone())
        })
        .map(|_| args)
}

/// The fixture project a run was given, and where it came from.
struct Fixture {
    path: PathBuf,
    origin: &'static str,
}

/// Picks the fixture project: the one named, the plugin's own, or a fresh one.
fn fixture(
    installed: &InstalledPlugin,
    dev: Option<&DevInstall>,
    options: &Options,
    scratch: &Scratch,
) -> SubResult<Fixture> {
    if let Some(named) = &options.fixture {
        if !named.is_file() {
            return Err(
                SubError::new(codes::INVALID_ARGUMENT, "--fixture names no project file")
                    .with_detail("path", named.display().to_string()),
            );
        }
        return Ok(Fixture {
            path: named.clone(),
            origin: "option",
        });
    }

    // A dev install is linked to the crate it was built from, and that crate
    // is where `plugin new` wrote the fixture.
    if let Some(source) = dev.map(|install| install.source.root.join(FIXTURE_PATH))
        && source.is_file()
    {
        return Ok(Fixture {
            path: source,
            origin: "source",
        });
    }

    let installed_fixture = installed.directory.join(FIXTURE_PATH);
    if installed_fixture.is_file() {
        return Ok(Fixture {
            path: installed_fixture,
            origin: "plugin",
        });
    }

    // Nothing shipped one, so the harness writes the same starter project
    // `subordinate-cli new` would: one sequence, one video track, one audio
    // track.
    let path = scratch.path().join("fixture.sub");
    crate::project::new(&path, Some(installed.name()), true)?;
    Ok(Fixture {
        path,
        origin: "generated",
    })
}

/// A temporary directory that lasts as long as one test run.
///
/// A generated fixture is written here, so a `plugin test` never leaves a
/// project file behind and never writes into the plugin directory it is
/// testing.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A scratch directory named after the plugin under test.
    ///
    /// The name carries a counter as well as the process id, because two runs
    /// of the same plugin in one process — the tests below, and an agent
    /// testing after a rebuild — must not share a directory one of them
    /// deletes on the way out.
    fn new(id: &PluginId) -> Self {
        static RUNS: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "subordinate-plugin-test-{}-{}-{}",
            id.as_str(),
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed),
        ));
        Self { path }
    }

    /// The directory, created on first use.
    fn path(&self) -> &Path {
        let _ = std::fs::create_dir_all(&self.path);
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Options, run};

    /// A user plugin directory holding one installed plugin whose component is
    /// the `bytes` given.
    fn installed(name: &str, worlds: &str, bytes: &[u8]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("subordinate-cli-plugin-test-{name}"));
        std::fs::remove_dir_all(&root).ok();
        let dir = root.join("com.example.one");
        std::fs::create_dir_all(&dir).expect("a plugin directory");
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "[plugin]\nid = \"com.example.one\"\nname = \"One\"\nversion = \"0.1.0\"\n\
                 api = \"0.1\"\nworlds = [{worlds}]\n",
            ),
        )
        .expect("a manifest");
        std::fs::write(dir.join("plugin.wasm"), bytes).expect("a component");
        root
    }

    /// Options pointed at a scratch plugin directory.
    fn dirs(root: &Path) -> crate::plugin::Options {
        crate::plugin::Options {
            user_dir: Some(root.to_path_buf()),
            project: None,
        }
    }

    #[test]
    fn a_plugin_that_is_not_installed_is_reported_before_anything_is_loaded() {
        let root = installed("missing", "\"command\"", b"\0asm\x01\0\0\0");
        let error = run("com.example.other", &dirs(&root), &Options::default())
            .expect_err("nothing is installed under that id");
        assert_eq!(error.code.as_str(), "plugin.not_installed");
    }

    #[test]
    fn a_malformed_id_is_refused() {
        let root = installed("bad-id", "\"command\"", b"\0asm\x01\0\0\0");
        let error =
            run("nodots", &dirs(&root), &Options::default()).expect_err("not a reverse-DNS id");
        assert_eq!(error.code.as_str(), "plugin.invalid_plugin_id");
    }

    #[test]
    fn args_that_are_not_json_are_refused() {
        let root = installed("args", "\"command\"", b"\0asm\x01\0\0\0");
        let options = Options {
            args: Some("threshold=-40".to_owned()),
            ..Options::default()
        };
        let error = run("com.example.one", &dirs(&root), &options).expect_err("not JSON");
        assert_eq!(error.code.as_str(), "core.invalid_argument");
    }

    /// A fixture's expectation sidecar is read before the plugin is loaded, so
    /// a hand-written one with a typo is refused by name rather than turning
    /// into a mystery check (TASK-129).
    #[test]
    fn an_expectation_sidecar_that_is_not_one_is_refused_naming_itself() {
        let root = installed("expectation", "\"command\"", b"\0asm\x01\0\0\0");
        let fixture = root.join("fixture.sub");
        crate::project::new(&fixture, Some("Fixture"), true).expect("a fixture project");
        std::fs::write(
            sub_plugin::expect::sidecar_for(&fixture),
            "{ \"sequences\": \"not a list\" }",
        )
        .expect("a sidecar");
        let options = Options {
            fixture: Some(fixture),
            ..Options::default()
        };
        let error = run("com.example.one", &dirs(&root), &options).expect_err("not an expectation");
        assert_eq!(error.code.as_str(), "core.invalid_argument");
        assert!(
            error.details.contains_key("path"),
            "the sidecar names itself: {error:?}",
        );
    }

    /// And a fixture with no sidecar runs exactly as it did before: the report
    /// says it declared none.
    #[test]
    fn a_fixture_with_no_expectation_reports_none() {
        let root = installed("no-expectation", "\"command\"", b"\0asm\x01\0\0\0");
        let report =
            run("com.example.one", &dirs(&root), &Options::default()).expect("a whole report");
        assert_eq!(report["expectation"], serde_json::Value::Null);
    }

    #[test]
    fn a_fixture_that_is_not_there_is_refused() {
        let root = installed("fixture", "\"command\"", b"\0asm\x01\0\0\0");
        let options = Options {
            fixture: Some(root.join("nothing.sub")),
            ..Options::default()
        };
        let error = run("com.example.one", &dirs(&root), &options).expect_err("no such fixture");
        assert_eq!(error.code.as_str(), "core.invalid_argument");
    }

    #[test]
    fn an_installed_plugin_with_no_component_says_so() {
        let root = installed("no-wasm", "\"command\"", b"");
        std::fs::remove_file(root.join("com.example.one").join("plugin.wasm")).expect("removed");
        let error = run("com.example.one", &dirs(&root), &Options::default())
            .expect_err("there is no component");
        assert_eq!(error.code.as_str(), "plugin.load_failed");
    }

    /// A component that is not one fails the run rather than ending it: the
    /// report is still printed, and it says `ok: false` with the load failure
    /// as its first check, which is what the CLI exits non-zero on.
    #[test]
    fn a_component_that_does_not_load_is_a_failed_check_with_a_readable_report() {
        // A core module header: enough to install, not a component.
        let root = installed("bad-component", "\"command\"", b"\0asm\x01\0\0\0");
        let report =
            run("com.example.one", &dirs(&root), &Options::default()).expect("a whole report");
        assert_eq!(report["ok"], false);
        assert_eq!(report["failed"], 1);
        assert_eq!(report["checks"][0]["name"], "component_loads");
        assert_eq!(report["checks"][0]["status"], "fail");
        assert_eq!(
            report["checks"][0]["error"]["code"], "plugin.load_failed",
            "{report:#}",
        );
        assert!(
            report["summary"]
                .as_str()
                .expect("a summary")
                .contains("1 failed"),
        );
        // The fixture was generated, because nothing shipped one, and it is a
        // project this build wrote itself.
        assert_eq!(report["fixture_source"], "generated");
        assert_eq!(report["worlds"][0], "command");
    }
}
