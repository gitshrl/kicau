use serde_json::Value;

use crate::models::{Author, DmConversation, DmMessage, Profile, Tweet};

/// The `account.js` from an X data export → the archive owner as an Author.
/// Files are JS-wrapped (`window.YTD.account.part0 = [...]`); the JSON array
/// begins at the first `[`.
pub fn parse_archive_account(js: &str) -> Option<Author> {
    let arr = strip_js_wrapper(js)?;
    let acct = arr.get(0)?.get("account")?;
    let username = str_at(acct, "/username")?;
    Some(Author {
        name: str_at(acct, "/accountDisplayName").unwrap_or_else(|| username.clone()),
        id: str_at(acct, "/accountId").unwrap_or_default(),
        username,
    })
}

/// The `tweets.js` from an X data export → owned tweets attributed to `author`.
pub fn parse_archive_tweets(js: &str, author: &Author) -> Vec<Tweet> {
    let Some(arr) = strip_js_wrapper(js) else { return Vec::new() };
    let mut tweets = Vec::new();
    for entry in as_array(&arr) {
        let t = &entry["tweet"];
        let Some(id) = str_at(t, "/id_str") else { continue };
        tweets.push(Tweet {
            id,
            text: str_at(t, "/full_text").unwrap_or_default(),
            author: author.clone(),
            created_at: str_at(t, "/created_at").map(|d| to_iso8601(&d)),
            reply_count: None,
            retweet_count: str_at(t, "/retweet_count").and_then(|s| s.parse().ok()),
            like_count: str_at(t, "/favorite_count").and_then(|s| s.parse().ok()),
            conversation_id: None,
            in_reply_to_status_id: str_at(t, "/in_reply_to_status_id_str"),
        });
    }
    tweets
}

fn strip_js_wrapper(js: &str) -> Option<Value> {
    let start = js.find('[')?;
    serde_json::from_str(&js[start..]).ok()
}

/// Parse a DM `inbox_initial_state` into conversations and messages. `my_id` is
/// the current account's user id, used to name one-to-one conversations by the
/// other participant.
pub fn parse_dm_inbox(data: &Value, my_id: &str) -> (Vec<DmConversation>, Vec<DmMessage>) {
    let inbox = &data["inbox_initial_state"];
    let users = &inbox["users"];
    let handle = |id: &str| -> String {
        users
            .get(id)
            .and_then(|u| u.get("screen_name"))
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string()
    };

    let mut conversations = Vec::new();
    if let Some(map) = inbox["conversations"].as_object() {
        for (id, conv) in map {
            let others: Vec<String> = as_array(&conv["participants"])
                .iter()
                .filter_map(|p| str_at(p, "/user_id"))
                .filter(|uid| uid != my_id)
                .map(|uid| format!("@{}", handle(&uid)))
                .collect();
            conversations.push(DmConversation {
                id: id.clone(),
                kind: str_at(conv, "/type").unwrap_or_default(),
                title: if others.is_empty() { id.clone() } else { others.join(", ") },
            });
        }
    }

    let mut messages = Vec::new();
    for entry in as_array(&inbox["entries"]) {
        let message = &entry["message"];
        if message.is_null() {
            continue;
        }
        let md = &message["message_data"];
        let sender = str_at(md, "/sender_id").unwrap_or_default();
        messages.push(DmMessage {
            id: str_at(message, "/id").unwrap_or_default(),
            conversation_id: str_at(message, "/conversation_id").unwrap_or_default(),
            sender_handle: handle(&sender),
            sender_id: sender,
            text: str_at(md, "/text").unwrap_or_default(),
            created_at: unix_millis_to_iso(json_millis(&md["time"])),
        });
    }
    messages.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    (conversations, messages)
}

fn json_millis(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}

/// Unix milliseconds → ISO 8601 UTC, no external date dependency (Hinnant's
/// civil-from-days algorithm).
pub fn unix_millis_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    let (h, mi, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{mi:02}:{s:02}.000Z")
}

/// Map a UserByScreenName `data.user.result` into a Profile. X is migrating
/// user fields from legacy.* to core.*, so both are tried.
pub fn parse_user(result: &Value) -> Option<Profile> {
    if result["__typename"] == "UserUnavailable" {
        return None;
    }
    let handle = str_at(result, "/legacy/screen_name")
        .or_else(|| str_at(result, "/core/screen_name"))?;
    let name = str_at(result, "/legacy/name")
        .or_else(|| str_at(result, "/core/name"))
        .unwrap_or_else(|| handle.clone());
    Some(Profile {
        id: str_at(result, "/rest_id").unwrap_or_default(),
        handle,
        name,
        bio: str_at(result, "/legacy/description").unwrap_or_default(),
        followers: result.pointer("/legacy/followers_count").and_then(Value::as_u64).unwrap_or(0),
        following: result.pointer("/legacy/friends_count").and_then(Value::as_u64).unwrap_or(0),
        location: str_at(result, "/legacy/location").filter(|s| !s.is_empty()),
        verified: result.pointer("/is_blue_verified").and_then(Value::as_bool).unwrap_or(false)
            || result.pointer("/legacy/verified").and_then(Value::as_bool).unwrap_or(false),
    })
}

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

