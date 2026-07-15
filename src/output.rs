use crate::models::Tweet;

/// Single-tweet view (read): text, date, engagement counts.
pub fn print_tweet(tweet: &Tweet, json: bool, plain: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(tweet).unwrap_or_default());
        return;
    }
    println!("@{} ({}):", tweet.author.username, tweet.author.name);
    println!("{}", tweet.text);
    if let Some(created) = &tweet.created_at {
        println!("\n{} {created}", tok(plain, "📅", "date:"));
    }
    if plain {
        println!(
            "likes: {}  retweets: {}  replies: {}",
            tweet.like_count.unwrap_or(0),
            tweet.retweet_count.unwrap_or(0),
            tweet.reply_count.unwrap_or(0),
        );
    } else {
        println!(
            "❤️ {}  🔁 {}  💬 {}",
            tweet.like_count.unwrap_or(0),
            tweet.retweet_count.unwrap_or(0),
            tweet.reply_count.unwrap_or(0),
        );
    }
}

/// Tweet-list view (search/mentions/replies/thread): blocks separated by a rule.
pub fn print_tweets(tweets: &[Tweet], json: bool, plain: bool, empty_message: &str) {
    if json {
        println!("{}", serde_json::to_string_pretty(tweets).unwrap_or_default());
        return;
    }
    if tweets.is_empty() {
        println!("{empty_message}");
        return;
    }
    for tweet in tweets {
        println!("\n@{} ({}):", tweet.author.username, tweet.author.name);
        println!("{}", tweet.text);
        if let Some(created) = &tweet.created_at {
            println!("{} {created}", tok(plain, "📅", "date:"));
        }
        println!(
            "{} https://x.com/{}/status/{}",
            tok(plain, "🔗", "url:"),
            tweet.author.username,
            tweet.id,
        );
        println!("{}", "─".repeat(50));
    }
}

fn tok<'a>(plain: bool, emoji: &'a str, label: &'a str) -> &'a str {
    if plain {
        label
    } else {
        emoji
    }
}
