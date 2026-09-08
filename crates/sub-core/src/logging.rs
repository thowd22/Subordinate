//! `tracing` setup shared by every Subordinate binary.
//!
//! Logs always go to stderr, never stdout: `subordinate-mcp` speaks its
//! protocol on stdout and the CLI prints results there. Verbosity comes from an
//! env-filter directive and the output is either human text or one JSON object
//! per event for machine consumers.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

use crate::error::{SubError, SubResult, codes};

/// Environment variable holding the env-filter directive (`sub_media=debug`).
pub const FILTER_ENV: &str = "SUBORDINATE_LOG";
/// Fallback filter variable, for people who reach for it out of habit.
pub const FILTER_ENV_FALLBACK: &str = "RUST_LOG";
/// Environment variable selecting the output format: `text` or `json`.
pub const FORMAT_ENV: &str = "SUBORDINATE_LOG_FORMAT";

/// How log events are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable single-line text.
    #[default]
    Text,
    /// One JSON object per event, for agents and log shippers.
    Json,
}

impl LogFormat {
    /// Parses `text` or `json`, case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns a [`SubError`] with code `core.invalid_argument` for any other
    /// value.
    pub fn parse(value: &str) -> SubResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" | "plain" | "" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(SubError::new(
                codes::INVALID_ARGUMENT,
                "log format must be `text` or `json`",
            )
            .with_detail("value", other)
            .with_detail("variable", FORMAT_ENV)),
        }
    }
}

/// Resolved logging configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    /// An `EnvFilter` directive, e.g. `info,sub_media=debug`.
    pub filter: String,
    /// Text or JSON output.
    pub format: LogFormat,
    /// Whether to colour text output.
    pub ansi: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            filter: "info".to_owned(),
            format: LogFormat::Text,
            ansi: false,
        }
    }
}

impl LogConfig {
    /// Reads the configuration from the process environment, colouring text
    /// output when stderr is a terminal and `NO_COLOR` is unset.
    ///
    /// # Errors
    ///
    /// Returns a [`SubError`] if `SUBORDINATE_LOG_FORMAT` holds an unknown
    /// value.
    pub fn from_env(default_filter: &str) -> SubResult<Self> {
        let config = Self::resolve(default_filter, |key| std::env::var(key).ok())?;
        Ok(Self {
            ansi: config.format == LogFormat::Text
                && std::io::stderr().is_terminal()
                && std::env::var_os("NO_COLOR").is_none(),
            ..config
        })
    }

    /// Resolves the configuration from an arbitrary variable lookup, which is
    /// what makes the precedence rules testable.
    ///
    /// # Errors
    ///
    /// Returns a [`SubError`] if the format variable holds an unknown value.
    pub fn resolve(default_filter: &str, var: impl Fn(&str) -> Option<String>) -> SubResult<Self> {
        let filter = var(FILTER_ENV)
            .or_else(|| var(FILTER_ENV_FALLBACK))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default_filter.to_owned());
        let format = match var(FORMAT_ENV) {
            Some(value) => LogFormat::parse(&value)?,
            None => LogFormat::default(),
        };
        Ok(Self {
            filter,
            format,
            ansi: false,
        })
    }
}

/// Installs the global tracing subscriber from the environment.
///
/// Call this once, first thing in `main`. `default_filter` applies when neither
/// `SUBORDINATE_LOG` nor `RUST_LOG` is set.
///
/// # Errors
///
/// Returns a [`SubError`] if the filter directive or the format variable is
/// invalid, or if a global subscriber is already installed.
pub fn init(default_filter: &str) -> SubResult<()> {
    init_with(&LogConfig::from_env(default_filter)?)
}

/// Installs the global tracing subscriber from an explicit configuration.
///
/// # Errors
///
/// Returns a [`SubError`] if the filter directive cannot be parsed or a global
/// subscriber is already installed.
pub fn init_with(config: &LogConfig) -> SubResult<()> {
    let filter = EnvFilter::try_new(&config.filter).map_err(|err| {
        SubError::wrap(
            codes::INVALID_ARGUMENT,
            "invalid log filter directive",
            &err,
        )
        .with_detail("filter", config.filter.clone())
        .with_detail("variable", FILTER_ENV)
    })?;

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true);

    let result = if config.format == LogFormat::Json {
        builder.json().flatten_event(true).try_init()
    } else {
        builder.with_ansi(config.ansi).try_init()
    };

    result.map_err(|err| {
        SubError::new(
            codes::LOGGING_INIT,
            "a global tracing subscriber is already installed",
        )
        .with_detail("cause", err.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn default_filter_applies_when_nothing_is_set() {
        let config = LogConfig::resolve("info", vars(&[])).unwrap();
        assert_eq!(config.filter, "info");
        assert_eq!(config.format, LogFormat::Text);
    }

    #[test]
    fn subordinate_log_wins_over_rust_log() {
        let config = LogConfig::resolve(
            "info",
            vars(&[
                (FILTER_ENV, "sub_media=debug"),
                (FILTER_ENV_FALLBACK, "warn"),
            ]),
        )
        .unwrap();
        assert_eq!(config.filter, "sub_media=debug");

        let config = LogConfig::resolve("info", vars(&[(FILTER_ENV_FALLBACK, "warn")])).unwrap();
        assert_eq!(config.filter, "warn");
    }

    #[test]
    fn blank_filter_variables_fall_back_to_the_default() {
        let config = LogConfig::resolve("info", vars(&[(FILTER_ENV, "  ")])).unwrap();
        assert_eq!(config.filter, "info");
    }

    #[test]
    fn json_format_is_selected_by_the_format_variable() {
        let config = LogConfig::resolve("info", vars(&[(FORMAT_ENV, "JSON")])).unwrap();
        assert_eq!(config.format, LogFormat::Json);
    }

    #[test]
    fn an_unknown_format_is_a_sub_error() {
        let err = LogConfig::resolve("info", vars(&[(FORMAT_ENV, "yaml")])).unwrap_err();
        assert_eq!(err.code, codes::INVALID_ARGUMENT);
        assert_eq!(
            err.details.get("value").and_then(serde_json::Value::as_str),
            Some("yaml")
        );
    }

    #[test]
    fn an_invalid_filter_directive_is_a_sub_error() {
        let config = LogConfig {
            filter: "not a filter=!!".to_owned(),
            ..LogConfig::default()
        };
        let err = init_with(&config).unwrap_err();
        assert_eq!(err.code, codes::INVALID_ARGUMENT);
    }

    #[test]
    fn init_succeeds_once_and_then_reports_a_double_install() {
        let config = LogConfig::default();
        assert!(init_with(&config).is_ok());
        let err = init_with(&config).unwrap_err();
        assert_eq!(err.code, codes::LOGGING_INIT);
    }
}