/// Walk timeline `instructions` collecting user profiles from `user_results`
/// entries (Following/Followers timelines).
pub fn users_from_instructions(instructions: &Value) -> Vec<Profile> {
    let mut users = Vec::new();
    for instruction in as_array(instructions) {
        for entry in as_array(&instruction["entries"]) {
            if let Some(p) = entry
                .pointer("/content/itemContent/user_results/result")
                .and_then(parse_user)
            {
                users.push(p);
            }
        }
    }
    users
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
    fn parses_user_profile() {
        let result = json!({
            "rest_id": "44196397",
            "is_blue_verified": true,
            "legacy": {
                "screen_name": "elonmusk",
                "name": "Elon Musk",
                "description": "tech",
                "followers_count": 200000000_u64,
                "friends_count": 500,
                "location": "Mars"
            }
        });
        let p = parse_user(&result).unwrap();
        assert_eq!(p.id, "44196397");
        assert_eq!(p.handle, "elonmusk");
        assert_eq!(p.name, "Elon Musk");
        assert_eq!(p.followers, 200000000);
        assert_eq!(p.location.as_deref(), Some("Mars"));
        assert!(p.verified);
    }

    #[test]
    fn unavailable_user_yields_none() {
        assert!(parse_user(&json!({ "__typename": "UserUnavailable" })).is_none());
    }

    #[test]
    fn parses_x_data_export() {
        let account_js = r#"window.YTD.account.part0 = [ { "account" : {
            "accountId" : "77", "username" : "me", "accountDisplayName" : "Me" } } ]"#;
        let author = parse_archive_account(account_js).unwrap();
        assert_eq!(author.id, "77");
        assert_eq!(author.username, "me");

        let tweets_js = r#"window.YTD.tweets.part0 = [
            { "tweet" : { "id_str" : "5", "full_text" : "archived tweet",
              "created_at" : "Wed Oct 10 20:19:24 +0000 2018",
              "favorite_count" : "9", "retweet_count" : "3" } }
        ]"#;
        let tweets = parse_archive_tweets(tweets_js, &author);
        assert_eq!(tweets.len(), 1);
        assert_eq!(tweets[0].id, "5");
        assert_eq!(tweets[0].text, "archived tweet");
        assert_eq!(tweets[0].author.username, "me");
        assert_eq!(tweets[0].like_count, Some(9));
        assert_eq!(tweets[0].created_at.as_deref(), Some("2018-10-10T20:19:24.000Z"));
    }

    #[test]
    fn unix_millis_to_iso_known_values() {
        assert_eq!(unix_millis_to_iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(unix_millis_to_iso(1_000_000_000_000), "2001-09-09T01:46:40.000Z");
    }

    #[test]
    fn parses_dm_inbox() {
        let data = json!({
            "inbox_initial_state": {
                "users": {
                    "me": { "screen_name": "me" },
                    "them": { "screen_name": "alice" }
                },
                "conversations": {
                    "c1": { "type": "ONE_TO_ONE", "participants": [{ "user_id": "me" }, { "user_id": "them" }] }
                },
                "entries": [
                    { "message": { "id": "m2", "conversation_id": "c1", "message_data": { "sender_id": "them", "text": "hi back", "time": "1000000001000" } } },
                    { "message": { "id": "m1", "conversation_id": "c1", "message_data": { "sender_id": "me", "text": "hi", "time": 1000000000000_i64 } } }
                ]
            }
        });
        let (convs, msgs) = parse_dm_inbox(&data, "me");
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].title, "@alice", "one-to-one names the other participant");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, "m1", "sorted oldest first");
        assert_eq!(msgs[0].sender_handle, "me");
        assert_eq!(msgs[1].text, "hi back");
    }

    #[test]
    fn users_from_timeline_instructions() {
        let instructions = json!([{
            "entries": [
                { "content": { "itemContent": { "user_results": { "result": {
                    "rest_id": "7", "legacy": { "screen_name": "bob", "name": "Bob" }
                } } } } },
                { "content": { "itemContent": { "cursorType": "Bottom" } } }
            ]
        }]);
        let users = users_from_instructions(&instructions);
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].handle, "bob");
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
