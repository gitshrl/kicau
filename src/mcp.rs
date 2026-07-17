//! An MCP server over stdio, so an agent can read the archive without shelling
//! out and parsing text.
//!
//! Hand-rolled against the 2025-11-25 spec rather than pulled from an SDK: the
//! protocol here is four methods of line-delimited JSON-RPC, and the crate that
//! would supply them brings proc-macros and a schema generator for code that
//! fits on a page.
//!
//! Two rules the transport enforces, both fatal to get wrong:
//!
//! - **stdout carries protocol and nothing else.** A client parses every line as
//!   JSON; one stray `println!` and the session dies. Anything human goes to
//!   stderr, which the spec reserves for exactly that.
//! - **A failing tool is not a failing request.** Protocol errors mean the call
//!   never happened; a tool that ran and failed returns `isError: true` so the
//!   model can read the reason and try something else.

use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::TwitterClient;
use crate::{db, extract, models::Tweet};

/// The version this server speaks. Clients hold a fixed list of acceptable
/// versions and hang up on anything outside it, so this is not a free choice.
const PROTOCOL_VERSION: &str = "2025-11-25";

const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
const JSONRPC_INVALID_PARAMS: i64 = -32602;

/// Serve MCP on stdin/stdout until the client closes the pipe.
///
/// `client` is optional: every tool but `read_tweet` answers from the local
/// archive, so a server with no credentials still answers everything else.
/// `read_tweet` is the only network tool and the only one that needs cookies.
pub async fn serve(client: Option<TwitterClient>) -> Result<()> {
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        // Read the raw line rather than `lines()`, which errors out of the whole
        // session on a non-UTF-8 byte. A lossy decode turns a bad byte into a
        // replacement char, so the line merely fails to parse and is skipped,
        // the same as any other garbage.
        if reader.read_until(b'\n', &mut buf).await? == 0 {
            break; // EOF: the client closed the pipe.
        }
        let msg = String::from_utf8_lossy(&buf);
        let msg = msg.trim();
        if msg.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(msg) else {
            // A malformed line has no id to answer, so there is nobody to tell.
            eprintln!("kicau mcp: ignoring unparseable line");
            continue;
        };

        // No id means a notification: act on it, answer nothing.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        let response = match dispatch(method, &params, client.as_ref()).await {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(e) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": e.code, "message": e.message }
            }),
        };

        stdout.write_all(format!("{response}\n").as_bytes()).await?;
        stdout.flush().await?;
    }
    Ok(())
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

async fn dispatch(
    method: &str,
    params: &Value,
    client: Option<&TwitterClient>,
) -> std::result::Result<Value, RpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "kicau", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(params, client).await,
        other => Err(RpcError {
            code: JSONRPC_METHOD_NOT_FOUND,
            message: format!("unknown method: {other}"),
        }),
    }
}

/// A tool that ran and failed: the model sees the reason and can recover.
fn tool_failure(message: impl std::fmt::Display) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.to_string() }],
        "isError": true
    })
}

fn tool_text(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

async fn call_tool(
    params: &Value,
    client: Option<&TwitterClient>,
) -> std::result::Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Only failing to *find* the tool is a protocol error. Everything past this
    // point ran, so it reports through isError instead.
    let known = TOOLS.iter().any(|t| t.0 == name);
    if !known {
        return Err(RpcError {
            code: JSONRPC_INVALID_PARAMS,
            message: format!("unknown tool: {name}"),
        });
    }

    Ok(run(name, &args, client).await.unwrap_or_else(tool_failure))
}

