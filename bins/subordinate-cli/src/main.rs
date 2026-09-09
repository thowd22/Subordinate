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
  subordinate-cli --help       print this text
";

/// Parses the arguments after the program name.
fn parse(args: &[String]) -> Command {
    let mut args = args.iter().map(String::as_str);
    match args.next() {
        None => Command::Version,
        Some("--help" | "-h" | "help") => Command::Help,
        Some("diag") => {
            let mut pretty = true;
            for arg in args {
                match arg {
                    "--compact" => pretty = false,
                    "--pretty" | "--json" => pretty = true,
                    other => return Command::Unknown(other.to_owned()),
                }
            }
            Command::Diag { pretty }
        }
        Some(other) => Command::Unknown(other.to_owned()),
    }
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
    let text = if pretty {
        serde_json::to_string_pretty(&json)
    } else {
        serde_json::to_string(&json)
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
    }
}
