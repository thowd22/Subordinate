//! The `plugin` subcommands: what is installed, and which of it is switched on.
//!
//! `subordinate-cli plugin install|list|enable|disable|remove|reload` acts on the plugin
//! directories directly (`sub_plugin::registry`), so it works with no editor
//! running — the registry caches nothing and every call re-reads the disk, so
//! a running editor sees the same answer on its next scan. The same four
//! operations reach an agent as MCP tools, because
//! [`sub_plugin::registry::register_methods`] puts them on the Command API the
//! bridge forwards to (docs/PLAN.md §6.4, §7).
//!
//! `plugin install <path> --dev` links the built component and the source
//! tree it was built from into the plugin directory and records where they
//! came from, which is what makes a running host watch and hot-reload it
//! (docs/PLAN.md §6.4). Install and `plugin reload` both load the component
//! here as well, with a loader that compiles it and nothing more — this
//! process runs no plugins — so a component that will not load is reported to
//! the caller as a structured error straight away instead of failing later
//! inside the editor.
//!
//! Every subcommand prints JSON, like the rest of the CLI: the listing carries
//! the two directories it walked, the plugins it loaded, the ones that failed
//! to load and the ones hidden by an id conflict.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sub_core::{SubError, SubResult};
use sub_plugin::dev::{self, DevHost};
use sub_plugin::manifest::PluginId;
use sub_plugin::registry::{InstallLocation, PluginDirs, PluginRegistry, default_user_dir};
use sub_plugin::runtime::PluginRuntime;

/// What `plugin` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Install a plugin from a built component or a plugin directory.
    Install {
        /// The `.wasm` that was built, or the directory holding `plugin.toml`.
        path: PathBuf,
        /// Link it to its sources and record them, so a running host watches
        /// and hot-reloads it.
        dev: bool,
        /// Which plugin directory to install into. Defaults to the per-user
        /// one.
        location: Option<InstallLocation>,
    },
    /// List everything installed.
    List,
    /// Load one installed plugin again.
    Reload(String),
    /// Switch one plugin on.
    Enable(String),
    /// Switch one plugin off.
    Disable(String),
    /// Delete one plugin from disk.
    Remove(String),
}

/// Which directories the subcommand looks in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Overrides the per-user plugin directory. Tests set it; a user has no
    /// reason to.
    pub user_dir: Option<PathBuf>,
    /// A project file, whose `.subordinate/plugins` directory joins the walk.
    pub project: Option<PathBuf>,
}

impl Options {
    /// The plugin directories these options name.
    ///
    /// # Errors
    ///
    /// `plugin.plugin_dir_unavailable` when no directory was given and the
    /// platform offers no per-user data directory.
    pub fn dirs(&self) -> SubResult<PluginDirs> {
        let user = match &self.user_dir {
            Some(dir) => dir.clone(),
            None => default_user_dir()?,
        };
        let dirs = PluginDirs::new(user);
        Ok(match &self.project {
            Some(project) => dirs.with_project_file(project),
            None => dirs,
        })
    }
}

/// Runs one `plugin` subcommand and answers what it did as JSON.
///
/// # Errors
///
/// `plugin.invalid_plugin_id` for an id that is not reverse-DNS, and whatever
/// the registry returns: `plugin.not_installed`, an unreadable plugin
/// directory, or a `plugins.json` this build cannot read.
pub fn run(action: &Action, options: &Options) -> SubResult<Value> {
    let dirs = options.dirs()?;
    let registry = Arc::new(PluginRegistry::new(dirs.clone()));
    match action {
        Action::Install {
            path,
            dev,
            location,
        } => {
            let mut host = host(registry);
            let installed = host.install(*location, path, *dev)?;
            let mut report = value(&installed)?;
            report["status"] = value(&host.status(&installed.id))?;
            Ok(report)
        }
        Action::Reload(id) => value(&host(registry).reload(&parse_id(id)?)?),
        Action::List => {
            let scan = registry.scan()?;
            let mut report = serde_json::to_value(&scan).map_err(|err| {
                SubError::wrap(
                    sub_core::codes::INTERNAL,
                    "the plugin listing could not be serialised",
                    &err,
                )
            })?;
            report["user_dir"] = display(dirs.user());
            report["project_dir"] = dirs.project().map_or(Value::Null, display);
            Ok(report)
        }
        Action::Enable(id) => value(&registry.set_enabled(&parse_id(id)?, true)?),
        Action::Disable(id) => value(&registry.set_enabled(&parse_id(id)?, false)?),
        Action::Remove(id) => value(&registry.remove(&parse_id(id)?)?),
    }
}

