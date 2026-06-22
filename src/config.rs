//! Configuration: externalizes deployment-specific settings (ssh host, remote log
//! path, editor) so the source can be published without anyone's specifics baked in.
//!
//! Lookup order when `--config` is not given (first existing file wins):
//!   1. ./config.toml                       (repo-local, gitignored — your settings)
//!   2. $HOME/.config/interactsh-tui/config.toml   (standard per-user location)
//! A missing file is fine — built-in defaults apply. CLI flags override the config.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// ssh host alias to pull the log from (e.g. an entry in ~/.ssh/config).
    pub host: String,
    /// Absolute path to interactsh's interactions.jsonl on that host.
    pub remote_log: String,
    /// Editor for the `e` key; falls back to $EDITOR then `nvim` when unset.
    pub editor: Option<String>,
    /// Auto-refresh interval in seconds. 0 disables it. Default 60.
    pub refresh_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: String::new(),
            remote_log: "/var/log/interactsh/interactions.jsonl".into(),
            editor: None,
            refresh_secs: 60,
        }
    }
}

impl Config {
    /// Load config, returning it plus the path it came from (`None` = defaults only).
    /// An explicit `--config` path that doesn't exist is an error; an absent
    /// auto-discovered file is not.
    pub fn load(explicit: Option<&Path>) -> Result<(Config, Option<PathBuf>)> {
        if let Some(p) = explicit {
            if !p.is_file() {
                bail!("config file not found: {}", p.display());
            }
            return Ok((Self::read(p)?, Some(p.to_path_buf())));
        }
        for p in Self::default_paths() {
            if p.is_file() {
                return Ok((Self::read(&p)?, Some(p)));
            }
        }
        Ok((Config::default(), None))
    }

    fn default_paths() -> Vec<PathBuf> {
        let mut v = vec![PathBuf::from("config.toml")];
        if let Ok(home) = std::env::var("HOME") {
            v.push(PathBuf::from(home).join(".config/interactsh-tui/config.toml"));
        }
        v
    }

    fn read(p: &Path) -> Result<Config> {
        let s = std::fs::read_to_string(p)
            .with_context(|| format!("reading config {}", p.display()))?;
        toml::from_str(&s).with_context(|| format!("parsing config {}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults() {
        // Only `host` set: other fields fall back to their defaults.
        let cfg: Config = toml::from_str(r#"host = "oob""#).unwrap();
        assert_eq!(cfg.host, "oob");
        assert_eq!(cfg.remote_log, "/var/log/interactsh/interactions.jsonl");
        assert!(cfg.editor.is_none());
        assert_eq!(cfg.refresh_secs, 60);
    }

    #[test]
    fn all_fields_parse() {
        let cfg: Config = toml::from_str(
            "host='h'\nremote_log='/x.jsonl'\neditor='code -w'\nrefresh_secs=10",
        )
        .unwrap();
        assert_eq!(cfg.host, "h");
        assert_eq!(cfg.remote_log, "/x.jsonl");
        assert_eq!(cfg.editor.as_deref(), Some("code -w"));
        assert_eq!(cfg.refresh_secs, 10);
    }

    #[test]
    fn refresh_can_be_disabled() {
        let cfg: Config = toml::from_str("refresh_secs = 0").unwrap();
        assert_eq!(cfg.refresh_secs, 0);
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(toml::from_str::<Config>("host='h'\nnope=1").is_err());
    }

    #[test]
    fn empty_config_is_all_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.host.is_empty());
        assert_eq!(cfg.remote_log, "/var/log/interactsh/interactions.jsonl");
    }
}
