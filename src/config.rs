//! Typed loading and deterministic rule resolution for Melibea configuration.

use std::{
    env,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::Deserialize;

use crate::attention::{AttentionPolicy, WidthParseError, WidthPolicy};

const CONFIG_DIRECTORY: &str = "melibea";
const CONFIG_FILENAME: &str = "config.toml";

/// Validated Melibea configuration.
#[derive(Clone, Debug)]
pub struct Config {
    attention: Vec<AttentionRule>,
}

impl Config {
    /// Parses and validates configuration from TOML text.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid TOML, unknown fields, an empty rule list,
    /// invalid matchers, or invalid width values.
    pub fn parse(input: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(input).map_err(ConfigError::Toml)?;
        Self::from_raw(raw)
    }

    /// Reads and validates configuration from `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or its contents are not a
    /// valid Melibea configuration.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::load_with_source(path).map(|(config, _)| config)
    }

    /// Reads configuration and returns both validated policy and source text.
    ///
    /// The source is useful for deterministic change detection without relying
    /// on filesystem timestamp precision.
    ///
    /// # Errors
    ///
    /// Returns the same contextual read and validation errors as [`Self::load`].
    pub fn load_with_source(path: &Path) -> Result<(Self, String), ConfigError> {
        let input = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;

        let config = Self::parse(&input).map_err(|error| ConfigError::AtPath {
            path: path.to_owned(),
            source: Box::new(error),
        })?;
        Ok((config, input))
    }

    /// Returns the configured attention rules in resolution order.
    #[must_use]
    pub fn attention_rules(&self) -> &[AttentionRule] {
        &self.attention
    }

    /// Returns the first rule matching `window` and its zero-based index.
    #[must_use]
    pub fn resolve(&self, window: WindowIdentity<'_>) -> Option<ResolvedRule<'_>> {
        self.attention
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.matches(window))
            .map(|(index, rule)| ResolvedRule { index, rule })
    }

    fn from_raw(raw: RawConfig) -> Result<Self, ConfigError> {
        if raw.attention.is_empty() {
            return Err(ConfigError::NoAttentionRules);
        }

        let attention = raw
            .attention
            .into_iter()
            .enumerate()
            .map(|(index, rule)| AttentionRule::try_from_raw(index, rule))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { attention })
    }
}

/// A validated attention rule.
#[derive(Clone, Debug)]
pub struct AttentionRule {
    app_id: Option<Regex>,
    title: Option<Regex>,
    policy: AttentionPolicy,
}

impl AttentionRule {
    /// Returns the width behavior attached to this rule.
    #[must_use]
    pub const fn policy(&self) -> AttentionPolicy {
        self.policy
    }

    /// Returns whether this rule matches the supplied window identity.
    #[must_use]
    pub fn matches(&self, window: WindowIdentity<'_>) -> bool {
        matches_optional_pattern(self.app_id.as_ref(), window.app_id)
            && matches_optional_pattern(self.title.as_ref(), window.title)
    }

    fn try_from_raw(index: usize, raw: RawAttentionRule) -> Result<Self, ConfigError> {
        if raw.app_id.is_none() && raw.title.is_none() {
            return Err(rule_error(
                index,
                "at least one of `app_id` or `title` is required",
            ));
        }

        let app_id = compile_pattern(index, "app_id", raw.app_id)?;
        let title = compile_pattern(index, "title", raw.title)?;
        let focused = parse_width(index, "focused_width", raw.focused_width)?;
        let unfocused = parse_width(index, "unfocused_width", raw.unfocused_width)?;

        Ok(Self {
            app_id,
            title,
            policy: AttentionPolicy { focused, unfocused },
        })
    }
}

/// Window metadata available to rule matching.
#[derive(Clone, Copy, Debug)]
pub struct WindowIdentity<'a> {
    pub app_id: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// The first matching rule and its position in the configuration.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedRule<'a> {
    pub index: usize,
    pub rule: &'a AttentionRule,
}

/// Returns Melibea's default XDG configuration path.
///
/// # Errors
///
/// Returns an error when neither `XDG_CONFIG_HOME` nor `HOME` provides a
/// usable base directory.
pub fn default_config_path() -> Result<PathBuf, ConfigPathError> {
    if let Some(base) = non_empty_env("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(base)
            .join(CONFIG_DIRECTORY)
            .join(CONFIG_FILENAME));
    }

    if let Some(home) = non_empty_env("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join(CONFIG_DIRECTORY)
            .join(CONFIG_FILENAME));
    }

    Err(ConfigPathError)
}

fn non_empty_env(name: &str) -> Option<std::ffi::OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn matches_optional_pattern(pattern: Option<&Regex>, value: Option<&str>) -> bool {
    pattern.is_none_or(|regex| value.is_some_and(|candidate| regex.is_match(candidate)))
}

fn compile_pattern(
    index: usize,
    field: &'static str,
    pattern: Option<String>,
) -> Result<Option<Regex>, ConfigError> {
    pattern
        .map(|value| {
            Regex::new(&value)
                .map_err(|error| rule_error(index, format!("invalid `{field}` regex: {error}")))
        })
        .transpose()
}

fn parse_width(
    index: usize,
    field: &'static str,
    value: RawWidth,
) -> Result<WidthPolicy, ConfigError> {
    match value {
        RawWidth::Text(value) => value
            .parse::<WidthPolicy>()
            .map_err(|error| width_error(index, field, &error)),
        RawWidth::Number(value) => WidthPolicy::proportion(value)
            .map_err(WidthParseError::InvalidProportion)
            .map_err(|error| width_error(index, field, &error)),
    }
}

