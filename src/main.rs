mod client;
mod db;
mod extract;
mod models;
mod output;
mod parse;
mod query_ids;

use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

use client::TwitterClient;

#[derive(Parser)]
#[command(name = "kicau", version, about = "Cookie-based X/Twitter GraphQL CLI")]
struct Cli {
    /// Override auth_token cookie
    #[arg(long, global = true)]
    auth_token: Option<String>,
    /// Override ct0 cookie
    #[arg(long, global = true)]
    ct0: Option<String>,
    /// Machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,
    /// No color/emoji
    #[arg(long, global = true)]
    plain: bool,
    /// Request timeout in milliseconds
    #[arg(long, global = true, default_value_t = 30000)]
    timeout: u64,
    /// Skip archiving fetched tweets to the local SQLite database
    #[arg(long, global = true)]
    no_db: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show which account the current credentials belong to
    Whoami,
    /// Show credential status and source
    Check,
    /// Read/fetch a tweet by id or URL
    Read {
        /// Tweet id or URL
        tweet: String,
    },
    /// Search latest tweets matching a query
    Search {
        /// Search query (e.g. "@handle" or "from:handle")
        query: String,
        /// Number of tweets to fetch
        #[arg(short = 'n', long, default_value_t = 10)]
        count: u32,
    },
    /// Find tweets mentioning the current account
    Mentions {
        /// Number of tweets to fetch
        #[arg(short = 'n', long, default_value_t = 10)]
        count: u32,
    },
    /// Post a new tweet
    Tweet {
        /// Tweet text
        text: String,
        /// Attach a media file (repeatable, up to 4)
        #[arg(long)]
        media: Vec<String>,
        /// Alt text for the media at the same position (repeatable)
        #[arg(long)]
        alt: Vec<String>,
        /// Print what would be posted without hitting X
        #[arg(long)]
        dry_run: bool,
    },
    /// Reply to a tweet by id or URL
    Reply {
        /// Tweet id or URL to reply to
        tweet: String,
        /// Reply text
        text: String,
        /// Attach a media file (repeatable, up to 4)
        #[arg(long)]
        media: Vec<String>,
        /// Alt text for the media at the same position (repeatable)
        #[arg(long)]
        alt: Vec<String>,
        /// Print what would be posted without hitting X
        #[arg(long)]
        dry_run: bool,
    },
    /// List replies to a tweet
    Replies {
        /// Tweet id or URL
        tweet: String,
    },
    /// Show the full conversation thread for a tweet
    Thread {
        /// Tweet id or URL
        tweet: String,
    },
    /// Fetch a live collection and persist it into the local SQLite store
    Sync {
        /// What to sync: bookmarks | tweets
        what: String,
        #[arg(short = 'n', long, default_value_t = 100)]
        limit: u32,
    },
    /// Snapshot a user's follow graph into the local store
    Graph {
        /// Direction: following | followers
        direction: String,
        /// @handle
        handle: String,
        #[arg(short = 'n', long, default_value_t = 50)]
        count: u32,
    },
    /// Capture a profile snapshot into the local store
    Profiles {
        /// @handle
        handle: String,
    },
    /// Show local database stats
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Scrape x.com for current GraphQL query ids and refresh the cache
    UpdateQueryIds,
    /// Full-text search the locally archived tweets
    Find {
        /// Text to search for
        query: String,
        /// Max results
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show the most recently archived tweets
    Log {
        /// Number of tweets to show
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show a user's profile
    User {
        /// @handle (with or without @)
        handle: String,
    },
    /// Show a user's tweets
    UserTweets {
        /// @handle (with or without @)
        handle: String,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show your chronological home timeline
    Home {
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show your bookmarks
    Bookmarks {
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show a list's tweets by list id
    List {
        /// List id
        list: String,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Like a tweet
    Like { tweet: String, #[arg(long)] dry_run: bool },
    /// Remove a like from a tweet
    Unlike { tweet: String, #[arg(long)] dry_run: bool },
    /// Retweet a tweet
    Retweet { tweet: String, #[arg(long)] dry_run: bool },
    /// Remove a retweet
    Unretweet { tweet: String, #[arg(long)] dry_run: bool },
    /// Bookmark a tweet
    Bookmark { tweet: String, #[arg(long)] dry_run: bool },
    /// Remove a bookmark
    Unbookmark { tweet: String, #[arg(long)] dry_run: bool },
    /// Upload a media file and print its media_id
    Upload {
        /// Path to an image/gif/video
        file: String,
        /// Alt text (images only)
        #[arg(long)]
        alt: Option<String>,
    },
    /// Follow a user
    Follow { handle: String, #[arg(long)] dry_run: bool },
    /// Unfollow a user
    Unfollow { handle: String, #[arg(long)] dry_run: bool },
}

#[derive(Subcommand)]
enum DbAction {
    /// Row counts and on-disk size
    Stats,
}

struct Credentials {
    auth_token: String,
    ct0: String,
    source: String,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("❌ {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    // check must report on partial/missing credentials, so it resolves its own.
    if matches!(cli.command, Command::Check) {
        return check(cli.auth_token, cli.ct0, cli.timeout, cli.plain).await;
    }
    // find/log are offline: they read the local archive, no credentials needed.
    match &cli.command {
        Command::Find { query, count } => {
            let tweets = db::Db::open_default()?.find(query, *count)?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No matching tweets archived.");
            return Ok(());
        }
        Command::Log { count } => {
            let tweets = db::Db::open_default()?.recent(*count)?;
            output::print_tweets(&tweets, cli.json, cli.plain, "Nothing archived yet.");
            return Ok(());
        }
        Command::Db { action: DbAction::Stats } => {
            let s = db::Db::open_default()?.stats()?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({ "tweets": s.tweets, "profiles": s.profiles, "collections": s.collections, "edges": s.edges, "bytes": s.bytes })
                );
            } else {
                println!("tweets:      {}", s.tweets);
                println!("profiles:    {}", s.profiles);
                println!("collections: {}", s.collections);
                println!("edges:       {}", s.edges);
                println!("size:        {} KiB", s.bytes / 1024);
            }
            return Ok(());
        }
        _ => {}
    }

    let creds = resolve_credentials(cli.auth_token.clone(), cli.ct0.clone())?;
    let client = TwitterClient::new(
        creds.auth_token,
        creds.ct0,
        Duration::from_millis(cli.timeout),
    )?;

    match cli.command {
        Command::Check | Command::Find { .. } | Command::Log { .. } | Command::Db { .. } => {
            unreachable!("handled above")
        }
        Command::Sync { what, limit } => {
            let user = client.current_user().await?;
            let tweets = match what.as_str() {
                "bookmarks" => client.bookmarks(limit).await?,
                "tweets" => client.user_tweets(&user.username, limit).await?,
                other => return Err(anyhow!("unknown sync target '{other}' (use: bookmarks | tweets)")),
            };
            let mut db = db::Db::open_default()?;
            let saved = db.archive_collection(&tweets, &user.id, &what)?;
            println!("✅ synced {saved} {what} into the local store");
            Ok(())
        }
        Command::Graph { direction, handle, count } => {
            let profiles = match direction.as_str() {
                "following" => client.following(&handle, count).await?,
                "followers" => client.followers(&handle, count).await?,
                other => return Err(anyhow!("unknown direction '{other}' (use: following | followers)")),
            };
            output::print_profiles(&profiles, cli.json, "No accounts found.");
            if !cli.no_db {
                let account = client.user(&handle).await?;
                let n = db::Db::open_default()?.save_follow_edges(&account.id, &direction, &profiles)?;
                if !cli.json {
                    eprintln!("📥 saved {n} {direction} edges");
                }
            }
            Ok(())
        }
        Command::Profiles { handle } => {
            let profile = client.user(&handle).await?;
            output::print_profile(&profile, cli.json, cli.plain);
            if !cli.no_db {
                db::Db::open_default()?.save_profile(&profile)?;
            }
            Ok(())
        }
        Command::Whoami => whoami(&client, &creds.source, cli.json, cli.plain).await,
        Command::Read { tweet } => {
            let id = extract::extract_tweet_id(&tweet);
            let tweet = client.get_tweet(&id).await?;
            output::print_tweet(&tweet, cli.json, cli.plain);
            archive(std::slice::from_ref(&tweet), cli.no_db);
            Ok(())
        }
        Command::Search { query, count } => {
            let tweets = client.search(&query, count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No tweets found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Mentions { count } => {
            let user = client.current_user().await?;
            let tweets = client.search(&format!("@{}", user.username), count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No mentions found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Tweet { text, media, alt, dry_run } => {
            if dry_run {
                println!("📝 [dry-run] would tweet: {text}");
                if !media.is_empty() {
                    println!("   with media: {}", media.join(", "));
                }
                return Ok(());
            }
            let media_ids = upload_all(&client, &media, &alt).await?;
            let id = client.post_tweet(&text, &media_ids).await?;
            println!("✅ Tweet posted successfully!");
            println!("🔗 https://x.com/i/status/{id}");
            Ok(())
        }
        Command::Reply { tweet, text, media, alt, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would reply to {id}: {text}");
                if !media.is_empty() {
                    println!("   with media: {}", media.join(", "));
                }
                return Ok(());
            }
            let media_ids = upload_all(&client, &media, &alt).await?;
            let new_id = client.post_reply(&text, &id, &media_ids).await?;
            println!("✅ Reply posted successfully!");
            println!("🔗 https://x.com/i/status/{new_id}");
            Ok(())
        }
        Command::Replies { tweet } => {
            let id = extract::extract_tweet_id(&tweet);
            let tweets = client.get_replies(&id).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No replies found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Thread { tweet } => {
            let id = extract::extract_tweet_id(&tweet);
            let tweets = client.get_thread(&id).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No thread tweets found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::User { handle } => {
            let profile = client.user(&handle).await?;
            output::print_profile(&profile, cli.json, cli.plain);
            Ok(())
        }
        Command::UserTweets { handle, count } => {
            let tweets = client.user_tweets(&handle, count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No tweets found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Home { count } => {
            let tweets = client.home(count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No home tweets found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Bookmarks { count } => {
            let tweets = client.bookmarks(count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No bookmarks found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::List { list, count } => {
            let tweets = client.list_tweets(&list, count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No list tweets found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Like { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run { println!("📝 [dry-run] would like {id}"); return Ok(()); }
            client.like(&id).await?;
            println!("❤️ liked {id}");
            Ok(())
        }
        Command::Unlike { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run { println!("📝 [dry-run] would unlike {id}"); return Ok(()); }
            client.unlike(&id).await?;
            println!("✅ unliked {id}");
            Ok(())
        }
        Command::Retweet { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run { println!("📝 [dry-run] would retweet {id}"); return Ok(()); }
            client.retweet(&id).await?;
            println!("🔁 retweeted {id}");
            Ok(())
        }
        Command::Unretweet { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run { println!("📝 [dry-run] would unretweet {id}"); return Ok(()); }
            client.unretweet(&id).await?;
            println!("✅ unretweeted {id}");
            Ok(())
        }
        Command::Bookmark { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run { println!("📝 [dry-run] would bookmark {id}"); return Ok(()); }
            client.bookmark(&id).await?;
            println!("🔖 bookmarked {id}");
            Ok(())
        }
        Command::Unbookmark { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run { println!("📝 [dry-run] would unbookmark {id}"); return Ok(()); }
            client.unbookmark(&id).await?;
            println!("✅ unbookmarked {id}");
            Ok(())
        }
        Command::Upload { file, alt } => {
            let id = client
                .upload_media(std::path::Path::new(&file), alt.as_deref())
                .await?;
            println!("{id}");
            Ok(())
        }
        Command::Follow { handle, dry_run } => {
            if dry_run {
                println!("📝 [dry-run] would follow @{}", handle.trim_start_matches('@'));
                return Ok(());
            }
            client.follow(&handle).await?;
            println!("✅ followed @{}", handle.trim_start_matches('@'));
            Ok(())
        }
        Command::Unfollow { handle, dry_run } => {
            if dry_run {
                println!("📝 [dry-run] would unfollow @{}", handle.trim_start_matches('@'));
                return Ok(());
            }
            client.unfollow(&handle).await?;
            println!("✅ unfollowed @{}", handle.trim_start_matches('@'));
            Ok(())
        }
        Command::UpdateQueryIds => {
            let ids = client
                .refresh_query_ids(&["TweetDetail", "SearchTimeline", "CreateTweet"])
                .await?;
            println!("refreshed {} query id(s):", ids.len());
            let mut pairs: Vec<_> = ids.iter().collect();
            pairs.sort();
            for (op, id) in pairs {
                println!("  {op}: {id}");
            }
            Ok(())
        }
    }
}

/// Upload each media file, pairing it with alt text by position, and return the
/// resulting media_ids in order.
async fn upload_all(client: &TwitterClient, media: &[String], alt: &[String]) -> Result<Vec<String>> {
    let mut ids = Vec::with_capacity(media.len());
    for (i, path) in media.iter().enumerate() {
        let alt_text = alt.get(i).map(String::as_str).filter(|s| !s.is_empty());
        let id = client.upload_media(std::path::Path::new(path), alt_text).await?;
        ids.push(id);
    }
    Ok(ids)
}

/// Persist fetched tweets to the local archive. Best-effort: a DB failure warns
/// but never fails the command or suppresses the output already printed.
fn archive(tweets: &[models::Tweet], no_db: bool) {
    if no_db || tweets.is_empty() {
        return;
    }
    let result = db::Db::open_default().and_then(|mut db| db.archive(tweets));
    if let Err(e) = result {
        eprintln!("⚠️ archive skipped: {e}");
    }
}

async fn whoami(client: &TwitterClient, source: &str, json: bool, plain: bool) -> Result<()> {
    let user = client.current_user().await?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "id": user.id,
                "username": user.username,
                "name": user.name,
                "source": source,
            })
        );
    } else if plain {
        println!("logged in as @{} ({})", user.username, user.name);
        println!("user id: {}", user.id);
        println!("credentials: {source}");
    } else {
        println!("🙋 Logged in as @{} ({})", user.username, user.name);
        println!("🪪 User ID: {}", user.id);
        println!("🔑 Credentials: {source}");
    }
    Ok(())
}

/// Credential status: which cookies resolved, from where, and whether they work.
async fn check(
    flag_auth: Option<String>,
    flag_ct0: Option<String>,
    timeout: u64,
    plain: bool,
) -> Result<()> {
    let ok = |e: &str| if plain { format!("[ok] {e}") } else { format!("✅ {e}") };
    let bad = |e: &str| if plain { format!("[missing] {e}") } else { format!("❌ {e}") };

    let creds = match resolve_credentials(flag_auth, flag_ct0) {
        Ok(creds) => creds,
        Err(e) => {
            println!("{}", bad("no credentials found"));
            return Err(e);
        }
    };

    println!("{}", ok(&format!("auth_token: {}…", head(&creds.auth_token))));
    println!("{}", ok(&format!("ct0: {}…", head(&creds.ct0))));
    println!("{} {}", if plain { "source:" } else { "📍 Source:" }, creds.source);

    let client = TwitterClient::new(
        creds.auth_token,
        creds.ct0,
        Duration::from_millis(timeout),
    )?;
    match client.current_user().await {
        Ok(user) => println!("{}", ok(&format!("valid — logged in as @{}", user.username))),
        Err(e) => println!(
            "{}",
            bad(&format!("credentials present but rejected by X: {e}"))
        ),
    }
    Ok(())
}

fn head(token: &str) -> String {
    token.chars().take(10).collect()
}

/// Precedence: CLI flags → env vars → cookie file → config file.
fn resolve_credentials(flag_auth: Option<String>, flag_ct0: Option<String>) -> Result<Credentials> {
    if let (Some(auth_token), Some(ct0)) = (nonempty(flag_auth), nonempty(flag_ct0)) {
        return Ok(Credentials { auth_token, ct0, source: "CLI flags".into() });
    }

    let auth_keys = ["KICAU_AUTH_TOKEN", "AUTH_TOKEN"];
    let ct0_keys = ["KICAU_CT0", "CT0"];
    if let (Some(auth_token), Some(ct0)) = (first_env(&auth_keys), first_env(&ct0_keys)) {
        return Ok(Credentials { auth_token, ct0, source: "environment variables".into() });
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let cookie_file = format!("{home}/.kicau/cookies.env");
    if let Some(creds) = from_env_file(&cookie_file) {
        return Ok(creds);
    }

    let config = format!("{home}/.config/kicau/config.json");
    if let Some(creds) = from_config_json(&config) {
        return Ok(creds);
    }

    Err(anyhow!(
        "missing credentials — provide --auth-token/--ct0, AUTH_TOKEN/CT0 env vars, {cookie_file}, or {config}"
    ))
}

fn from_env_file(path: &str) -> Option<Credentials> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut vars = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, val)) = line.split_once('=') else { continue };
        let val = val.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        if let Some(val) = nonempty(Some(val)) {
            vars.insert(key.trim().to_string(), val);
        }
    }
    // resolve by key priority, not line order
    let pick = |keys: &[&str]| keys.iter().find_map(|k| vars.get(*k).cloned());
    Some(Credentials {
        auth_token: pick(&["KICAU_AUTH_TOKEN", "AUTH_TOKEN"])?,
        ct0: pick(&["KICAU_CT0", "CT0"])?,
        source: path.to_string(),
    })
}

fn from_config_json(path: &str) -> Option<Credentials> {
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let auth_token = nonempty(v.get("authToken").and_then(|x| x.as_str()).map(str::to_string))?;
    let ct0 = nonempty(v.get("ct0").and_then(|x| x.as_str()).map(str::to_string))?;
    Some(Credentials { auth_token, ct0, source: path.to_string() })
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| nonempty(std::env::var(k).ok()))
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_cookie_env_file_by_key_priority() {
        let mut f = tempfile("parse");
        writeln!(f.0, "AUTH_TOKEN=abc123\nKICAU_CT0=kk\nCT0=\"def456\"\n# comment").unwrap();
        let creds = from_env_file(&f.1).expect("should parse");
        assert_eq!(creds.auth_token, "abc123");
        assert_eq!(creds.ct0, "kk"); // KICAU_CT0 wins over CT0; quotes would strip
    }

    #[test]
    fn missing_key_yields_none() {
        let mut f = tempfile("missing");
        writeln!(f.0, "AUTH_TOKEN=only").unwrap();
        assert!(from_env_file(&f.1).is_none());
    }

    fn tempfile(tag: &str) -> (std::fs::File, String) {
        let path = format!(
            "{}/kicau-test-{}-{}.env",
            std::env::temp_dir().display(),
            std::process::id(),
            tag
        );
        (std::fs::File::create(&path).unwrap(), path)
    }
}