/// A host over `registry` whose loader only compiles a component: the CLI runs
/// no plugins, so there is nothing to instantiate, but a component that is not
/// one is still reported here rather than in the editor.
fn host(registry: Arc<PluginRegistry>) -> DevHost {
    match PluginRuntime::new() {
        Ok(runtime) => DevHost::with_loader(registry, dev::compile_only_loader(runtime)),
        // A build with no compiler cannot check the component; installing and
        // reloading still work, and the editor reports the same error when it
        // tries to run it.
        Err(error) => {
            tracing::warn!(
                code = error.code.as_str(),
                "the component is installed unchecked: {}",
                error.message,
            );
            DevHost::new(registry, |_, _| {
                Ok(sub_plugin::dev::PluginArtifacts::default())
            })
        }
    }
}

/// A path as the JSON string the reports carry.
fn display(path: &Path) -> Value {
    Value::String(path.display().to_string())
}

/// One registry answer as JSON.
fn value<T: Serialize>(answer: &T) -> SubResult<Value> {
    serde_json::to_value(answer).map_err(|err| {
        SubError::wrap(
            sub_core::codes::INTERNAL,
            "the plugin answer could not be serialised",
            &err,
        )
    })
}

/// Parses an id from the command line, reporting a bad one with the stable
/// code the host uses everywhere else.
fn parse_id(id: &str) -> SubResult<PluginId> {
    PluginId::parse(id).map_err(|err| {
        SubError::new(
            sub_plugin::codes::INVALID_PLUGIN_ID,
            "a plugin id is two or more dot-separated lowercase segments",
        )
        .with_detail("id", err.0)
    })
}