fn width_error(index: usize, field: &'static str, error: &WidthParseError) -> ConfigError {
    rule_error(index, format!("invalid `{field}`: {error}"))
}

fn rule_error(index: usize, message: impl Into<String>) -> ConfigError {
    ConfigError::Rule {
        number: index + 1,
        message: message.into(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    attention: Vec<RawAttentionRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAttentionRule {
    app_id: Option<String>,
    title: Option<String>,
    focused_width: RawWidth,
    unfocused_width: RawWidth,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawWidth {
    Text(String),
    Number(f64),
}

/// A configuration loading or validation error.
#[derive(Debug)]
pub enum ConfigError {
    /// The configuration file could not be read.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A path adds context to a parsing or validation error.
    AtPath { path: PathBuf, source: Box<Self> },
    /// TOML syntax or shape was invalid.
    Toml(toml::de::Error),
    /// No attention rules were supplied.
    NoAttentionRules,
    /// One attention rule was invalid.
    Rule { number: usize, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "could not read `{}`: {source}", path.display())
            }
            Self::AtPath { path, source } => {
                write!(
                    formatter,
                    "invalid configuration `{}`: {source}",
                    path.display()
                )
            }
            Self::Toml(error) => write!(formatter, "invalid TOML: {error}"),
            Self::NoAttentionRules => {
                formatter.write_str("at least one `[[attention]]` rule is required")
            }
            Self::Rule { number, message } => {
                write!(formatter, "attention rule {number}: {message}")
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::AtPath { source, .. } => Some(source),
            Self::Toml(error) => Some(error),
            Self::NoAttentionRules | Self::Rule { .. } => None,
        }
    }
}

/// The default configuration directory cannot be discovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigPathError;

impl fmt::Display for ConfigPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("cannot locate configuration: both `XDG_CONFIG_HOME` and `HOME` are unset")
    }
}

impl Error for ConfigPathError {}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError, WindowIdentity};
    use crate::attention::WidthPolicy;

    fn width(value: f64) -> WidthPolicy {
        WidthPolicy::proportion(value).expect("valid test width")
    }

    const VALID: &str = r#"
        [[attention]]
        app_id = "^kitty$"
        focused_width = "50%"
        unfocused_width = "10%"

        [[attention]]
        app_id = "^code$"
        title = "Melibea"
        focused_width = 0.9
        unfocused_width = "preserve"
    "#;

    #[test]
    fn parses_typed_rules() {
        let config = Config::parse(VALID).expect("valid configuration");

        assert_eq!(config.attention_rules().len(), 2);
        assert_eq!(config.attention_rules()[0].policy().focused, width(0.5));
        assert_eq!(
            config.attention_rules()[1].policy().unfocused,
            WidthPolicy::Preserve
        );
    }

    #[test]
    fn resolves_only_the_first_matching_rule() {
        let config = Config::parse(
            r#"
                [[attention]]
                app_id = "kitty"
                focused_width = "50%"
                unfocused_width = "10%"

                [[attention]]
                app_id = ".*"
                focused_width = "75%"
                unfocused_width = "25%"
            "#,
        )
        .expect("valid configuration");

        let resolved = config
            .resolve(WindowIdentity {
                app_id: Some("kitty"),
                title: Some("shell"),
            })
            .expect("matching rule");

        assert_eq!(resolved.index, 0);
        assert_eq!(resolved.rule.policy().focused, width(0.5));
    }

    #[test]
    fn requires_all_configured_matchers() {
        let config = Config::parse(VALID).expect("valid configuration");

        assert!(
            config
                .resolve(WindowIdentity {
                    app_id: Some("code"),
                    title: Some("Melibea - config.rs"),
                })
                .is_some()
        );
        assert!(
            config
                .resolve(WindowIdentity {
                    app_id: Some("code"),
                    title: Some("Another project"),
                })
                .is_none()
        );
    }

    #[test]
    fn rejects_empty_rules() {
        let error = Config::parse("").expect_err("empty configuration must fail");
        assert!(matches!(error, ConfigError::NoAttentionRules));
    }

    #[test]
    fn rejects_rule_without_matcher() {
        let error = Config::parse(
            r#"
                [[attention]]
                focused_width = "50%"
                unfocused_width = "10%"
            "#,
        )
        .expect_err("matcher is required");

        assert!(
            error
                .to_string()
                .contains("at least one of `app_id` or `title`")
        );
    }

    #[test]
    fn reports_invalid_regex_with_rule_context() {
        let error = Config::parse(
            r#"
                [[attention]]
                app_id = "("
                focused_width = "50%"
                unfocused_width = "10%"
            "#,
        )
        .expect_err("invalid regex must fail");

        let message = error.to_string();
        assert!(message.contains("attention rule 1"));
        assert!(message.contains("invalid `app_id` regex"));
    }

    #[test]
    fn reports_invalid_width_with_field_context() {
        let error = Config::parse(
            r#"
                [[attention]]
                app_id = "kitty"
                focused_width = "125%"
                unfocused_width = "10%"
            "#,
        )
        .expect_err("invalid width must fail");

        let message = error.to_string();
        assert!(message.contains("attention rule 1"));
        assert!(message.contains("focused_width"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = Config::parse(
            r#"
                [[attention]]
                app_id = "kitty"
                focused_width = "50%"
                unfocused_width = "10%"
                magic = true
            "#,
        )
        .expect_err("unknown field must fail");

        assert!(error.to_string().contains("unknown field `magic`"));
    }
}
