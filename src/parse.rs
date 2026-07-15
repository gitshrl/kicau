use serde_json::Value;

use crate::models::{Author, Tweet};

/// Map a raw GraphQL `tweet_results.result` into a Tweet. Returns None when it
/// lacks a legacy block or an author handle (cursors, tombstones, ads).
pub fn map_tweet_result(result: &Value) -> Option<Tweet> {
    // Newer results wrap the tweet in a visibility envelope.
    let result = if result["__typename"] == "TweetWithVisibilityResults" {
        result.get("tweet")?
    } else {
        result
    };

    let legacy = result.get("legacy")?;
    let user = result.pointer("/core/user_results/result")?;
    // X is migrating author fields from legacy.* to core.*; try both.
    let username = str_at(user, "/legacy/screen_name").or_else(|| str_at(user, "/core/screen_name"))?;
    let name = str_at(user, "/legacy/name")
        .or_else(|| str_at(user, "/core/name"))
        .unwrap_or_else(|| username.clone());
    let author_id = str_at(user, "/rest_id").unwrap_or_default();

    // Articles and long-form note tweets keep the real body outside legacy;
    // full_text is only a t.co stub for them.
    let text = article_text(result)
        .or_else(|| str_at(result, "/note_tweet/note_tweet_results/result/text"))
        .or_else(|| str_at(legacy, "/full_text"))
        .unwrap_or_default();

    Some(Tweet {
        id: str_at(result, "/rest_id").unwrap_or_default(),
        text,
        author: Author { id: author_id, username, name },
        created_at: str_at(legacy, "/created_at").map(|d| to_iso8601(&d)),
        reply_count: legacy["reply_count"].as_u64(),
        retweet_count: legacy["retweet_count"].as_u64(),
        like_count: legacy["favorite_count"].as_u64(),
        conversation_id: str_at(legacy, "/conversation_id_str"),
        in_reply_to_status_id: str_at(legacy, "/in_reply_to_status_id_str"),
    })
}

/// Walk timeline `instructions` (TweetDetail or SearchTimeline shape) collecting
/// every tweet. Handles both direct `itemContent` entries and the nested
/// `items[]` of conversationthread modules.
pub fn tweets_from_instructions(instructions: &Value) -> Vec<Tweet> {
    let mut tweets = Vec::new();
    for instruction in as_array(instructions) {
        for entry in as_array(&instruction["entries"]) {
            if let Some(t) = entry
                .pointer("/content/itemContent/tweet_results/result")
                .and_then(map_tweet_result)
            {
                tweets.push(t);
            }
            for item in as_array(&entry["content"]["items"]) {
                if let Some(t) = item
                    .pointer("/item/itemContent/tweet_results/result")
                    .and_then(map_tweet_result)
                {
                    tweets.push(t);
                }
            }
        }
    }
    tweets
}

/// Title + preview for an article tweet. ponytail: full draft.js content_state
/// rendering is out of scope; preview_text is what this response carries.
fn article_text(result: &Value) -> Option<String> {
    let article = result.pointer("/article/article_results/result")?;
    let title = str_at(article, "/title").map(|s| s.trim().to_string());
    let preview = str_at(article, "/preview_text");
    match (title, preview) {
        (Some(t), Some(p)) => Some(format!("{t}\n\n{p}")),
        (Some(t), None) => Some(t),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    }
}

fn str_at(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer).and_then(Value::as_str).map(str::to_string)
}

/// Normalize X's wire date `Mon Jul 06 19:08:45 +0000 2026` to ISO 8601 so every
/// surface — output, --json, DB storage, thread sort — uses one sortable format.
/// Returns the input unchanged if it doesn't parse.
pub fn to_iso8601(x: &str) -> String {
    let p: Vec<&str> = x.split_whitespace().collect();
    if p.len() != 6 {
        return x.to_string();
    }
    let month = match p[1] {
        "Jan" => "01", "Feb" => "02", "Mar" => "03", "Apr" => "04",
        "May" => "05", "Jun" => "06", "Jul" => "07", "Aug" => "08",
        "Sep" => "09", "Oct" => "10", "Nov" => "11", "Dec" => "12",
        _ => return x.to_string(),
    };
    // p = [dow, mon, dd, hh:mm:ss, tz, yyyy]
    format!("{year}-{month}-{day}T{time}.000Z", year = p[5], day = p[2], time = p[3])
}

