use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Author {
    pub id: String,
    pub username: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub handle: String,
    pub name: String,
    pub bio: String,
    pub followers: u64,
    pub following: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tweet {
    pub id: String,
    pub text: String,
    pub author: Author,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retweet_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to_status_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tweet {
        Tweet {
            id: "1".into(),
            text: "hi".into(),
            author: Author { id: "42".into(), username: "u".into(), name: "n".into() },
            created_at: Some("Mon Jul 06 19:08:45 +0000 2026".into()),
            reply_count: Some(3),
            retweet_count: Some(2),
            like_count: Some(1),
            conversation_id: None,
            in_reply_to_status_id: None,
        }
    }

    #[test]
    fn serializes_camel_case_json() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["createdAt"], "Mon Jul 06 19:08:45 +0000 2026");
        assert_eq!(v["replyCount"], 3);
        assert_eq!(v["retweetCount"], 2);
        assert_eq!(v["likeCount"], 1);
        assert_eq!(v["author"]["username"], "u");
    }

    #[test]
    fn omits_none_fields() {
        let v = serde_json::to_value(sample()).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("conversationId"));
        assert!(!obj.contains_key("inReplyToStatusId"));
    }
}
