//! A minimal `subordinate:plugin/command` plugin.
//!
//! It reads the playhead, then calls the Command API host import to add a
//! marker there. It never touches the project model directly, so the edit lands
//! on the host's undo stack like any other command.

wit_bindgen::generate!({
    path: "../../../../wit",
    world: "command",
});

use subordinate::plugin::command_api;
use subordinate::plugin::types::Detail;

struct Plugin;

export!(Plugin);

impl Guest for Plugin {
    fn run(project: ProjectId, args: String) -> Result<String, Error> {
        command_api::log(command_api::LogLevel::Info, "marker plugin starting");

        let label = label_from(&args)?;
        let at = command_api::playhead(&project)?;

        let params = format!(
            r#"{{"name":{},"at":{{"value":{},"rate_numerator":{},"rate_denominator":{}}}}}"#,
            json_string(&label),
            at.value,
            at.rate.numerator,
            at.rate.denominator
        );
        let response = command_api::run_command(&project, "sequence.add_marker", &params)?;

        command_api::log(command_api::LogLevel::Info, "marker plugin done");
        Ok(response)
    }
}

/// Extracts the `label` string from the `args` JSON object.
///
/// Hand-rolled rather than pulling `serde_json` in: the spike is measuring the
/// component boundary, and a guest that parses JSON properly belongs in the SDK
/// crate (TASK-89), not here.
fn label_from(args: &str) -> Result<String, Error> {
    let key = "\"label\"";
    let after_key = args.find(key).map(|i| i + key.len()).ok_or_else(|| {
        error(
            "plugin.invalid_argument",
            "args must contain a `label` string",
        )
    })?;
    let rest = args[after_key..].trim_start();
    let rest = rest.strip_prefix(':').map(str::trim_start).ok_or_else(|| {
        error(
            "plugin.invalid_argument",
            "`label` must be followed by a value",
        )
    })?;
    let rest = rest
        .strip_prefix('"')
        .ok_or_else(|| error("plugin.invalid_argument", "`label` must be a string"))?;
    let end = rest
        .find('"')
        .ok_or_else(|| error("plugin.invalid_argument", "`label` string is unterminated"))?;
    Ok(rest[..end].to_owned())
}

/// Quotes and escapes `value` as a JSON string.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Builds a `SubError`-shaped failure to return across the component boundary.
fn error(code: &str, message: &str) -> Error {
    Error {
        code: code.to_owned(),
        message: message.to_owned(),
        details: Vec::<Detail>::new(),
    }
}
