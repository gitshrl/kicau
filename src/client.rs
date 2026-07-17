use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Result, anyhow};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue};
use serde_json::Value;

use crate::models::{DmConversation, DmMessage, Profile, Tweet};
use crate::{parse, query};

/// Request shape per GraphQL operation. X routes these differently: reads take a
/// GET, search a POST with variables still in the query string, writes a plain POST.
#[derive(Clone, Copy)]
pub enum Call {
    Read,
    Search,
    Write,
}

/// Operations whose payloads can carry an article. Without the toggle X returns
/// only the article's title and preview blurb; with it, the full body arrives as
/// `plain_text`. X rejects toggles an operation does not declare, so this stays
/// an explicit list rather than a blanket header.
const ARTICLE_OPS: &[&str] = &[
    "TweetDetail",
    "SearchTimeline",
    "UserTweets",
    "HomeLatestTimeline",
    "Bookmarks",
    "ListLatestTweetsTimeline",
    "Likes",
];

fn field_toggles(operation: &str) -> Option<Value> {
    ARTICLE_OPS
        .contains(&operation)
        .then(|| serde_json::json!({ "withArticlePlainText": true }))
}

/// How long to wait between timeline pages. X rate-limits reads, and a long
/// backfill is dozens of pages.
const PAGE_DELAY: Duration = Duration::from_secs(2);

/// Entries X will serve in one page. It silently caps at 100 and rejects a
/// count much beyond that with "Query: Unspecified", so the count on the wire is
/// a page size, never the total wanted.
const PAGE_SIZE: u32 = 100;

/// Whether a GraphQL `data` block actually carries something. Null, or an empty
/// object, means X answered with nothing and any error beside it is the real
/// result.
fn has_content(data: &Value) -> bool {
    match data {
        Value::Null => false,
        Value::Object(map) => !map.is_empty(),
        _ => true,
    }
}

