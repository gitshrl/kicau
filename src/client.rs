use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue};
use serde_json::Value;

use crate::models::{Profile, Tweet};
use crate::{parse, query_ids};

/// Request shape per GraphQL operation. X routes these differently: reads take a
/// GET, search a POST with variables still in the query string, writes a plain POST.
#[derive(Clone, Copy)]
pub enum Call {
    Read,
    Search,
    Write,
}

#[derive(Debug, thiserror::Error)]
enum GqlError {
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("{0}")]
    Api(String),
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Header(#[from] InvalidHeaderValue),
}

const TWITTER_API_BASE: &str = "https://x.com/i/api/graphql";
const BEARER: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub struct TwitterClient {
    auth_token: String,
    ct0: String,
    user_agent: String,
    http: reqwest::Client,
}

pub struct CurrentUser {
    pub id: String,
    pub username: String,
    pub name: String,
}

impl TwitterClient {
    pub fn new(auth_token: String, ct0: String, timeout: Duration) -> Result<Self> {
        if auth_token.is_empty() || ct0.is_empty() {
            return Err(anyhow!("both auth_token and ct0 are required"));
        }
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            auth_token,
            ct0,
            user_agent: DEFAULT_USER_AGENT.to_string(),
            http,
        })
    }

    fn headers(&self) -> std::result::Result<HeaderMap, InvalidHeaderValue> {
        let mut headers = self.base_headers()?;
        headers.insert(HeaderName::from_static("content-type"), HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    /// Headers without content-type, for form-encoded REST calls where reqwest's
    /// `.form()` sets `application/x-www-form-urlencoded` itself.
    fn form_headers(&self) -> std::result::Result<HeaderMap, InvalidHeaderValue> {
        self.base_headers()
    }

    fn base_headers(&self) -> std::result::Result<HeaderMap, InvalidHeaderValue> {
        let pairs = [
            ("authorization", BEARER.to_string()),
            ("x-csrf-token", self.ct0.clone()),
            ("x-twitter-auth-type", "OAuth2Session".to_string()),
            ("x-twitter-active-user", "yes".to_string()),
            ("x-twitter-client-language", "en".to_string()),
            ("cookie", format!("auth_token={}; ct0={}", self.auth_token, self.ct0)),
            ("user-agent", self.user_agent.clone()),
            ("origin", "https://x.com".to_string()),
            ("referer", "https://x.com/".to_string()),
        ];
        let mut headers = HeaderMap::new();
        for (k, v) in pairs {
            headers.insert(HeaderName::from_static(k), HeaderValue::from_str(&v)?);
        }
        Ok(headers)
    }

    /// GraphQL fetch. Resolves the query id by operation name, trying the cached
    /// then baked candidates (X sometimes 404s a freshly-shipped bundle id while
    /// honoring an older one), and self-heals by scraping fresh ids if all 404.
    pub async fn fetch_graphql(
        &self,
        operation: &str,
        variables: Value,
        features: Value,
        call: Call,
    ) -> Result<Value> {
        // Prefer the curated baked id (known-good) and fall back to a freshly
        // scraped cache entry — X sometimes 404s a new bundle id it hasn't rolled
        // out server-side while still honoring the older baked one.
        let mut candidates = Vec::new();
        if let Some(baked) = query_ids::baked(operation) {
            candidates.push(baked);
        }
        let cached = query_ids::resolve(operation).await;
        if !candidates.contains(&cached) {
            candidates.push(cached);
        }

        for query_id in &candidates {
            match self.graphql_call_resilient(query_id, operation, &variables, &features, call).await {
                Err(GqlError::Http { status: 404, .. }) => continue,
                Err(e) => return Err(friendly(e)),
                Ok(value) => return Ok(value),
            }
        }

        // Every known id rotated out — scrape current ids and retry once.
        query_ids::force_refresh(&self.http, &[operation]).await?;
        let query_id = query_ids::resolve(operation).await;
        self.graphql_call_resilient(&query_id, operation, &variables, &features, call)
            .await
            .map_err(friendly)
    }

    /// graphql_call, retried once on a transient server-side error (e.g. X's
    /// DeadlineExceeded). Reads only — a retried write could double-post.
    async fn graphql_call_resilient(
        &self,
        query_id: &str,
        operation: &str,
        variables: &Value,
        features: &Value,
        call: Call,
    ) -> std::result::Result<Value, GqlError> {
        match self.graphql_call(query_id, operation, variables, features, call).await {
            Err(GqlError::Api(msg))
                if !matches!(call, Call::Write) && is_transient(&msg) =>
            {
                self.graphql_call(query_id, operation, variables, features, call).await
            }
            other => other,
        }
    }

    async fn graphql_call(
        &self,
        query_id: &str,
        operation: &str,
        variables: &Value,
        features: &Value,
        call: Call,
    ) -> std::result::Result<Value, GqlError> {
        let url = format!("{TWITTER_API_BASE}/{query_id}/{operation}");
        let req = match call {
            // Read: GET with variables + features in the query string.
            Call::Read => self.http.get(&url).query(&[
                ("variables", variables.to_string()),
                ("features", features.to_string()),
            ]),
            // Search: POST with variables in the query string, features + queryId in the body.
            Call::Search => self
                .http
                .post(&url)
                .query(&[("variables", variables.to_string())])
                .json(&serde_json::json!({ "features": features, "queryId": query_id })),
            // Write: POST with everything in the body. Action mutations (like,
            // retweet, bookmark) send no features — omit the key when null.
            Call::Write => {
                let mut body = serde_json::json!({ "variables": variables, "queryId": query_id });
                if !features.is_null() {
                    body["features"] = features.clone();
                }
                self.http.post(&url).json(&body)
            }
        };

        let resp = req.headers(self.headers()?).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(GqlError::Http {
                status: status.as_u16(),
                body: truncate(&text, 200).to_string(),
            });
        }
        let json: Value = serde_json::from_str(&text)?;
        if let Some(errors) = json.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let msg = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(GqlError::Api(msg));
            }
        }
        Ok(json.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Fetch a single tweet by id via TweetDetail.
    pub async fn get_tweet(&self, tweet_id: &str) -> Result<Tweet> {
        let data = self.tweet_detail(tweet_id).await?;
        let instructions =
            &data["threaded_conversation_with_injections_v2"]["instructions"];
        parse::tweets_from_instructions(instructions)
            .into_iter()
            .find(|t| t.id == tweet_id)
            .ok_or_else(|| anyhow!("tweet not found in response"))
    }

    /// Search latest tweets matching a query via SearchTimeline.
    pub async fn search(&self, query: &str, count: u32) -> Result<Vec<Tweet>> {
        let variables = serde_json::json!({
            "rawQuery": query,
            "count": count,
            "querySource": "typed_query",
            "product": "Latest",
        });
        let data = self
            .fetch_graphql("SearchTimeline", variables, read_features(), Call::Search)
            .await?;
        let instructions =
            &data["search_by_raw_query"]["search_timeline"]["timeline"]["instructions"];
        Ok(parse::tweets_from_instructions(instructions))
    }

    /// A profile by handle via UserByScreenName.
    pub async fn user(&self, handle: &str) -> Result<Profile> {
        let handle = handle.trim_start_matches('@');
        let variables = serde_json::json!({
            "screen_name": handle,
            "withSafetyModeUserFields": true,
        });
        let data = self.fetch_graphql("UserByScreenName", variables, user_features(), Call::Read).await?;
        parse::parse_user(&data["user"]["result"]).ok_or_else(|| anyhow!("user @{handle} not found"))
    }

    /// A user's own tweets: resolve the handle to a rest_id, then UserTweets.
    pub async fn user_tweets(&self, handle: &str, count: u32) -> Result<Vec<Tweet>> {
        let user = self.user(handle).await?;
        let variables = serde_json::json!({
            "userId": user.id,
            "count": count,
            "includePromotedContent": false,
            "withQuickPromoteEligibilityTweetFields": true,
            "withVoice": true,
        });
        self.timeline("UserTweets", variables, read_features(), "/user/result/timeline/timeline/instructions").await
    }

    /// The account's chronological home timeline.
    pub async fn home(&self, count: u32) -> Result<Vec<Tweet>> {
        let variables = serde_json::json!({
            "count": count,
            "includePromotedContent": true,
            "latestControlAvailable": true,
            "requestContext": "launch",
            "withCommunity": true,
        });
        self.timeline("HomeLatestTimeline", variables, read_features(), "/home/home_timeline_urt/instructions").await
    }

    /// The account's bookmarks.
    pub async fn bookmarks(&self, count: u32) -> Result<Vec<Tweet>> {
        let variables = serde_json::json!({
            "count": count,
            "includePromotedContent": false,
            "withDownvotePerspective": false,
            "withReactionsMetadata": false,
            "withReactionsPerspective": false,
        });
        self.timeline("Bookmarks", variables, bookmarks_features(), "/bookmark_timeline_v2/timeline/instructions").await
    }

    /// A list's latest tweets.
    pub async fn list_tweets(&self, list_id: &str, count: u32) -> Result<Vec<Tweet>> {
        let variables = serde_json::json!({ "listId": list_id, "count": count });
        self.timeline("ListLatestTweetsTimeline", variables, read_features(), "/list/tweets_timeline/timeline/instructions").await
    }

    /// Shared read-timeline path: GraphQL GET, then parse tweets at `pointer`.
    async fn timeline(&self, operation: &str, variables: Value, features: Value, pointer: &str) -> Result<Vec<Tweet>> {
        let data = self.fetch_graphql(operation, variables, features, Call::Read).await?;
        let instructions = data.pointer(pointer).cloned().unwrap_or(Value::Null);
        Ok(parse::tweets_from_instructions(&instructions))
    }

    pub async fn like(&self, tweet_id: &str) -> Result<()> {
        self.action("FavoriteTweet", serde_json::json!({ "tweet_id": tweet_id })).await
    }
    pub async fn unlike(&self, tweet_id: &str) -> Result<()> {
        self.action("UnfavoriteTweet", serde_json::json!({ "tweet_id": tweet_id })).await
    }
    pub async fn retweet(&self, tweet_id: &str) -> Result<()> {
        self.action("CreateRetweet", serde_json::json!({ "tweet_id": tweet_id, "dark_request": false })).await
    }
    pub async fn unretweet(&self, tweet_id: &str) -> Result<()> {
        self.action("DeleteRetweet", serde_json::json!({ "source_tweet_id": tweet_id, "dark_request": false })).await
    }
    pub async fn bookmark(&self, tweet_id: &str) -> Result<()> {
        self.action("CreateBookmark", serde_json::json!({ "tweet_id": tweet_id })).await
    }
    pub async fn unbookmark(&self, tweet_id: &str) -> Result<()> {
        self.action("DeleteBookmark", serde_json::json!({ "tweet_id": tweet_id })).await
    }
    pub async fn follow(&self, handle: &str) -> Result<()> {
        let user = self.user(handle).await?;
        self.friendship("create", &user.id).await
    }
    pub async fn unfollow(&self, handle: &str) -> Result<()> {
        let user = self.user(handle).await?;
        self.friendship("destroy", &user.id).await
    }

    /// GraphQL action mutation: POST with no features, success = no error.
    async fn action(&self, operation: &str, variables: Value) -> Result<()> {
        self.fetch_graphql(operation, variables, Value::Null, Call::Write).await?;
        Ok(())
    }

    /// follow/unfollow via the REST v1.1 friendships endpoint (form-encoded).
    async fn friendship(&self, action: &str, user_id: &str) -> Result<()> {
        let url = format!("https://x.com/i/api/1.1/friendships/{action}.json");
        let resp = self
            .http
            .post(&url)
            .headers(self.form_headers()?)
            .form(&[("user_id", user_id), ("skip_status", "true")])
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!("HTTP {}: {}", status.as_u16(), truncate(&text, 200)));
        }
        Ok(())
    }

    /// Scrape current query ids for the given operations and update the cache.
    pub async fn refresh_query_ids(
        &self,
        operations: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        query_ids::force_refresh(&self.http, operations).await
    }

    /// Replies to a tweet: conversation tweets whose parent is this id.
    pub async fn get_replies(&self, tweet_id: &str) -> Result<Vec<Tweet>> {
        let data = self.tweet_detail(tweet_id).await?;
        let instructions =
            &data["threaded_conversation_with_injections_v2"]["instructions"];
        Ok(parse::tweets_from_instructions(instructions)
            .into_iter()
            .filter(|t| t.in_reply_to_status_id.as_deref() == Some(tweet_id))
            .collect())
    }

    /// Full conversation thread for a tweet, ordered oldest first.
    pub async fn get_thread(&self, tweet_id: &str) -> Result<Vec<Tweet>> {
        let data = self.tweet_detail(tweet_id).await?;
        let instructions =
            &data["threaded_conversation_with_injections_v2"]["instructions"];
        let tweets = parse::tweets_from_instructions(instructions);
        let root = tweets
            .iter()
            .find(|t| t.id == tweet_id)
            .and_then(|t| t.conversation_id.clone())
            .unwrap_or_else(|| tweet_id.to_string());
        let mut thread: Vec<Tweet> = tweets
            .into_iter()
            .filter(|t| t.conversation_id.as_deref() == Some(root.as_str()))
            .collect();
        thread.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(thread)
    }

    /// Post a tweet, returning its new id.
    pub async fn post_tweet(&self, text: &str) -> Result<String> {
        self.create_tweet(create_tweet_variables(text, None)).await
    }

    /// Reply to a tweet, returning the new reply's id.
    pub async fn post_reply(&self, text: &str, reply_to: &str) -> Result<String> {
        self.create_tweet(create_tweet_variables(text, Some(reply_to))).await
    }

    async fn create_tweet(&self, variables: Value) -> Result<String> {
        let data = self
            .fetch_graphql("CreateTweet", variables, write_features(), Call::Write)
            .await?;
        data["create_tweet"]["tweet_results"]["result"]["rest_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("tweet created but no id returned"))
    }

    async fn tweet_detail(&self, tweet_id: &str) -> Result<Value> {
        let variables = serde_json::json!({
            "focalTweetId": tweet_id,
            "with_rux_injections": false,
            "rankingMode": "Relevance",
            "includePromotedContent": true,
            "withCommunity": true,
            "withQuickPromoteEligibilityTweetFields": true,
            "withBirdwatchNotes": true,
            "withVoice": true,
        });
        self.fetch_graphql("TweetDetail", variables, read_features(), Call::Read).await
    }

    /// Account behind the current cookies. The authenticated settings page is the
    /// path that resolves against current X; the legacy REST endpoints 404 today,
    /// so they only run as a fallback.
    pub async fn current_user(&self) -> Result<CurrentUser> {
        let mut last_error = match self.current_user_from_settings_page().await {
            Ok(Some(user)) => return Ok(user),
            Ok(None) => "could not parse settings page for user info".to_string(),
            Err(e) => e.to_string(),
        };

        let candidates = [
            "https://x.com/i/api/account/settings.json",
            "https://api.twitter.com/1.1/account/settings.json",
            "https://x.com/i/api/account/verify_credentials.json?skip_status=true&include_entities=false",
            "https://api.twitter.com/1.1/account/verify_credentials.json?skip_status=true&include_entities=false",
        ];
        for url in candidates {
            match self.try_current_user(url).await {
                Ok(Some(user)) => return Ok(user),
                Ok(None) => last_error = "could not determine current user from response".into(),
                Err(e) => last_error = e.to_string(),
            }
        }
        // Match the graphql path's phrasing when every endpoint rejects the cookies.
        if last_error.contains("HTTP 401") || last_error.contains("HTTP 403") {
            return Err(anyhow!("unauthorized — auth_token/ct0 invalid or expired"));
        }
        Err(anyhow!(last_error))
    }

    async fn current_user_from_settings_page(&self) -> Result<Option<CurrentUser>> {
        let cookie = format!("auth_token={}; ct0={}", self.auth_token, self.ct0);
        for page in ["https://x.com/settings/account", "https://twitter.com/settings/account"] {
            let resp = self
                .http
                .get(page)
                .header("cookie", &cookie)
                .header("user-agent", &self.user_agent)
                .send()
                .await?;
            if !resp.status().is_success() {
                continue;
            }
            let html = resp.text().await?;
            let username = capture(r#""screen_name":"([^"]+)""#, &html);
            let user_id = capture(r#""user_id"\s*:\s*"(\d+)""#, &html);
            if let (Some(username), Some(id)) = (username, user_id) {
                let name = capture(r#""name":"([^"\\]*(?:\\.[^"\\]*)*)""#, &html)
                    .map(|n| n.replace("\\\"", "\""))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| username.clone());
                return Ok(Some(CurrentUser { id, username, name }));
            }
        }
        Ok(None)
    }

    async fn try_current_user(&self, url: &str) -> Result<Option<CurrentUser>> {
        let resp = self.http.get(url).headers(self.headers()?).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!("HTTP {}: {}", status.as_u16(), truncate(&text, 200)));
        }
        let d: Value = serde_json::from_str(&text)?;

        let username = str_at(&d, "screen_name").or_else(|| str_at(&d["user"], "screen_name"));
        let user_id = str_at(&d, "user_id")
            .or_else(|| str_at(&d, "user_id_str"))
            .or_else(|| str_at(&d["user"], "id_str"))
            .or_else(|| str_at(&d["user"], "id"));

        match (username, user_id) {
            (Some(username), Some(id)) => {
                let name = str_at(&d, "name")
                    .or_else(|| str_at(&d["user"], "name"))
                    .unwrap_or_else(|| username.clone());
                Ok(Some(CurrentUser { id, username, name }))
            }
            _ => Ok(None),
        }
    }
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn capture(pattern: &str, haystack: &str) -> Option<String> {
    regex::Regex::new(pattern)
        .ok()?
        .captures(haystack)?
        .get(1)
        .map(|m| m.as_str().to_string())
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// A GraphQL error X resolves on a retry: backend fetcher timeouts returned
/// alongside (or instead of) data. Distinct from auth/validation errors, which
/// a retry won't fix.
fn is_transient(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("deadlineexceeded") || m.contains("timeout") || m.contains("timedout")
}

/// Turn transport-level GraphQL errors into actionable messages for the common
/// auth/rate-limit failures; everything else passes through.
fn friendly(err: GqlError) -> anyhow::Error {
    if let GqlError::Http { status, .. } = &err {
        match status {
            401 | 403 => return anyhow!("unauthorized — auth_token/ct0 invalid or expired ({status})"),
            429 => return anyhow!("rate limited by X — wait and retry (429)"),
            _ => {}
        }
    }
    err.into()
}

/// CreateTweet variables for a tweet, or a reply when `reply_to` is set.
pub fn create_tweet_variables(text: &str, reply_to: Option<&str>) -> Value {
    let mut variables = serde_json::json!({
        "tweet_text": text,
        "dark_request": false,
        "media": { "media_entities": [], "possibly_sensitive": false },
        "semantic_annotation_ids": [],
    });
    if let Some(id) = reply_to {
        variables["reply"] = serde_json::json!({
            "in_reply_to_tweet_id": id,
            "exclude_reply_user_ids": [],
        });
    }
    variables
}

/// Feature flags for the Bookmarks timeline (requires the bookmark timeline switch).
fn bookmarks_features() -> Value {
    serde_json::json!({
        "graphql_timeline_v2_bookmark_timeline": true,
        "rweb_video_screen_enabled": true,
        "profile_label_improvements_pcf_label_in_post_enabled": true,
        "responsive_web_profile_redirect_enabled": true,
        "rweb_tipjar_consumption_enabled": true,
        "verified_phone_label_enabled": false,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "premium_content_api_read_enabled": false,
        "communities_web_enable_tweet_community_results_fetch": true,
        "c9s_tweet_anatomy_moderator_badge_enabled": true,
        "responsive_web_grok_analyze_button_fetch_trends_enabled": false,
        "responsive_web_grok_analyze_post_followups_enabled": false,
        "responsive_web_grok_annotations_enabled": false,
        "responsive_web_jetfuel_frame": true,
        "post_ctas_fetch_enabled": true,
        "responsive_web_grok_share_attachment_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "view_counts_everywhere_api_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": true,
        "tweet_awards_web_tipping_enabled": false,
        "responsive_web_grok_show_grok_translated_post": false,
        "responsive_web_grok_analysis_button_from_backend": true,
        "creator_subscriptions_quote_tweet_preview_enabled": false,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "standardized_nudges_misinfo": true,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "rweb_video_timestamps_enabled": true,
        "longform_notetweets_rich_text_read_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "responsive_web_grok_image_annotation_enabled": true,
        "responsive_web_grok_imagine_annotation_enabled": true,
        "responsive_web_grok_community_note_auto_translation_is_enabled": false,
        "articles_preview_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
    })
}

/// Feature flags for UserByScreenName (profile lookup carries its own set).
fn user_features() -> Value {
    serde_json::json!({
        "hidden_profile_subscriptions_enabled": true,
        "hidden_profile_likes_enabled": true,
        "rweb_tipjar_consumption_enabled": true,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "verified_phone_label_enabled": false,
        "subscriptions_verification_info_is_identity_verified_enabled": true,
        "subscriptions_verification_info_verified_since_enabled": true,
        "highlights_tweets_tab_ui_enabled": true,
        "responsive_web_twitter_article_notes_tab_enabled": true,
        "subscriptions_feature_can_gift_premium": true,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "responsive_web_graphql_timeline_navigation_enabled": true,
    })
}

/// Feature flags shared by the TweetDetail and SearchTimeline read queries.
fn read_features() -> Value {
    serde_json::json!({
        "rweb_tipjar_consumption_enabled": true,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "verified_phone_label_enabled": false,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "communities_web_enable_tweet_community_results_fetch": true,
        "c9s_tweet_anatomy_moderator_badge_enabled": true,
        "articles_preview_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "view_counts_everywhere_api_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": true,
        "tweet_awards_web_tipping_enabled": false,
        "creator_subscriptions_quote_tweet_preview_enabled": false,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "standardized_nudges_misinfo": true,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "rweb_video_timestamps_enabled": true,
        "longform_notetweets_rich_text_read_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
    })
}

/// Feature flags for CreateTweet (writes carry a larger set than reads).
fn write_features() -> Value {
    serde_json::json!({
        "premium_content_api_read_enabled": false,
        "communities_web_enable_tweet_community_results_fetch": true,
        "c9s_tweet_anatomy_moderator_badge_enabled": true,
        "responsive_web_grok_analyze_button_fetch_trends_enabled": false,
        "responsive_web_grok_analyze_post_followups_enabled": false,
        "responsive_web_jetfuel_frame": true,
        "responsive_web_grok_share_attachment_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "view_counts_everywhere_api_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": true,
        "tweet_awards_web_tipping_enabled": false,
        "responsive_web_grok_show_grok_translated_post": false,
        "responsive_web_grok_analysis_button_from_backend": true,
        "creator_subscriptions_quote_tweet_preview_enabled": false,
        "longform_notetweets_rich_text_read_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "profile_label_improvements_pcf_label_in_post_enabled": true,
        "responsive_web_profile_redirect_enabled": false,
        "rweb_tipjar_consumption_enabled": true,
        "verified_phone_label_enabled": false,
        "articles_preview_enabled": true,
        "responsive_web_grok_community_note_auto_translation_is_enabled": false,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "standardized_nudges_misinfo": true,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "responsive_web_grok_image_annotation_enabled": true,
        "responsive_web_grok_imagine_annotation_enabled": true,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tweet_variables_omit_reply_block() {
        let v = create_tweet_variables("hello world", None);
        assert_eq!(v["tweet_text"], "hello world");
        assert_eq!(v["dark_request"], false);
        assert!(v.get("reply").is_none());
    }

    #[test]
    fn reply_variables_carry_parent_id() {
        let v = create_tweet_variables("nice", Some("123"));
        assert_eq!(v["tweet_text"], "nice");
        assert_eq!(v["reply"]["in_reply_to_tweet_id"], "123");
        assert_eq!(v["reply"]["exclude_reply_user_ids"], serde_json::json!([]));
    }

    #[test]
    fn classifies_transient_vs_fatal_errors() {
        assert!(is_transient("DeadlineExceeded: Unspecified"));
        assert!(is_transient("Dependency: Timedout"));
        assert!(!is_transient("Could not authenticate you."));
        assert!(!is_transient("Bad guest token"));
    }
}
