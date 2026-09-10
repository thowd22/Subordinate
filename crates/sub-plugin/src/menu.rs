//! Plugin-contributed commands: what a `commands`-world plugin registers, and
//! how the host turns one run into one undo step.
//!
//! A plugin in the `commands` world answers `commands()` once, when it loads,
//! with a [`CommandDesc`] per entry it contributes. The host validates those
//! descriptions, qualifies each id with the plugin's own id and keeps them in a
//! [`PluginCommandRegistry`]; the Plugins menu and the keyboard map are then
//! answerable without instantiating anything, which is what lets the editor
//! draw a menu every frame over plugins it has not run.
//!
//! Running is the other half: [`run_as_undo_group`] wraps a whole plugin run in
//! one [`EngineHandle`] command group, so a command that applies twenty
//! primitives through the Command API is still a single Ctrl+Z for the user,
//! and a run that fails part way leaves the project as it found it.

use sub_core::{SubError, SubResult};
use sub_edit::EngineHandle;

use crate::codes;

/// What one command invocation acts on: the project, and the arguments as
/// JSON. Straight from the WIT; the host builds one per menu click or chord.
pub use crate::bindings::menu::subordinate::plugin::command_menu::CommandContext;
/// The description of one command as a plugin declares it: `id`, `title` and
/// an optional `shortcut`, straight from the WIT.
pub use crate::bindings::menu::subordinate::plugin::command_menu::CommandDesc;

/// The character that joins a plugin id and a command id into the qualified id
/// the host, the keymap file and the menu all use.
pub const QUALIFIED_SEPARATOR: char = '/';

/// The longest identifier a plugin or a command may have.
const MAX_IDENTIFIER_LEN: usize = 128;

/// One command a plugin contributes, after the host has validated it.
///
/// The plugin does not choose its own qualified id: two plugins may both call a
/// command `split`, and it is the host that keeps them apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginCommand {
    plugin: String,
    id: String,
    title: String,
    shortcut: Option<String>,
    qualified_id: String,
}

impl PluginCommand {
    /// The id of the plugin that contributed this command.
    #[must_use]
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// The plugin-local command id, as the plugin declared it.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The human-readable name the Plugins menu shows.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The chord the plugin asked for, if it asked for one. A request: the
    /// host's own actions and the user's keymap win.
    #[must_use]
    pub fn shortcut(&self) -> Option<&str> {
        self.shortcut.as_deref()
    }

    /// `<plugin id>/<command id>`: the name the host, the menu and a keymap
    /// file use for this command.
    #[must_use]
    pub fn qualified_id(&self) -> &str {
        &self.qualified_id
    }

    /// The label the undo step gets when this command runs.
    #[must_use]
    pub fn undo_label(&self) -> String {
        self.title.clone()
    }
}

/// Every plugin-contributed command the host currently knows about.
///
/// Registration is per plugin and replaces whatever that plugin registered
/// before, which is what a hot reload needs (docs/PLAN.md §6.4). Order is
/// registration order, then declaration order within a plugin, so the menu does
/// not shuffle itself between frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginCommandRegistry {
    commands: Vec<PluginCommand>,
}

