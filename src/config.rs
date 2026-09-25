// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `~/.cigscript/config`: a few `key = value` lines. No dependency, no
//! surprises, comments with `#`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

/// Every key `cig config` accepts, with its default and meaning.
pub const KEYS: &[(&str, &str, &str)] = &[
    (
        "crash_reports",
        "ask",
        "what to do with a crash report: ask, always (file without asking), never (keep local only)",
    ),
    (
        "issues_repo",
        "otmof-ops/CigScript",
        "GitHub repository that receives crash reports (owner/name)",
    ),
    (
        "update_channel",
        "stable",
        "which releases `cig update` considers: stable, or prerelease",
    ),
    (
        "update_notice",
        "true",
        "mention a newer release on stderr after `cig doctor` or `cig update --check` found one",
    ),
];

#[derive(Clone, Debug, Default)]
pub struct Config {
    values: BTreeMap<String, String>,
}

pub fn path() -> PathBuf {
    crate::burn::runs::home_dir().join("config")
}

impl Config {
    /// Load the config file, tolerating its absence.
    pub fn load() -> Config {
        match fs::read_to_string(path()) {
            Ok(text) => Config::parse(&text),
            Err(_) => Config::default(),
        }
    }

    pub fn parse(text: &str) -> Config {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !k.is_empty() {
                    values.insert(k.to_string(), v.to_string());
                }
            }
        }
        Config { values }
    }

    /// The effective value: the file, else the environment variable
    /// `CIG_<KEY>`, else the default.
    pub fn get(&self, key: &str) -> String {
        if let Some(v) = self.values.get(key) {
            return v.clone();
        }
        let env_key = format!("CIG_{}", key.to_ascii_uppercase());
        if let Ok(v) = std::env::var(&env_key) {
            return v;
        }
        KEYS.iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, d, _)| d.to_string())
            .unwrap_or_default()
    }

    pub fn is_set(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let Some((_, _, _)) = KEYS.iter().find(|(k, _, _)| *k == key) else {
            let known: Vec<&str> = KEYS.iter().map(|(k, _, _)| *k).collect();
            return Err(format!(
                "unknown key `{key}`; known keys: {}",
                known.join(", ")
            ));
        };
        match (key, value) {
            ("crash_reports", "ask" | "always" | "never") => {}
            ("crash_reports", other) => {
                return Err(format!(
                    "crash_reports must be ask, always or never, not `{other}`"
                ))
            }
            ("update_channel", "stable" | "prerelease") => {}
            ("update_channel", other) => {
                return Err(format!(
                    "update_channel must be stable or prerelease, not `{other}`"
                ))
            }
            ("update_notice", "true" | "false") => {}
            ("update_notice", other) => {
                return Err(format!(
                    "update_notice must be true or false, not `{other}`"
                ))
            }
            ("issues_repo", v) if v.split('/').count() == 2 && !v.contains(char::is_whitespace) => {
            }
            ("issues_repo", other) => {
                return Err(format!(
                    "issues_repo must look like owner/name, not `{other}`"
                ))
            }
            _ => {}
        }
        self.values.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> bool {
        self.values.remove(key).is_some()
    }

    pub fn save(&self) -> io::Result<()> {
        let p = path();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text =
            String::from("# CigScript configuration. `cig config` explains every key.\n");
        for (k, v) in &self.values {
            text.push_str(&format!("{k} = {v}\n"));
        }
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, p)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, String, bool)> {
        KEYS.iter()
            .map(|(k, _, _)| (*k, self.get(k), self.is_set(k)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates() {
        let c =
            Config::parse("# comment\ncrash_reports = never\n\nissues_repo=\"a/b\"\nbad line\n");
        assert_eq!(c.get("crash_reports"), "never");
        assert_eq!(c.get("issues_repo"), "a/b");
        assert_eq!(c.get("update_channel"), "stable");
        let mut c = Config::default();
        assert!(c.set("crash_reports", "sometimes").is_err());
        assert!(c.set("nope", "x").is_err());
        assert!(c.set("issues_repo", "just-a-name").is_err());
        assert!(c.set("crash_reports", "always").is_ok());
        assert!(c.unset("crash_reports"));
        assert!(!c.unset("crash_reports"));
    }
}
