//! The `plugin` subcommands: what is installed, and which of it is switched on.
//!
//! `subordinate-cli plugin list|enable|disable|remove` acts on the plugin
//! directories directly (`sub_plugin::registry`), so it works with no editor
//! running — the registry caches nothing and every call re-reads the disk, so
//! a running editor sees the same answer on its next scan. The same four
//! operations reach an agent as MCP tools, because
//! [`sub_plugin::registry::register_methods`] puts them on the Command API the
//! bridge forwards to (docs/PLAN.md §6.4, §7).
//!
//! Every subcommand prints JSON, like the rest of the CLI: the listing carries
//! the two directories it walked, the plugins it loaded, the ones that failed
//! to load and the ones hidden by an id conflict.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sub_core::{SubError, SubResult};
use sub_plugin::manifest::PluginId;
use sub_plugin::registry::{PluginDirs, PluginRegistry, default_user_dir};

/// What `plugin` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// List everything installed.
    List,
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
    let registry = PluginRegistry::new(dirs.clone());
    match action {
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
pub const NEEDS_ACTION: &str = "plugin needs one of list, enable, disable or remove";

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

    #[test]
    fn a_malformed_id_is_reported_before_the_disk_is_touched() {
        let root = scratch("bad-id");
        let error =
            run(&Action::Enable("nodots".to_owned()), &options(&root)).expect_err("a bad id");
        assert_eq!(error.code.as_str(), "plugin.invalid_plugin_id");
        assert_eq!(error.details["id"], "nodots");
    }
}
