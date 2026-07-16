//! Filesystem layout, the `~/.kicau/config.toml` file, and credential resolution.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::Deserialize;

/// Compiled-in default GraphQL query ids — the seed for the user config file
/// and the offline fallback. Curated known-good values; X rotates ids, so the
/// runtime layers (config file, scraped cache) can override any of these.
pub const QUERY_IDS: &[(&str, &str)] = &[
    ("TweetDetail", "jd3V43oDY9cY7obs1YMfbQ"),
    ("SearchTimeline", "Bcw3RzK-PatNAmbnw54hFw"),
    ("CreateTweet", "R5EPiGHgSqbTYFyozd-gFw"),
    ("DeleteTweet", "nxpZCY2K-I6QoFHAHeojFQ"),
    ("UserByScreenName", "xc8f1g7BYqr6VTzTbvNlGw"),
    ("UserTweets", "Wms1GvIiHXAPBaCr9KblaA"),
    ("HomeLatestTimeline", "iOEZpOdfekFsxSlPQCQtPg"),
    ("Bookmarks", "RV1g3b8n_SGOHwkqKYSCFw"),
    ("ListLatestTweetsTimeline", "2TemLyqrMpTeAmysdbnVqw"),
    ("ListOwnerships", "wQcOSjSQ8NtgxIwvYl1lMg"),
    ("Following", "mWYeougg_ocJS2Vr1Vt28w"),
    ("Followers", "SFYY3WsgwjlXSLlfnEUE4A"),
    ("Likes", "JR2gceKucIKcVNB_9JkhsA"),
    ("FavoriteTweet", "lI07N6Otwv1PhnEgXILM7A"),
    ("UnfavoriteTweet", "ZYKSe-w7KEslx3JhSIk5LA"),
    ("CreateRetweet", "mbRO74GrOvSfRcJnlMapnQ"),
    ("DeleteRetweet", "iQtK4dl5hBmXewYZuEOKVw"),
    ("CreateFriendship", "8h9JVdV8dlSyqyRDJEPCsA"),
    ("DestroyFriendship", "ppXWuagMNXgvzx6WoXBW0Q"),
    ("CreateBookmark", "aoDbu3RHznuiSkQ9aNM67Q"),
    ("DeleteBookmark", "Wlmlj2-xzyS1GN3a6cj-mQ"),
];

/// `~/.kicau` — the single home for config, cookies, cache, and the SQLite store.
pub fn state_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".kicau")
}

/// `~/.kicau/config.toml` — the unified config file.
pub fn config_toml_path() -> PathBuf {
    state_dir().join("config.toml")
}

#[derive(Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    credentials: CredsSection,
    /// Optional per-operation query-id overrides.
    #[serde(default)]
    query_ids: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
struct CredsSection {
    auth_token: Option<String>,
    ct0: Option<String>,
}

fn load_config() -> FileConfig {
    std::fs::read_to_string(config_toml_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// A user-pinned query id from `config.toml`'s `[query_ids]`, if set.
pub fn query_id_override(operation: &str) -> Option<String> {
    load_config().query_ids.get(operation).cloned().and_then(|v| nonempty(Some(v)))
}

pub struct Credentials {
    pub auth_token: String,
    pub ct0: String,
    pub source: String,
}

/// Precedence: CLI flags → env vars → config.toml `[credentials]`.
pub fn resolve_credentials(flag_auth: Option<String>, flag_ct0: Option<String>) -> Result<Credentials> {
    if let (Some(auth_token), Some(ct0)) = (nonempty(flag_auth), nonempty(flag_ct0)) {
        return Ok(Credentials { auth_token, ct0, source: "CLI flags".into() });
    }

    if let (Some(auth_token), Some(ct0)) =
        (first_env(&["KICAU_AUTH_TOKEN", "AUTH_TOKEN"]), first_env(&["KICAU_CT0", "CT0"]))
    {
        return Ok(Credentials { auth_token, ct0, source: "environment variables".into() });
    }

    let cfg = load_config();
    if let (Some(auth_token), Some(ct0)) =
        (nonempty(cfg.credentials.auth_token), nonempty(cfg.credentials.ct0))
    {
        return Ok(Credentials { auth_token, ct0, source: config_toml_path().display().to_string() });
    }

    Err(anyhow!(
        "missing credentials — set [credentials] in {}, or pass --auth-token/--ct0 or AUTH_TOKEN/CT0",
        config_toml_path().display()
    ))
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

    #[test]
    fn parses_credentials_and_query_ids() {
        let cfg: FileConfig = toml::from_str(
            "[credentials]\nauth_token = \"abc\"\nct0 = \"def\"\n\n[query_ids]\nSearchTimeline = \"PINNED\"\n",
        )
        .unwrap();
        assert_eq!(cfg.credentials.auth_token.as_deref(), Some("abc"));
        assert_eq!(cfg.credentials.ct0.as_deref(), Some("def"));
        assert_eq!(cfg.query_ids.get("SearchTimeline").map(String::as_str), Some("PINNED"));
    }

    #[test]
    fn missing_sections_default_empty() {
        let cfg: FileConfig = toml::from_str("").unwrap();
        assert!(cfg.credentials.auth_token.is_none());
        assert!(cfg.query_ids.is_empty());
    }
}