/// What a caller is told when `plugin` names no subcommand.
pub const NEEDS_ACTION: &str =
    "plugin needs one of install, list, enable, disable, remove or reload";

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Action, Options, run};

    /// A scratch user plugin directory holding one installed plugin.
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("subordinate-cli-plugin-{name}"));
        std::fs::remove_dir_all(&root).ok();
        let dir = root.join("com.example.one");
        std::fs::create_dir_all(&dir).expect("a plugin directory");
        std::fs::write(
            dir.join("plugin.toml"),
            "[plugin]\nid = \"com.example.one\"\nname = \"One\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"command\"]\n",
        )
        .expect("a manifest");
        root
    }

    /// Options pointed at a scratch directory and no project.
    fn options(root: &Path) -> Options {
        Options {
            user_dir: Some(root.to_path_buf()),
            project: None,
        }
    }

    #[test]
    fn listing_reports_the_directories_it_walked() {
        let root = scratch("list");
        let report = run(&Action::List, &options(&root)).expect("a listing");
        assert_eq!(report["plugins"][0]["id"], "com.example.one");
        assert_eq!(report["plugins"][0]["enabled"], true);
        assert_eq!(report["user_dir"], root.display().to_string());
        assert_eq!(report["project_dir"], serde_json::Value::Null);
        assert!(report["failures"].as_array().expect("failures").is_empty());
    }

    #[test]
    fn a_project_file_adds_its_own_plugin_directory() {
        let root = scratch("project");
        let options = Options {
            user_dir: Some(root.clone()),
            project: Some(root.join("edit.subproj")),
        };
        let report = run(&Action::List, &options).expect("a listing");
        let project_dir = report["project_dir"].as_str().expect("a project directory");
        assert!(project_dir.ends_with("plugins"), "{project_dir}");
        assert!(project_dir.contains(".subordinate"), "{project_dir}");
    }

    #[test]
    fn enable_disable_and_remove_round_trip() {
        let root = scratch("cycle");
        let options = options(&root);
        let id = "com.example.one".to_owned();

        let off = run(&Action::Disable(id.clone()), &options).expect("disabled");
        assert_eq!(off["enabled"], false);
        assert_eq!(off["changed"], true);
        let listed = run(&Action::List, &options).expect("a listing");
        assert_eq!(listed["plugins"][0]["enabled"], false);

        let on = run(&Action::Enable(id.clone()), &options).expect("enabled");
        assert_eq!(on["enabled"], true);

        let removed = run(&Action::Remove(id.clone()), &options).expect("removed");
        assert_eq!(removed["id"], "com.example.one");
        assert_eq!(removed["location"], "user");
        let listed = run(&Action::List, &options).expect("a listing");
        assert!(listed["plugins"].as_array().expect("plugins").is_empty());

        let error = run(&Action::Enable(id), &options).expect_err("no longer installed");
        assert_eq!(error.code.as_str(), "plugin.not_installed");
    }

    /// A source tree as a plugin author's crate looks after a build: a
    /// manifest and a component in a build directory below it.
    fn source(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("subordinate-cli-plugin-src-{name}"));
        std::fs::remove_dir_all(&root).ok();
        let built = root.join("target").join("wasm32-wasip2").join("release");
        std::fs::create_dir_all(&built).expect("a build directory");
        std::fs::write(
            root.join("plugin.toml"),
            "[plugin]\nid = \"com.example.one\"\nname = \"One\"\nversion = \"0.1.0\"\n\
             api = \"0.1\"\nworlds = [\"command\"]\n",
        )
        .expect("a manifest");
        let wasm = built.join("one.wasm");
        // A core module header: enough to install, not a component, which is
        // what makes the load failure below the real one a caller would see.
        std::fs::write(&wasm, b"\0asm\x01\0\0\0").expect("a component");
        (root, wasm)
    }

    #[test]
    fn a_dev_install_links_the_built_component_and_is_listed_as_dev() {
        let user = std::env::temp_dir().join("subordinate-cli-plugin-install");
        std::fs::remove_dir_all(&user).ok();
        let (root, wasm) = source("install");
        let options = options(&user);

        // The component is not a real one, so the install reports the load
        // failure — with its stable code — and leaves the plugin installed.
        let error = run(
            &Action::Install {
                path: wasm,
                dev: true,
                location: None,
            },
            &options,
        )
        .expect_err("the component does not load");
        assert_eq!(error.code.as_str(), "plugin.load_failed");

        let listed = run(&Action::List, &options).expect("a listing");
        assert_eq!(listed["plugins"][0]["id"], "com.example.one");
        assert_eq!(listed["plugins"][0]["dev"], true);
        assert!(
            user.join("com.example.one").join("plugin.wasm").exists(),
            "the built component is linked in under the name the host loads"
        );

        // Reloading answers the same structured error, which is what the CLI
        // and an agent over MCP see after a bad build.
        let error = run(&Action::Reload("com.example.one".to_owned()), &options)
            .expect_err("still not a component");
        assert_eq!(error.code.as_str(), "plugin.load_failed");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&user).ok();
    }

    #[test]
    fn installing_something_that_is_not_a_plugin_says_so() {
        let user = std::env::temp_dir().join("subordinate-cli-plugin-install-bad");
        std::fs::remove_dir_all(&user).ok();
        let error = run(
            &Action::Install {
                path: user.join("nothing.wasm"),
                dev: true,
                location: None,
            },
            &options(&user),
        )
        .expect_err("there is nothing there");
        assert_eq!(error.code.as_str(), "plugin.dev_source_invalid");
    }

    #[test]
    fn a_malformed_id_is_reported_before_the_disk_is_touched() {
        let root = scratch("bad-id");
        let error =
            run(&Action::Enable("nodots".to_owned()), &options(&root)).expect_err("a bad id");
        assert_eq!(error.code.as_str(), "plugin.invalid_plugin_id");
        assert_eq!(error.details["id"], "nodots");
    }
}
