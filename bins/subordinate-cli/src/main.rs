//! Headless command-line interface.
//!
//! Loads, saves, inspects and renders projects without a GUI, serves the
//! Command API for out-of-process clients, and scaffolds and tests plugins.
//! It is also the CI smoke test.

use std::process::ExitCode;

/// What the arguments asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// No subcommand: print the version, as the CI smoke test expects.
    Version,
    /// Print the usage text.
    Help,
    /// Print the hardware diagnostics as JSON.
    Diag { pretty: bool },
    /// Print the JSON Schema of the Command API.
    Schema { pretty: bool },
    /// Anything unrecognised, reported with the offending argument.
    Unknown(String),
}

/// The usage text, kept next to the parser so the two cannot drift.
const USAGE: &str = "\
subordinate-cli - headless interface to Subordinate

Usage:
  subordinate-cli              print the version
  subordinate-cli diag         print hardware diagnostics as JSON
  subordinate-cli diag --compact
                               the same JSON on one line
  subordinate-cli schema       print the Command API JSON Schema
  subordinate-cli schema --compact
                               the same JSON on one line
  subordinate-cli --help       print this text
";

/// Parses the arguments after the program name.
fn parse(args: &[String]) -> Command {
    let mut args = args.iter().map(String::as_str);
    match args.next() {
        None => Command::Version,
        Some("--help" | "-h" | "help") => Command::Help,
        Some("diag") => match json_flags(args) {
            Ok(pretty) => Command::Diag { pretty },
            Err(unknown) => unknown,
        },
        Some("schema") => match json_flags(args) {
            Ok(pretty) => Command::Schema { pretty },
            Err(unknown) => unknown,
        },
        Some(other) => Command::Unknown(other.to_owned()),
    }
}

/// Reads the `--compact`, `--pretty` and `--json` flags the JSON-printing
/// subcommands share, or reports the first argument that is none of them.
fn json_flags<'a>(args: impl Iterator<Item = &'a str>) -> Result<bool, Command> {
    let mut pretty = true;
    for arg in args {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            other => return Err(Command::Unknown(other.to_owned())),
        }
    }
    Ok(pretty)
}

/// Prints `value` as JSON, one line when `pretty` is false.
fn print_json(value: &serde_json::Value, pretty: bool) -> ExitCode {
    let text = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    match text {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// Generates the Command API JSON Schema and prints it.
///
/// The document is produced by the code, never written by hand: a dispatcher
/// over a throwaway engine is asked to describe every method it serves, which
/// is the same table it dispatches through. The committed copy at
/// `docs/schema/command-api.json` is what this prints.
fn schema(pretty: bool) -> ExitCode {
    let engine = match sub_edit::Engine::spawn(sub_model::Project::new("schema")) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("{}", err.to_json());
            return ExitCode::FAILURE;
        }
    };
    let dispatcher = sub_command::Dispatcher::new(engine.handle().clone());
    let document = sub_command::schema::document(&dispatcher);
    let status = print_json(&document, pretty);
    if let Err(err) = engine.shutdown() {
        eprintln!("{}", err.to_json());
        return ExitCode::FAILURE;
    }
    status
}

/// Collects the diagnostics and prints them as JSON.
fn diag(pretty: bool) -> ExitCode {
    let diagnostics = match sub_media::HardwareDiagnostics::collect() {
        Ok(diagnostics) => diagnostics,
        Err(err) => {
            eprintln!("{}", err.to_json());
            return ExitCode::FAILURE;
        }
    };
    let mut json = match diagnostics.to_json() {
        Ok(json) => json,
        Err(err) => {
            eprintln!("{}", err.to_json());
            return ExitCode::FAILURE;
        }
    };
    // The registry says which encoders are installed; the probe says which of
    // them this machine can actually start, which is what export selection
    // goes by. The panel shows both, so the CLI prints both.
    match sub_export::EncoderProbe::cached().and_then(sub_export::EncoderProbe::to_json) {
        Ok(encoders) => json["encoder_probe"] = encoders,
        Err(err) => {
            eprintln!("{}", err.to_json());
            return ExitCode::FAILURE;
        }
    }
    print_json(&json, pretty)
}

fn main() -> ExitCode {
    if let Err(err) = sub_core::logging::init("info") {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Command::Version => {
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                "subordinate-cli starting"
            );
            println!("subordinate-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Diag { pretty } => diag(pretty),
        Command::Schema { pretty } => schema(pretty),
        Command::Unknown(arg) => {
            eprintln!("unknown argument: {arg}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, parse};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn no_arguments_still_prints_the_version() {
        assert_eq!(parse(&args(&[])), Command::Version);
    }

    #[test]
    fn diag_defaults_to_pretty_json_and_can_be_compacted() {
        assert_eq!(parse(&args(&["diag"])), Command::Diag { pretty: true });
        assert_eq!(
            parse(&args(&["diag", "--compact"])),
            Command::Diag { pretty: false }
        );
        assert_eq!(
            parse(&args(&["diag", "--json"])),
            Command::Diag { pretty: true }
        );
    }

    #[test]
    fn schema_defaults_to_pretty_json_and_can_be_compacted() {
        assert_eq!(parse(&args(&["schema"])), Command::Schema { pretty: true });
        assert_eq!(
            parse(&args(&["schema", "--compact"])),
            Command::Schema { pretty: false }
        );
        assert_eq!(
            parse(&args(&["schema", "--verbose"])),
            Command::Unknown("--verbose".to_owned())
        );
    }

    #[test]
    fn unknown_arguments_are_reported_rather_than_ignored() {
        assert_eq!(
            parse(&args(&["render"])),
            Command::Unknown("render".to_owned())
        );
        assert_eq!(
            parse(&args(&["diag", "--verbose"])),
            Command::Unknown("--verbose".to_owned())
        );
    }

    #[test]
    fn help_is_offered_under_every_spelling() {
        for spelling in ["--help", "-h", "help"] {
            assert_eq!(parse(&args(&[spelling])), Command::Help);
        }
        assert!(super::USAGE.contains("diag"));
        assert!(super::USAGE.contains("schema"));
    }
}
