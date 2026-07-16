use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;

/// Defaults built once from the compiled-in table in `config`.
static DEFAULT_IDS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    crate::config::QUERY_IDS.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect()
});

/// Ids scraped by this process. The 404 self-heal records what it found here so
/// the retry — and every later call in the same command — skips the scrape.
/// Deliberately not persisted: a rotated id is rare, and a scrape is cheap next
/// to a stale id cached on disk. Pin an id in `config.toml` to make it stick.
static SCRAPED: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
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

/// Ordered, deduped ids to try for an operation: a user pin from
/// `config.toml`'s `[query_ids]` first, then the compiled default, then anything
/// this process scraped. X sometimes 404s a freshly-shipped bundle id it hasn't
/// rolled out server-side, so trying the curated id before the scraped one is
/// deliberate.
pub fn candidates(operation: &str) -> Vec<String> {
    let mut out = Vec::new();
    for id in [crate::config::query_id_override(operation), baked(operation), scraped(operation)]
        .into_iter()
        .flatten()
    {
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn scraped(operation: &str) -> Option<String> {
    SCRAPED.lock().ok()?.get(operation).cloned()
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

/// Single best id for an operation, used right after a forced scrape (so the
/// scraped id outranks the defaults): config override, scraped, then baked.
pub async fn resolve(operation: &str) -> String {
    crate::config::query_id_override(operation)
        .or_else(|| scraped(operation))
        .or_else(|| baked(operation))
        .unwrap_or_default()
}

/// Scrape x.com bundles for current ids of `operations`, recording them for the
/// rest of this process. The 404 self-heal: called when a baked id has rotated out.
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
    if let Ok(mut cache) = SCRAPED.lock() {
        cache.extend(found.clone());
    }
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