/// The cursor for the next page, or None at the end of a timeline.
///
/// X writes cursor entries two ways: timelines put the value straight on
/// `content`, conversation threads nest it under `content.itemContent`. Both
/// appear in real responses, so both are read.
fn bottom_cursor(instructions: &Value) -> Option<String> {
    let entries = instructions
        .as_array()?
        .iter()
        .filter_map(|instruction| instruction.get("entries").and_then(Value::as_array))
        .flatten();

    for entry in entries {
        let is_bottom = entry
            .get("entryId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("cursor-bottom"));
        if !is_bottom {
            continue;
        }
        if let Some(content) = entry.get("content") {
            let value = content
                .get("value")
                .or_else(|| content.pointer("/itemContent/value"))
                .and_then(Value::as_str);
            if let Some(value) = value {
                return Some(value.to_string());
            }
        }
    }
    None
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
const UPLOAD_URL: &str = "https://upload.twitter.com/i/media/upload.json";
const MEDIA_METADATA_URL: &str = "https://x.com/i/api/1.1/media/metadata/create.json";
const BEARER: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub struct TwitterClient {
    auth_token: String,
    ct0: String,
    user_agent: String,
    http: reqwest::Client,
    txid: tokio::sync::OnceCell<Option<crate::transaction::TxidGenerator>>,
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
            txid: tokio::sync::OnceCell::new(),
        })
    }

    /// The `x-client-transaction-id` for a request, derived once per process from
    /// the live home page. Best-effort: None if the page layout ever changes.
    async fn client_transaction_id(&self, method: &str, path: &str) -> Option<String> {
        let cookie = format!("auth_token={}; ct0={}", self.auth_token, self.ct0);
        let generator = self
            .txid
            .get_or_init(|| async {
                crate::transaction::TxidGenerator::fetch(&self.http, &cookie, &self.user_agent)
                    .await
                    .ok()
            })
            .await;
        generator.as_ref().map(|g| g.generate(method, path))
    }

    fn headers(&self) -> std::result::Result<HeaderMap, InvalidHeaderValue> {
        let mut headers = self.base_headers()?;
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
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
            (
                "cookie",
                format!("auth_token={}; ct0={}", self.auth_token, self.ct0),
            ),
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
        // Ordered candidates (user pin → baked → fresh cache); applies to reads
        // and writes alike.
        for query_id in &query::candidates(operation) {
            match self
                .graphql_call_resilient(query_id, operation, &variables, &features, call)
                .await
            {
                Err(GqlError::Http { status: 404, .. }) => {}
                Err(e) => return Err(friendly(e)),
                Ok(value) => return Ok(value),
            }
        }

        // Every known id rotated out — scrape current ids and retry once.
        query::force_refresh(&self.http, &[operation]).await?;
        let query_id = query::resolve(operation);
        self.graphql_call_resilient(&query_id, operation, &variables, &features, call)
            .await
            .map_err(friendly)
    }

    /// `graphql_call`, retried once on a transient server-side error (e.g. X's
    /// `DeadlineExceeded`). Reads only — a retried write could double-post.
    async fn graphql_call_resilient(
        &self,
        query_id: &str,
        operation: &str,
        variables: &Value,
        features: &Value,
        call: Call,
    ) -> std::result::Result<Value, GqlError> {
        match self
            .graphql_call(query_id, operation, variables, features, call)
            .await
        {
            Err(GqlError::Api(msg)) if !matches!(call, Call::Write) && is_transient(&msg) => {
                self.graphql_call(query_id, operation, variables, features, call)
                    .await
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
        let toggles = field_toggles(operation);
        let req = match call {
            // Read: GET with variables + features in the query string.
            Call::Read => {
                let mut req = self.http.get(&url).query(&[
                    ("variables", variables.to_string()),
                    ("features", features.to_string()),
                ]);
                if let Some(toggles) = &toggles {
                    req = req.query(&[("fieldToggles", toggles.to_string())]);
                }
                req
            }
            // Search: POST with variables in the query string, features + queryId in the body.
            Call::Search => {
                let mut body = serde_json::json!({ "features": features, "queryId": query_id });
                if let Some(toggles) = &toggles {
                    body["fieldToggles"] = toggles.clone();
                }
                self.http
                    .post(&url)
                    .query(&[("variables", variables.to_string())])
                    .json(&body)
            }
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

        let mut req = req.headers(self.headers()?);
        // Content-creating writes (CreateTweet) need X's anti-automation header.
        if matches!(call, Call::Write) {
            let path = format!("/i/api/graphql/{query_id}/{operation}");
            if let Some(txid) = self.client_transaction_id("POST", &path).await {
                req = req.header("x-client-transaction-id", txid);
            }
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(GqlError::Http {
                status: status.as_u16(),
                body: truncate(&text, 200).to_string(),
            });
        }
        let json: Value = serde_json::from_str(&text)?;
        let data = json.get("data").cloned().unwrap_or(Value::Null);
        if let Some(errors) = json.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            let msg = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            // GraphQL allows errors beside real data, and X uses that: a deep
            // bookmarks page answers "Query: Unspecified" while still carrying a
            // full page of tweets and the cursor for the next one. Failing there
            // truncates a 1,671-bookmark timeline at the first hiccup, so for a
            // read the data wins and only a dataless response is an error.
            //
            // Writes get no such benefit of the doubt. `action` discards the
            // body, so a tolerated error would report a like or a delete that
            // never happened — for a write, the error is the result.
            if matches!(call, Call::Write) || !has_content(&data) {
                return Err(GqlError::Api(msg));
            }
        }
        Ok(data)
    }

    /// Fetch a single tweet by id via `TweetDetail`.
    pub async fn get_tweet(&self, tweet_id: &str) -> Result<Tweet> {
        let data = self.tweet_detail(tweet_id).await?;
        let instructions = &data["threaded_conversation_with_injections_v2"]["instructions"];
        parse::tweets_from_instructions(instructions)
            .into_iter()
            .find(|t| t.id == tweet_id)
            .ok_or_else(|| anyhow!("tweet not found in response"))
    }

    /// Search latest tweets matching a query via `SearchTimeline`.
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

    /// Accounts a user follows.
    pub async fn following(&self, handle: &str, count: u32) -> Result<Vec<Profile>> {
        self.user_graph("Following", handle, count).await
    }
    /// Accounts that follow a user.
    pub async fn followers(&self, handle: &str, count: u32) -> Result<Vec<Profile>> {
        self.user_graph("Followers", handle, count).await
    }

    async fn user_graph(&self, operation: &str, handle: &str, count: u32) -> Result<Vec<Profile>> {
        let user = self.user(handle).await?;
        let variables = serde_json::json!({
            "userId": user.id,
            "count": count,
            "includePromotedContent": false,
        });
        let data = self
            .fetch_graphql(operation, variables, read_features(), Call::Read)
            .await?;
        let instructions = data
            .pointer("/user/result/timeline/timeline/instructions")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(parse::users_from_instructions(&instructions))
    }

    /// A profile by handle via `UserByScreenName`.
    pub async fn user(&self, handle: &str) -> Result<Profile> {
        let handle = handle.trim_start_matches('@');
        let variables = serde_json::json!({
            "screen_name": handle,
            "withSafetyModeUserFields": true,
        });
        let data = self
            .fetch_graphql("UserByScreenName", variables, user_features(), Call::Read)
            .await?;
        parse::parse_user(&data["user"]["result"])
            .ok_or_else(|| anyhow!("user @{handle} not found"))
    }

    /// A user's own tweets: resolve the handle to a `rest_id`, then `UserTweets`.
    pub async fn user_tweets(&self, handle: &str, count: u32) -> Result<Vec<Tweet>> {
        let user = self.user(handle).await?;
        let variables = serde_json::json!({
            "userId": user.id,
            "count": count,
            "includePromotedContent": false,
            "withQuickPromoteEligibilityTweetFields": true,
            "withVoice": true,
        });
        self.timeline(
            "UserTweets",
            variables,
            read_features(),
            "/user/result/timeline/timeline/instructions",
            count,
        )
        .await
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
        self.timeline(
            "HomeLatestTimeline",
            variables,
            read_features(),
            "/home/home_timeline_urt/instructions",
            count,
        )
        .await
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
        self.timeline(
            "Bookmarks",
            variables,
            bookmarks_features(),
            "/bookmark_timeline_v2/timeline/instructions",
            count,
        )
        .await
    }

    /// A list's latest tweets.
    pub async fn list_tweets(&self, list_id: &str, count: u32) -> Result<Vec<Tweet>> {
        let variables = serde_json::json!({ "listId": list_id, "count": count });
        self.timeline(
            "ListLatestTweetsTimeline",
            variables,
            read_features(),
            "/list/tweets_timeline/timeline/instructions",
            count,
        )
        .await
    }

    /// Shared read-timeline path: GraphQL GET, parse tweets at `pointer`, then
    /// follow the bottom cursor until `count` tweets are collected or the
    /// timeline ends.
    ///
    /// X refuses to paginate a timeline indefinitely: bookmarks stop at roughly
    /// 400 entries deep with an `OperationalError`, whatever the page size. When
    /// a page fails after others have succeeded, the pages already collected are
    /// returned and the reason is reported on stderr, because throwing away a
    /// completed backfill helps nobody. A failure on the first page is the
    /// caller's problem and propagates.
    async fn timeline(
        &self,
        operation: &str,
        variables: Value,
        features: Value,
        pointer: &str,
        count: u32,
    ) -> Result<Vec<Tweet>> {
        let want = count as usize;
        let mut variables = variables;
        // Callers ask for a total; the wire wants a page size.
        variables["count"] = Value::from(count.min(PAGE_SIZE));
        let mut tweets = Vec::new();
        let mut seen_cursors = HashSet::new();

        loop {
            let data = match self
                .fetch_graphql(operation, variables.clone(), features.clone(), Call::Read)
                .await
            {
                Ok(data) => data,
                Err(e) if tweets.is_empty() => return Err(e),
                Err(e) => {
                    eprintln!("⚠️  {operation} stopped after {} tweets: {e}", tweets.len());
                    break;
                }
            };
            let instructions = data.pointer(pointer).cloned().unwrap_or(Value::Null);
            let page = parse::tweets_from_instructions(&instructions);

            // An empty page means the timeline is exhausted; without this the
            // cursor can keep resolving and the loop never ends.
            if page.is_empty() {
                break;
            }
            tweets.extend(page);
            if tweets.len() >= want {
                break;
            }

            let Some(cursor) = bottom_cursor(&instructions) else {
                break;
            };
            // At the end of a timeline X hands back the cursor it was given.
            if !seen_cursors.insert(cursor.clone()) {
                break;
            }
            variables["cursor"] = Value::String(cursor);
            tokio::time::sleep(PAGE_DELAY).await;
        }

        tweets.truncate(want);
        Ok(tweets)
    }

    /// Delete one of your own tweets.
    pub async fn delete_tweet(&self, tweet_id: &str) -> Result<()> {
        self.action(
            "DeleteTweet",
            serde_json::json!({ "tweet_id": tweet_id, "dark_request": false }),
        )
        .await
    }
    pub async fn like(&self, tweet_id: &str) -> Result<()> {
        self.action("FavoriteTweet", serde_json::json!({ "tweet_id": tweet_id }))
            .await
    }
    pub async fn unlike(&self, tweet_id: &str) -> Result<()> {
        self.action(
            "UnfavoriteTweet",
            serde_json::json!({ "tweet_id": tweet_id }),
        )
        .await
    }
    pub async fn retweet(&self, tweet_id: &str) -> Result<()> {
        self.action(
            "CreateRetweet",
            serde_json::json!({ "tweet_id": tweet_id, "dark_request": false }),
        )
        .await
    }
    pub async fn unretweet(&self, tweet_id: &str) -> Result<()> {
        self.action(
            "DeleteRetweet",
            serde_json::json!({ "source_tweet_id": tweet_id, "dark_request": false }),
        )
        .await
    }
    pub async fn bookmark(&self, tweet_id: &str) -> Result<()> {
        self.action(
            "CreateBookmark",
            serde_json::json!({ "tweet_id": tweet_id }),
        )
        .await
    }
    pub async fn unbookmark(&self, tweet_id: &str) -> Result<()> {
        self.action(
            "DeleteBookmark",
            serde_json::json!({ "tweet_id": tweet_id }),
        )
        .await
    }
    pub async fn follow(&self, handle: &str) -> Result<()> {
        self.rest_user_action("friendships/create", handle).await
    }
    pub async fn unfollow(&self, handle: &str) -> Result<()> {
        self.rest_user_action("friendships/destroy", handle).await
    }
    pub async fn block(&self, handle: &str) -> Result<()> {
        self.rest_user_action("blocks/create", handle).await
    }
    pub async fn unblock(&self, handle: &str) -> Result<()> {
        self.rest_user_action("blocks/destroy", handle).await
    }
    pub async fn mute(&self, handle: &str) -> Result<()> {
        self.rest_user_action("mutes/users/create", handle).await
    }
    pub async fn unmute(&self, handle: &str) -> Result<()> {
        self.rest_user_action("mutes/users/destroy", handle).await
    }

    /// GraphQL action mutation: POST with no features, success = no error.
    async fn action(&self, operation: &str, variables: Value) -> Result<()> {
        self.fetch_graphql(operation, variables, Value::Null, Call::Write)
            .await?;
        Ok(())
    }

    /// REST v1.1 user action (follow/block/mute create+destroy): resolve the
    /// handle to an id, then form-POST to the given endpoint path.
    async fn rest_user_action(&self, path: &str, handle: &str) -> Result<()> {
        let user = self.user(handle).await?;
        let url = format!("https://x.com/i/api/1.1/{path}.json");
        let resp = self
            .http
            .post(&url)
            .headers(self.form_headers()?)
            .form(&[("user_id", user.id.as_str()), ("skip_status", "true")])
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "HTTP {}: {}",
                status.as_u16(),
                truncate(&text, 200)
            ));
        }
        Ok(())
    }

    /// The DM inbox: recent conversations and messages across all threads.
    pub async fn dm_inbox(&self) -> Result<(Vec<DmConversation>, Vec<DmMessage>)> {
        let me = self.current_user().await?;
        let resp = self
            .http
            .get("https://x.com/i/api/1.1/dm/inbox_initial_state.json")
            .query(&[
                ("include_conversation_info", "true"),
                ("nsfw_filtering_enabled", "false"),
            ])
            .headers(self.headers()?)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "HTTP {}: {}",
                status.as_u16(),
                truncate(&text, 200)
            ));
        }
        let data: Value = serde_json::from_str(&text)?;
        Ok(parse::parse_dm_inbox(&data, &me.id))
    }

    /// Chunked media upload (INIT → APPEND → FINALIZE → STATUS), returning the
    /// `media_id`. Attaches alt text for images when provided.
    pub async fn upload_media(&self, path: &std::path::Path, alt: Option<&str>) -> Result<String> {
        let bytes =
            std::fs::read(path).map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
        let mime = mime_for_path(path)?;

        let init = self
            .upload_form(&[
                ("command", "INIT"),
                ("total_bytes", &bytes.len().to_string()),
                ("media_type", &mime),
                ("media_category", media_category(&mime)),
            ])
            .await?;
        let media_id = init["media_id_string"]
            .as_str()
            .ok_or_else(|| anyhow!("upload INIT returned no media_id"))?
            .to_string();

        for (index, chunk) in bytes.chunks(5 * 1024 * 1024).enumerate() {
            let part = reqwest::multipart::Part::bytes(chunk.to_vec())
                .file_name("media")
                .mime_str(&mime)?;
            let form = reqwest::multipart::Form::new()
                .text("command", "APPEND")
                .text("media_id", media_id.clone())
                .text("segment_index", index.to_string())
                .part("media", part);
            let resp = self
                .http
                .post(UPLOAD_URL)
                .headers(self.form_headers()?)
                .multipart(form)
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "upload APPEND HTTP {status}: {}",
                    truncate(&body, 200)
                ));
            }
        }

        let fin = self
            .upload_form(&[("command", "FINALIZE"), ("media_id", &media_id)])
            .await?;
        let processing = fin
            .pointer("/processing_info/state")
            .and_then(Value::as_str);
        if matches!(processing, Some(s) if s != "succeeded") {
            self.await_media_processing(&media_id).await?;
        }

        if let Some(text) = alt
            && mime.starts_with("image/")
            && !text.is_empty()
        {
            self.set_media_alt(&media_id, text).await?;
        }
        Ok(media_id)
    }

    async fn upload_form(&self, params: &[(&str, &str)]) -> Result<Value> {
        let resp = self
            .http
            .post(UPLOAD_URL)
            .headers(self.form_headers()?)
            .form(params)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "upload HTTP {}: {}",
                status.as_u16(),
                truncate(&text, 200)
            ));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn await_media_processing(&self, media_id: &str) -> Result<()> {
        for _ in 0..20 {
            let resp = self
                .http
                .get(UPLOAD_URL)
                .query(&[("command", "STATUS"), ("media_id", media_id)])
                .headers(self.form_headers()?)
                .send()
                .await?;
            let v: Value = serde_json::from_str(&resp.text().await?).unwrap_or(Value::Null);
            match v.pointer("/processing_info/state").and_then(Value::as_str) {
                Some("succeeded") | None => return Ok(()),
                Some("failed") => return Err(anyhow!("media processing failed")),
                _ => {
                    let secs = v
                        .pointer("/processing_info/check_after_secs")
                        .and_then(Value::as_u64)
                        .unwrap_or(2)
                        .max(1);
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                }
            }
        }
        Err(anyhow!("media processing timed out"))
    }

    async fn set_media_alt(&self, media_id: &str, text: &str) -> Result<()> {
        let body = serde_json::json!({ "media_id": media_id, "alt_text": { "text": text } });
        let resp = self
            .http
            .post(MEDIA_METADATA_URL)
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let b = resp.text().await.unwrap_or_default();
            return Err(anyhow!("alt-text HTTP {status}: {}", truncate(&b, 200)));
        }
        Ok(())
    }

    /// Scrape current query ids for the given operations and update the cache.
    pub async fn refresh_query_ids(
        &self,
        operations: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        query::force_refresh(&self.http, operations).await
    }

    /// Replies to a tweet: conversation tweets whose parent is this id.
    pub async fn get_replies(&self, tweet_id: &str) -> Result<Vec<Tweet>> {
        let data = self.tweet_detail(tweet_id).await?;
        let instructions = &data["threaded_conversation_with_injections_v2"]["instructions"];
        Ok(parse::tweets_from_instructions(instructions)
            .into_iter()
            .filter(|t| t.in_reply_to_status_id.as_deref() == Some(tweet_id))
            .collect())
    }

    /// Full conversation thread for a tweet, ordered oldest first.
    pub async fn get_thread(&self, tweet_id: &str) -> Result<Vec<Tweet>> {
        let data = self.tweet_detail(tweet_id).await?;
        let instructions = &data["threaded_conversation_with_injections_v2"]["instructions"];
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
    pub async fn post_tweet(&self, text: &str, media_ids: &[String]) -> Result<String> {
        self.create_tweet(create_tweet_variables(text, None, media_ids))
            .await
    }

    /// Reply to a tweet, returning the new reply's id.
    pub async fn post_reply(
        &self,
        text: &str,
        reply_to: &str,
        media_ids: &[String],
    ) -> Result<String> {
        self.create_tweet(create_tweet_variables(text, Some(reply_to), media_ids))
            .await
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
        self.fetch_graphql("TweetDetail", variables, read_features(), Call::Read)
            .await
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
        for page in [
            "https://x.com/settings/account",
            "https://twitter.com/settings/account",
        ] {
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
            return Err(anyhow!(
                "HTTP {}: {}",
                status.as_u16(),
                truncate(&text, 200)
            ));
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
            401 | 403 => {
                return anyhow!("unauthorized — auth_token/ct0 invalid or expired ({status})");
            }
            429 => return anyhow!("rate limited by X — wait and retry (429)"),
            _ => {}
        }
    }
    err.into()
}