impl PluginCommandRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers everything `plugin` contributes, replacing its previous
    /// entries.
    ///
    /// Either every description is accepted or none is: a plugin that declares
    /// one bad command does not end up half registered.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_PLUGIN_ID`] when `plugin` is not a valid identifier,
    /// [`codes::INVALID_COMMAND_ID`], [`codes::INVALID_COMMAND_TITLE`] or
    /// [`codes::INVALID_SHORTCUT`] when a description is malformed, and
    /// [`codes::DUPLICATE_COMMAND`] when the plugin declares the same id twice.
    pub fn register(&mut self, plugin: &str, descs: &[CommandDesc]) -> SubResult<()> {
        check_identifier(plugin).map_err(|why| {
            SubError::new(
                codes::INVALID_PLUGIN_ID,
                format!("plugin id {plugin:?} is not a valid identifier: {why}"),
            )
            .with_detail("plugin", plugin)
        })?;

        let mut accepted: Vec<PluginCommand> = Vec::with_capacity(descs.len());
        for desc in descs {
            let command = validate(plugin, desc)?;
            if accepted.iter().any(|other| other.id == command.id) {
                return Err(SubError::new(
                    codes::DUPLICATE_COMMAND,
                    format!("plugin {plugin:?} declares command {:?} twice", command.id),
                )
                .with_detail("plugin", plugin)
                .with_detail("command", &command.id));
            }
            accepted.push(command);
        }

        self.remove_plugin(plugin);
        self.commands.extend(accepted);
        Ok(())
    }

    /// Forgets everything `plugin` registered, returning how many commands
    /// went. What an uninstall, a disable or the first half of a reload does.
    pub fn remove_plugin(&mut self, plugin: &str) -> usize {
        let before = self.commands.len();
        self.commands.retain(|command| command.plugin != plugin);
        before - self.commands.len()
    }

    /// Every registered command, in menu order.
    #[must_use]
    pub fn commands(&self) -> &[PluginCommand] {
        &self.commands
    }

    /// The command a qualified id names, if it is still registered.
    #[must_use]
    pub fn get(&self, qualified_id: &str) -> Option<&PluginCommand> {
        self.commands
            .iter()
            .find(|command| command.qualified_id == qualified_id)
    }

    /// The command a qualified id names.
    ///
    /// # Errors
    ///
    /// [`codes::UNKNOWN_COMMAND`] when no registered command has that id: a
    /// menu entry outlived its plugin, or a keymap file names a command that
    /// was never installed.
    pub fn require(&self, qualified_id: &str) -> SubResult<&PluginCommand> {
        self.get(qualified_id).ok_or_else(|| {
            SubError::new(
                codes::UNKNOWN_COMMAND,
                format!("no plugin command is registered as {qualified_id:?}"),
            )
            .with_detail("command", qualified_id)
        })
    }

    /// The plugins that have registered a command, in registration order.
    #[must_use]
    pub fn plugins(&self) -> Vec<&str> {
        let mut plugins: Vec<&str> = Vec::new();
        for command in &self.commands {
            if !plugins.contains(&command.plugin.as_str()) {
                plugins.push(&command.plugin);
            }
        }
        plugins
    }

    /// How many commands are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether no plugin has registered a command.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Runs one plugin command as a single undo step.
///
/// `run` is the call into the plugin; everything it applies through the
/// Command API lands inside one [`EngineHandle`] command group labelled with
/// the command's title. A run that fails takes its group down with it, so a
/// half-finished plugin command is left neither on the project nor on the undo
/// stack.
///
/// # Errors
///
/// Whatever `run` returns, or the engine's own `edit.group_open` /
/// `edit.engine_stopped` converted into `E` when the group cannot be opened or
/// committed.
pub fn run_as_undo_group<T, E: From<SubError>>(
    engine: &EngineHandle,
    command: &PluginCommand,
    run: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    engine.begin_group(command.undo_label())?;
    match run() {
        Ok(value) => {
            engine.commit_group()?;
            Ok(value)
        }
        Err(error) => {
            // The run already failed; an abort that also fails must not hide
            // why. The engine broadcasts the rollback either way.
            drop(engine.abort_group());
            Err(error)
        }
    }
}

/// Validates one description and qualifies it with its plugin's id.
fn validate(plugin: &str, desc: &CommandDesc) -> SubResult<PluginCommand> {
    check_identifier(&desc.id).map_err(|why| {
        SubError::new(
            codes::INVALID_COMMAND_ID,
            format!("command id {:?} is not a valid identifier: {why}", desc.id),
        )
        .with_detail("plugin", plugin)
        .with_detail("command", &desc.id)
    })?;

    let title = desc.title.trim();
    if title.is_empty() || title.contains(['\n', '\r']) {
        return Err(SubError::new(
            codes::INVALID_COMMAND_TITLE,
            "a command title must be one non-empty line",
        )
        .with_detail("plugin", plugin)
        .with_detail("command", &desc.id));
    }

    let shortcut = match desc.shortcut.as_deref().map(str::trim) {
        None => None,
        Some("") => {
            return Err(SubError::new(
                codes::INVALID_SHORTCUT,
                "a requested shortcut must not be empty; leave it unset instead",
            )
            .with_detail("plugin", plugin)
            .with_detail("command", &desc.id));
        }
        Some(chord) => Some(chord.to_owned()),
    };

    Ok(PluginCommand {
        plugin: plugin.to_owned(),
        id: desc.id.clone(),
        title: title.to_owned(),
        shortcut,
        qualified_id: format!("{plugin}{QUALIFIED_SEPARATOR}{}", desc.id),
    })
}