async fn run(name: &str, args: &Value, client: Option<&TwitterClient>) -> Result<Value> {
    let limit = |default: u32| -> u32 {
        args.get("limit")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(default)
            .clamp(1, 200)
    };
    let text_arg = |key: &str| -> String {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    match name {
        "search_archive" => {
            let db = db::Db::open_default()?;
            let query = text_arg("query");
            let folder = args.get("folder").and_then(Value::as_str);
            let tweets = match folder {
                Some(folder) => db.find_in_folder(&query, folder, limit(20))?,
                None => db.find(&query, limit(20))?,
            };
            Ok(tool_text(&render(
                &tweets,
                "Nothing in the archive matches.",
            )))
        }
        "read_tweet" => {
            let Some(client) = client else {
                return Ok(tool_failure(
                    "read_tweet needs X credentials; the other tools read the local archive. Run kicau init.",
                ));
            };
            let id = extract::extract_tweet_id(&text_arg("tweet"));
            let tweet = client.get_tweet(&id).await?;
            // Reading is also archiving, exactly as the CLI behaves.
            if let Ok(mut db) = db::Db::open_default() {
                let _ = db.archive(std::slice::from_ref(&tweet));
            }
            Ok(tool_text(&render(&[tweet], "")))
        }
        "list_bookmarks" => {
            let db = db::Db::open_default()?;
            let tweets = match args.get("folder").and_then(Value::as_str) {
                Some(folder) => db.folder_tweets(folder, limit(20))?,
                None => db.collection("bookmarks", limit(20))?,
            };
            Ok(tool_text(&render(
                &tweets,
                "No bookmarks archived yet. Run: kicau sync bookmarks",
            )))
        }
        "recent_tweets" => {
            let db = db::Db::open_default()?;
            Ok(tool_text(&render(
                &db.recent(limit(20))?,
                "Nothing archived yet.",
            )))
        }
        "archive_stats" => {
            let db = db::Db::open_default()?;
            let s = db.stats()?;
            let folders = db.folders()?;
            Ok(tool_text(&format!(
                "tweets: {}\nprofiles: {}\ncollections: {}\nfollow edges: {}\ndms: {}\nsize: {} KiB\nbookmark folders: {}",
                s.tweets,
                s.profiles,
                s.collections,
                s.edges,
                s.dms,
                s.bytes / 1024,
                folders
                    .iter()
                    .map(|(name, n)| format!("{name} ({n})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
        other => Ok(tool_failure(format!("unhandled tool: {other}"))),
    }
}

/// Tweets as text a model can read: enough to cite, not so much that a page of
/// results buries the answer. Article bodies stay whole, since they are the
/// reason the archive is worth asking.
fn render(tweets: &[Tweet], empty: &str) -> String {
    if tweets.is_empty() {
        return empty.to_string();
    }
    tweets
        .iter()
        .map(|t| {
            let when = t.created_at.as_deref().unwrap_or("");
            format!(
                "@{} ({})  {}\nhttps://x.com/{}/status/{}\n{}",
                t.author.username, t.author.name, when, t.author.username, t.id, t.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// (name, description, JSON Schema for the arguments).
type ToolDef = (&'static str, &'static str, fn() -> Value);

const TOOLS: &[ToolDef] = &[
    (
        "search_archive",
        "Full-text search the local kicau archive of X posts. Instant and offline: it never calls X. Covers everything synced, including the full text of long-form Articles. Optionally scope to one bookmark folder.",
        || {
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "words to search for; an empty query with a folder returns that whole folder" },
                    "folder": { "type": "string", "description": "restrict to one bookmark folder, e.g. \"AI Engineering\"" },
                    "limit": { "type": "integer", "description": "max results, 1-200 (default 20)" }
                },
                "required": ["query"]
            })
        },
    ),
    (
        "read_tweet",
        "Fetch one post from X by id or URL, live. Returns the full text, including the body of an Article, and archives it locally. This is the only tool that calls X, so it is slower and subject to rate limits; prefer search_archive when the post may already be archived.",
        || {
            json!({
                "type": "object",
                "properties": {
                    "tweet": { "type": "string", "description": "post id or full URL" }
                },
                "required": ["tweet"]
            })
        },
    ),
    (
        "list_bookmarks",
        "List archived bookmarks, newest first, from the local archive. Optionally only those filed in one bookmark folder. Reads nothing from X.",
        || {
            json!({
                "type": "object",
                "properties": {
                    "folder": { "type": "string", "description": "only bookmarks filed in this folder" },
                    "limit": { "type": "integer", "description": "max results, 1-200 (default 20)" }
                }
            })
        },
    ),
    (
        "recent_tweets",
        "The most recently archived posts, newest first, from the local archive. Reads nothing from X.",
        || {
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "max results, 1-200 (default 20)" }
                }
            })
        },
    ),
    (
        "archive_stats",
        "What the local archive holds: counts of posts, profiles, collections, DMs, its size on disk, and the bookmark folders with their sizes. Useful for deciding whether search_archive can answer a question.",
        || json!({ "type": "object", "properties": {} }),
    ),
];

fn tools() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|(name, description, schema)| {
            json!({ "name": name, "description": description, "inputSchema": schema() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_answers_with_a_version_clients_accept() {
        // Clients hold a fixed accept-list and hang up on anything outside it.
        // This is the whole handshake: get it wrong and nothing else runs.
        const ACCEPTED: &[&str] = &[
            "2025-11-25",
            "2025-06-18",
            "2025-03-26",
            "2024-11-05",
            "2024-10-07",
        ];
        let result = dispatch("initialize", &json!({}), None).await.ok().unwrap();
        let version = result["protocolVersion"].as_str().unwrap();
        assert!(
            ACCEPTED.contains(&version),
            "unsupported version: {version}"
        );
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "kicau");
    }

    #[tokio::test]
    async fn every_tool_declares_a_usable_schema_and_read_tweet_is_listed() {
        let result = dispatch("tools/list", &json!({}), None).await.ok().unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
        assert!(
            tools.iter().any(|t| t["name"] == "read_tweet"),
            "read_tweet is available by default"
        );
        for tool in tools {
            assert!(!tool["name"].as_str().unwrap().is_empty());
            assert!(
                tool["description"].as_str().unwrap().len() > 40,
                "a terse description gives the model nothing to choose on"
            );
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[tokio::test]
    async fn an_unknown_method_is_a_protocol_error() {
        let err = dispatch("tools/nope", &json!({}), None)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, JSONRPC_METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_tool_is_invalid_params_not_method_not_found() {
        let err = call_tool(&json!({ "name": "nope" }), None)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, JSONRPC_INVALID_PARAMS);
    }

    #[tokio::test]
    async fn read_tweet_without_credentials_asks_for_init() {
        // read_tweet is the one tool that needs cookies. Without them it ran and
        // could not answer, which is a result the model can act on, not a
        // protocol error.
        let out = call_tool(
            &json!({ "name": "read_tweet", "arguments": { "tweet": "1" } }),
            None,
        )
        .await
        .expect("a failing tool must not fail the request");
        assert_eq!(out["isError"], true);
        assert!(
            out["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("kicau init")
        );
    }

    #[test]
    fn rendering_keeps_the_link_and_the_whole_body() {
        let tweet = Tweet {
            id: "9".into(),
            text: "a very long article body".into(),
            author: crate::models::Author {
                id: "u9".into(),
                username: "dankoe".into(),
                name: "DAN KOE".into(),
            },
            created_at: Some("2026-07-15T21:58:40.000Z".into()),
            reply_count: None,
            retweet_count: None,
            like_count: None,
            conversation_id: None,
            in_reply_to_status_id: None,
        };
        let out = render(std::slice::from_ref(&tweet), "empty");
        assert!(out.contains("https://x.com/dankoe/status/9"));
        assert!(out.contains("a very long article body"));
        assert_eq!(render(&[], "empty"), "empty");
    }
}
