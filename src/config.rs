//! Filesystem layout and credential resolution.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

/// `~/.kicau` — cookies and the SQLite store.
pub fn state_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".kicau")
}

/// `~/.config/kicau` — query-id config/cache and the credentials config file.
pub fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/kicau")
}

pub struct Credentials {
    pub auth_token: String,
    pub ct0: String,
    pub source: String,
}

/// Precedence: CLI flags → env vars → cookie file → config file.
pub fn resolve_credentials(flag_auth: Option<String>, flag_ct0: Option<String>) -> Result<Credentials> {
    if let (Some(auth_token), Some(ct0)) = (nonempty(flag_auth), nonempty(flag_ct0)) {
        return Ok(Credentials { auth_token, ct0, source: "CLI flags".into() });
    }

    if let (Some(auth_token), Some(ct0)) =
        (first_env(&["KICAU_AUTH_TOKEN", "AUTH_TOKEN"]), first_env(&["KICAU_CT0", "CT0"]))
    {
        return Ok(Credentials { auth_token, ct0, source: "environment variables".into() });
    }

    let cookie_file = state_dir().join("cookies.env");
    if let Some(creds) = from_env_file(&cookie_file) {
        return Ok(creds);
    }

    let config_json = config_dir().join("config.json");
    if let Some(creds) = from_config_json(&config_json) {
        return Ok(creds);
    }

    Err(anyhow!(
        "missing credentials — provide --auth-token/--ct0, AUTH_TOKEN/CT0 env vars, {}, or {}",
        cookie_file.display(),
        config_json.display()
    ))
}

fn from_env_file(path: &Path) -> Option<Credentials> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut vars = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, val)) = line.split_once('=') else { continue };
        let val = val.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        if let Some(val) = nonempty(Some(val)) {
            vars.insert(key.trim().to_string(), val);
        }
    }
    // resolve by key priority, not line order
    let pick = |keys: &[&str]| keys.iter().find_map(|k| vars.get(*k).cloned());
    Some(Credentials {
        auth_token: pick(&["KICAU_AUTH_TOKEN", "AUTH_TOKEN"])?,
        ct0: pick(&["KICAU_CT0", "CT0"])?,
        source: path.display().to_string(),
    })
}

fn from_config_json(path: &Path) -> Option<Credentials> {
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let auth_token = nonempty(v.get("authToken").and_then(|x| x.as_str()).map(str::to_string))?;
    let ct0 = nonempty(v.get("ct0").and_then(|x| x.as_str()).map(str::to_string))?;
    Some(Credentials { auth_token, ct0, source: path.display().to_string() })
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| nonempty(std::env::var(k).ok()))
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_cookie_env_file_by_key_priority() {
        let f = tempfile("parse");
        writeln!(&f.0, "AUTH_TOKEN=abc123\nKICAU_CT0=kk\nCT0=\"def456\"\n# comment").unwrap();
        let creds = from_env_file(Path::new(&f.1)).expect("should parse");
        assert_eq!(creds.auth_token, "abc123");
        assert_eq!(creds.ct0, "kk"); // KICAU_CT0 wins over CT0; quotes would strip
    }

    #[test]
    fn missing_key_yields_none() {
        let f = tempfile("missing");
        writeln!(&f.0, "AUTH_TOKEN=only").unwrap();
        assert!(from_env_file(Path::new(&f.1)).is_none());
    }

    fn tempfile(tag: &str) -> (std::fs::File, String) {
        let path = format!("{}/kicau-test-{}-{}.env", std::env::temp_dir().display(), std::process::id(), tag);
        (std::fs::File::create(&path).unwrap(), path)
    }
}
