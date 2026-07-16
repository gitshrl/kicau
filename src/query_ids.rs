use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Defaults built once from the compiled-in table in `config`.
static DEFAULT_IDS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    crate::config::QUERY_IDS.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect()
});
const TTL_SECS: u64 = 24 * 60 * 60;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";
const DISCOVERY_PAGES: &[&str] = &[
    "https://x.com/?lang=en",
    "https://x.com/explore",
    "https://x.com/notifications",
    "https://x.com/settings/profile",
];
const BUNDLE_RE: &str = r"https://abs\.twimg\.com/responsive-web/client-web(?:-legacy)?/[A-Za-z0-9._-]+\.js";

/// Compiled-in default id for an operation — the offline fallback.
pub fn baked(operation: &str) -> Option<String> {
    DEFAULT_IDS.get(operation).cloned()
}

/// Ordered, deduped ids to try for an operation: the config file
/// (`~/.config/kicau/query-ids.json`, seeded from the compiled defaults and yours
/// to edit) first, then the compiled fallback, then a still-fresh scraped cache
/// entry. X sometimes 404s a freshly-shipped bundle id it hasn't rolled out
/// server-side, so trying the curated id before the scraped one is deliberate.
pub fn candidates(operation: &str) -> Vec<String> {
    seed_config();
    let mut out = Vec::new();
    for id in [config_id(operation), baked(operation), fresh_cache(operation)].into_iter().flatten() {
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn config_path() -> PathBuf {
    crate::config::config_dir().join("query-ids.json")
}

/// Write the compiled defaults to the config file on first run so the ids are
/// visible and editable. Never overwrites an existing file.
fn seed_config() {
    let path = config_path();
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let defaults: std::collections::BTreeMap<&str, &str> = crate::config::QUERY_IDS.iter().copied().collect();
    if let Ok(json) = serde_json::to_string_pretty(&defaults) {
        let _ = std::fs::write(path, json + "\n");
    }
}

fn config_id(operation: &str) -> Option<String> {
    let raw = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str::<HashMap<String, String>>(&raw).ok()?.get(operation).cloned()
}

fn fresh_cache(operation: &str) -> Option<String> {
    let snapshot = read_cache()?;
    (now().saturating_sub(snapshot.fetched_at) <= TTL_SECS)
        .then(|| snapshot.ids.get(operation).cloned())
        .flatten()
}

/// Pull `operationName -> queryId` pairs for the wanted operations out of an
/// x.com client bundle. Handles both `{queryId,operationName}` field orders.
pub fn extract_operations(js: &str, targets: &[&str]) -> HashMap<String, String> {
    let wanted: HashSet<&str> = targets.iter().copied().collect();
    let mut out = HashMap::new();
    // ponytail: the two export-anchored orders cover current x.com bundles; looser
    // scan-between patterns are dead weight until a bundle layout breaks this.
    let patterns: [(&str, usize, usize); 2] = [
        (r#"e\.exports=\{queryId:"([^"]+)",operationName:"([^"]+)""#, 1, 2),
        (r#"e\.exports=\{operationName:"([^"]+)",queryId:"([^"]+)""#, 2, 1),
    ];
    for (pat, id_group, op_group) in patterns {
        let re = regex::Regex::new(pat).unwrap();
        for cap in re.captures_iter(js) {
            let op = cap.get(op_group).unwrap().as_str();
            let id = cap.get(id_group).unwrap().as_str();
            if wanted.contains(op) {
                out.entry(op.to_string()).or_insert_with(|| id.to_string());
            }
        }
    }
    out
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    fetched_at: u64,
    ids: HashMap<String, String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_path() -> PathBuf {
    crate::config::config_dir().join("query-ids-cache.json")
}

fn read_cache() -> Option<Snapshot> {
    let raw = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(snapshot: &Snapshot) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(snapshot) {
        let _ = std::fs::write(path, json);
    }
}

/// Single best id for an operation, used right after a forced scrape (so a fresh
/// cache entry outranks the defaults): config file, then fresh cache, then baked.
pub async fn resolve(operation: &str) -> String {
    seed_config();
    config_id(operation)
        .or_else(|| fresh_cache(operation))
        .or_else(|| baked(operation))
        .unwrap_or_default()
}

/// Scrape x.com bundles for current ids of `operations`, merge into the cache.
/// This is the 404 self-heal: called when a baked/cached id has rotated out.
pub async fn force_refresh(http: &reqwest::Client, operations: &[&str]) -> Result<HashMap<String, String>> {
    let bundles = discover_bundles(http).await?;
    let mut found = HashMap::new();
    for url in bundles {
        if found.len() == operations.len() {
            break;
        }
        let Ok(js) = fetch_text(http, &url).await else { continue };
        for (op, id) in extract_operations(&js, operations) {
            found.entry(op).or_insert(id);
        }
    }
    if found.is_empty() {
        anyhow::bail!("no query ids discovered; x.com bundle layout may have changed");
    }
    let mut ids = read_cache().map(|s| s.ids).unwrap_or_default();
    ids.extend(found.clone());
    write_cache(&Snapshot { fetched_at: now(), ids });
    Ok(found)
}

async fn discover_bundles(http: &reqwest::Client) -> Result<Vec<String>> {
    let re = regex::Regex::new(BUNDLE_RE).unwrap();
    let mut seen = HashSet::new();
    let mut bundles = Vec::new();
    for page in DISCOVERY_PAGES {
        let Ok(html) = fetch_text(http, page).await else { continue };
        for m in re.find_iter(&html) {
            if seen.insert(m.as_str().to_string()) {
                bundles.push(m.as_str().to_string());
            }
        }
        if !bundles.is_empty() {
            break; // one page's bundles already contain the ids
        }
    }
    if bundles.is_empty() {
        anyhow::bail!("no client bundles discovered");
    }
    Ok(bundles)
}

async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<String> {
    let resp = http
        .get(url)
        .header("user-agent", UA)
        .header("accept-language", "en-US,en;q=0.9")
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.text().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_both_field_orders_for_targets_only() {
        let js = r#"junk;e.exports={queryId:"AAA",operationName:"SearchTimeline"};more;
                    e.exports={operationName:"CreateTweet",queryId:"BBB"};tail;
                    e.exports={queryId:"CCC",operationName:"IgnoreMe"};"#;
        let ids = extract_operations(js, &["SearchTimeline", "CreateTweet", "TweetDetail"]);
        assert_eq!(ids.get("SearchTimeline").map(String::as_str), Some("AAA"));
        assert_eq!(ids.get("CreateTweet").map(String::as_str), Some("BBB")); // reversed order
        assert!(!ids.contains_key("TweetDetail")); // absent from bundle
        assert!(!ids.contains_key("IgnoreMe")); // not a target
    }

    #[test]
    fn baked_has_the_mvp_operations() {
        assert!(baked("SearchTimeline").is_some());
        assert!(baked("TweetDetail").is_some());
        assert!(baked("CreateTweet").is_some());
        assert!(baked("Nonexistent").is_none());
    }
}