/// `CreateTweet` variables for a tweet, or a reply when `reply_to` is set.
pub fn create_tweet_variables(text: &str, reply_to: Option<&str>, media_ids: &[String]) -> Value {
    let entities: Vec<Value> = media_ids
        .iter()
        .map(|id| serde_json::json!({ "media_id": id, "tagged_users": [] }))
        .collect();
    let mut variables = serde_json::json!({
        "tweet_text": text,
        "dark_request": false,
        "media": { "media_entities": entities, "possibly_sensitive": false },
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

fn mime_for_path(path: &std::path::Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        other => return Err(anyhow!("unsupported media type: .{other}")),
    };
    Ok(mime.to_string())
}

fn media_category(mime: &str) -> &'static str {
    if mime == "image/gif" {
        "tweet_gif"
    } else if mime.starts_with("image/") {
        "tweet_image"
    } else {
        "tweet_video"
    }
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

/// Feature flags for `UserByScreenName` (profile lookup carries its own set).
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

/// Feature flags shared by the `TweetDetail` and `SearchTimeline` read queries.
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

/// Feature flags for `CreateTweet` (writes carry a larger set than reads).
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
        let v = create_tweet_variables("hello world", None, &[]);
        assert_eq!(v["tweet_text"], "hello world");
        assert_eq!(v["dark_request"], false);
        assert!(v.get("reply").is_none());
        assert_eq!(v["media"]["media_entities"], serde_json::json!([]));
    }

    #[test]
    fn reply_variables_carry_parent_id() {
        let v = create_tweet_variables("nice", Some("123"), &[]);
        assert_eq!(v["tweet_text"], "nice");
        assert_eq!(v["reply"]["in_reply_to_tweet_id"], "123");
        assert_eq!(v["reply"]["exclude_reply_user_ids"], serde_json::json!([]));
    }

    #[test]
    fn media_ids_become_entities() {
        let v = create_tweet_variables("pic", None, &["999".to_string()]);
        assert_eq!(v["media"]["media_entities"][0]["media_id"], "999");
        assert_eq!(
            v["media"]["media_entities"][0]["tagged_users"],
            serde_json::json!([])
        );
    }

    #[test]
    fn media_category_by_mime() {
        assert_eq!(media_category("image/png"), "tweet_image");
        assert_eq!(media_category("image/gif"), "tweet_gif");
        assert_eq!(media_category("video/mp4"), "tweet_video");
    }

    #[test]
    fn classifies_transient_vs_fatal_errors() {
        assert!(is_transient("DeadlineExceeded: Unspecified"));
        assert!(is_transient("Dependency: Timedout"));
        assert!(!is_transient("Could not authenticate you."));
        assert!(!is_transient("Bad guest token"));
    }

    #[test]
    fn errors_beside_real_data_are_not_fatal() {
        // A deep bookmarks page: X reports an error and hands over the tweets anyway.
        let partial =
            serde_json::json!({ "bookmark_timeline_v2": { "timeline": { "instructions": [] } } });
        assert!(
            has_content(&partial),
            "a populated data block must survive an error"
        );
        // Nothing to salvage: the error is the whole response.
        assert!(!has_content(&serde_json::json!(null)));
        assert!(!has_content(&serde_json::json!({})));
    }

    #[test]
    fn only_reads_tolerate_a_partial_response() {
        // The rule graphql_call applies: tolerate iff the call is not a write and
        // something came back. `action` throws the body away, so a tolerated error
        // on a like or a delete would reach the user as success.
        let tolerated =
            |call: Call, data: &Value| !matches!(call, Call::Write) && has_content(data);
        let with_data = serde_json::json!({ "favorite_tweet": "Done" });

        assert!(tolerated(Call::Read, &with_data));
        assert!(tolerated(Call::Search, &with_data));
        assert!(
            !tolerated(Call::Write, &with_data),
            "a write must surface its error, not swallow it"
        );
        assert!(!tolerated(Call::Read, &serde_json::json!({})));
    }

    #[test]
    fn bottom_cursor_reads_both_shapes_x_uses() {
        // A real Bookmarks page: the value sits directly on content.
        let timeline = serde_json::json!([{
            "type": "TimelineAddEntries",
            "entries": [
                { "entryId": "tweet-1", "content": {} },
                { "entryId": "cursor-top-9", "content": { "cursorType": "Top", "value": "TOP" } },
                { "entryId": "cursor-bottom-9", "content": {
                    "entryType": "TimelineTimelineCursor", "cursorType": "Bottom", "value": "NEXT" } }
            ]
        }]);
        assert_eq!(bottom_cursor(&timeline).as_deref(), Some("NEXT"));

        // A conversation thread: the value is nested under itemContent.
        let thread = serde_json::json!([{
            "entries": [
                { "entryId": "cursor-bottom-1", "content": {
                    "itemContent": { "cursorType": "Bottom", "value": "NESTED" } } }
            ]
        }]);
        assert_eq!(bottom_cursor(&thread).as_deref(), Some("NESTED"));
    }

    #[test]
    fn bottom_cursor_absent_ends_pagination() {
        let last_page =
            serde_json::json!([{ "entries": [{ "entryId": "tweet-1", "content": {} }] }]);
        assert_eq!(bottom_cursor(&last_page), None);
        // A cursor entry with no value must not be mistaken for one.
        let valueless =
            serde_json::json!([{ "entries": [{ "entryId": "cursor-bottom-1", "content": {} }] }]);
        assert_eq!(bottom_cursor(&valueless), None);
        assert_eq!(bottom_cursor(&serde_json::json!({})), None);
    }

    #[test]
    fn article_toggle_only_for_operations_that_declare_it() {
        assert_eq!(
            field_toggles("TweetDetail"),
            Some(serde_json::json!({ "withArticlePlainText": true }))
        );
        assert!(field_toggles("Bookmarks").is_some());
        // User and mutation operations reject a toggle they never declared.
        assert!(field_toggles("UserByScreenName").is_none());
        assert!(field_toggles("CreateTweet").is_none());
    }
}