/// Whether `id` is a dot-separated identifier of `[a-z0-9_-]` segments.
///
/// The same shape as an [`ErrorCode`](sub_core::ErrorCode) and as the action
/// ids a keymap file names, so one lowercase, punctuation-free vocabulary spans
/// the manifest, the menu and the keymap.
fn check_identifier(id: &str) -> Result<(), &'static str> {
    if id.is_empty() {
        return Err("it is empty");
    }
    if id.len() > MAX_IDENTIFIER_LEN {
        return Err("it is longer than 128 characters");
    }
    for segment in id.split('.') {
        if segment.is_empty() {
            return Err("it has an empty dot-separated segment");
        }
        let plain = segment.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        });
        if !plain {
            return Err("segments may hold only [a-z0-9_-]");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CommandDesc, PluginCommandRegistry, check_identifier};

    fn desc(id: &str, title: &str, shortcut: Option<&str>) -> CommandDesc {
        CommandDesc {
            id: id.to_owned(),
            title: title.to_owned(),
            shortcut: shortcut.map(str::to_owned),
        }
    }

    #[test]
    fn registration_qualifies_ids_and_keeps_declaration_order() {
        let mut registry = PluginCommandRegistry::new();
        registry
            .register(
                "com.example.silence-cutter",
                &[
                    desc("cut-silence", "Cut silence", Some("Ctrl+Shift+K")),
                    desc("report", "  Silence report  ", None),
                ],
            )
            .expect("both descriptions are valid");

        let commands = registry.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0].qualified_id(),
            "com.example.silence-cutter/cut-silence"
        );
        assert_eq!(commands[0].shortcut(), Some("Ctrl+Shift+K"));
        // The title is trimmed, and it is what an undo step is labelled with.
        assert_eq!(commands[1].title(), "Silence report");
        assert_eq!(commands[1].undo_label(), "Silence report");
        assert!(registry.get("com.example.silence-cutter/report").is_some());
        assert_eq!(registry.plugins(), vec!["com.example.silence-cutter"]);
    }

    #[test]
    fn two_plugins_may_use_the_same_command_id() {
        let mut registry = PluginCommandRegistry::new();
        registry
            .register("a", &[desc("split", "Split", None)])
            .unwrap();
        registry
            .register("b", &[desc("split", "Split", None)])
            .unwrap();
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.get("a/split").unwrap().plugin(), "a");
        assert_eq!(registry.get("b/split").unwrap().plugin(), "b");
    }

    #[test]
    fn re_registering_a_plugin_replaces_its_commands_and_leaves_others_alone() {
        let mut registry = PluginCommandRegistry::new();
        registry.register("a", &[desc("one", "One", None)]).unwrap();
        registry.register("b", &[desc("two", "Two", None)]).unwrap();
        registry
            .register("a", &[desc("three", "Three", None)])
            .unwrap();

        assert!(registry.get("a/one").is_none(), "the old entry is gone");
        assert!(registry.get("a/three").is_some());
        assert!(registry.get("b/two").is_some());

        assert_eq!(registry.remove_plugin("a"), 1);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn a_bad_description_is_refused_whole_with_a_stable_code() {
        let mut registry = PluginCommandRegistry::new();

        let err = registry
            .register("Not An Id", &[desc("ok", "Ok", None)])
            .unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_plugin_id");

        let err = registry
            .register("a", &[desc("ok", "Ok", None), desc("Bad Id", "Bad", None)])
            .unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_command_id");
        assert!(
            registry.is_empty(),
            "a rejected batch registers nothing at all"
        );

        let err = registry
            .register("a", &[desc("ok", "   ", None)])
            .unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_command_title");

        let err = registry
            .register("a", &[desc("ok", "Ok", Some("  "))])
            .unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.invalid_shortcut");

        let err = registry
            .register("a", &[desc("ok", "One", None), desc("ok", "Two", None)])
            .unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.duplicate_command");

        let err = registry.require("a/ok").unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.unknown_command");
    }

    #[test]
    fn identifiers_are_lowercase_dotted_segments() {
        assert!(check_identifier("com.example.plug-in_1").is_ok());
        assert!(check_identifier("").is_err());
        assert!(check_identifier("a..b").is_err());
        assert!(check_identifier("Upper").is_err());
        assert!(check_identifier("has/slash").is_err());
        assert!(check_identifier(&"a".repeat(129)).is_err());
    }
}