fn as_array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn instructions() -> Value {
        json!([{
            "type": "TimelineAddEntries",
            "entries": [
                {
                    "entryId": "tweet-1",
                    "content": { "itemContent": { "tweet_results": { "result": {
                        "__typename": "Tweet",
                        "rest_id": "1",
                        "note_tweet": { "note_tweet_results": { "result": { "text": "long note body" } } },
                        "legacy": {
                            "full_text": "truncated https://t.co/x",
                            "created_at": "Mon Jul 06 19:08:45 +0000 2026",
                            "reply_count": 5, "retweet_count": 4, "favorite_count": 9,
                            "conversation_id_str": "1", "in_reply_to_status_id_str": null
                        },
                        "core": { "user_results": { "result": {
                            "rest_id": "900",
                            "legacy": { "screen_name": "alice", "name": "Alice" }
                        } } }
                    } } } }
                },
                { "entryId": "cursor-bottom-9", "content": { "itemContent": { "cursorType": "Bottom" } } },
                {
                    "entryId": "conversationthread-2",
                    "content": { "items": [ { "item": { "itemContent": { "tweet_results": { "result": {
                        "__typename": "TweetWithVisibilityResults",
                        "tweet": {
                            "rest_id": "2",
                            "legacy": {
                                "full_text": "a reply",
                                "reply_count": 0, "retweet_count": 0, "favorite_count": 1,
                                "conversation_id_str": "1", "in_reply_to_status_id_str": "1"
                            },
                            "core": { "user_results": { "result": {
                                "core": { "screen_name": "bob", "name": "Bob" }
                            } } }
                        }
                    } } } } } ] }
                }
            ]
        }])
    }

    #[test]
    fn collects_direct_and_module_tweets() {
        let tweets = tweets_from_instructions(&instructions());
        assert_eq!(tweets.len(), 2, "cursor skipped, both tweets kept");
    }

    #[test]
    fn note_text_beats_truncated_full_text() {
        let tweets = tweets_from_instructions(&instructions());
        assert_eq!(tweets[0].text, "long note body");
        assert_eq!(tweets[0].id, "1");
        assert_eq!(tweets[0].author.id, "900");
        assert_eq!(tweets[0].author.username, "alice");
        assert_eq!(tweets[0].author.name, "Alice");
        assert_eq!(tweets[0].reply_count, Some(5));
        assert_eq!(tweets[0].conversation_id.as_deref(), Some("1"));
        assert_eq!(tweets[0].in_reply_to_status_id, None);
        // created_at is normalized to ISO 8601 at parse time
        assert_eq!(tweets[0].created_at.as_deref(), Some("2026-07-06T19:08:45.000Z"));
    }

    #[test]
    fn iso_conversion_is_sortable_and_lenient() {
        assert_eq!(to_iso8601("Mon Jul 06 19:08:45 +0000 2026"), "2026-07-06T19:08:45.000Z");
        assert_eq!(to_iso8601("garbage"), "garbage");
    }

    #[test]
    fn unwraps_visibility_wrapper_and_core_user() {
        let tweets = tweets_from_instructions(&instructions());
        assert_eq!(tweets[1].id, "2");
        assert_eq!(tweets[1].text, "a reply");
        assert_eq!(tweets[1].author.username, "bob"); // core.screen_name fallback
        assert_eq!(tweets[1].in_reply_to_status_id.as_deref(), Some("1"));
    }

    #[test]
    fn rejects_result_without_author() {
        let no_user = json!({ "rest_id": "9", "legacy": { "full_text": "x" } });
        assert!(map_tweet_result(&no_user).is_none());
    }

    #[test]
    fn article_uses_title_and_preview_over_tco_stub() {
        let result = json!({
            "rest_id": "5",
            "legacy": { "full_text": "https://t.co/stub" },
            "article": { "article_results": { "result": {
                "title": "Getting started with loops",
                "preview_text": "There's a lot of talk about designing loops."
            } } },
            "core": { "user_results": { "result": {
                "legacy": { "screen_name": "claude", "name": "Claude" }
            } } }
        });
        let tweet = map_tweet_result(&result).unwrap();
        assert_eq!(
            tweet.text,
            "Getting started with loops\n\nThere's a lot of talk about designing loops."
        );
    }
}
