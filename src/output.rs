use crate::models::{Profile, Tweet};

/// Profile view (user).
pub fn print_profile(p: &Profile, json: bool, plain: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(p).unwrap_or_default());
        return;
    }
    let check = if p.verified && !plain { " ✓" } else { "" };
    println!("@{} ({}){check}", p.handle, p.name);
    if !p.bio.is_empty() {
        println!("{}", p.bio);
    }
    let people = tok(plain, "👥", "");
    let loc = p.location.as_deref().map(|l| format!("  📍 {l}")).unwrap_or_default();
    let loc = if plain { loc.replace("📍 ", "location: ") } else { loc };
    println!("{people} {} followers · {} following{loc}", p.followers, p.following);
}

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

/// Compact profile-list view (follow graph).
pub fn print_profiles(profiles: &[Profile], json: bool, empty_message: &str) {
    if json {
        println!("{}", serde_json::to_string_pretty(profiles).unwrap_or_default());
        return;
    }
    if profiles.is_empty() {
        println!("{empty_message}");
        return;
    }
    for p in profiles {
        println!("@{} ({})", p.handle, p.name);
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
