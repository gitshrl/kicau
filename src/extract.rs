/// Pull the tweet id out of an x.com/twitter.com status URL, or return the input
/// unchanged when it is already a bare id.
pub fn extract_tweet_id(input: &str) -> String {
    let re = regex::Regex::new(r"(?:twitter\.com|x\.com)/\w+/status/(\d+)").unwrap();
    match re.captures(input).and_then(|c| c.get(1)) {
        Some(m) => m.as_str().to_string(),
        None => input.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_id_passes_through() {
        assert_eq!(extract_tweet_id("2074208949205881033"), "2074208949205881033");
    }

    #[test]
    fn extracts_from_x_url() {
        assert_eq!(
            extract_tweet_id("https://x.com/ClaudeDevs/status/2074208949205881033"),
            "2074208949205881033"
        );
    }

    #[test]
    fn extracts_from_twitter_url_with_query() {
        assert_eq!(
            extract_tweet_id("https://twitter.com/foo/status/123456?s=20"),
            "123456"
        );
    }
}
