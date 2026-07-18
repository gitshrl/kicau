mod client;
mod config;
mod db;
mod extract;
mod mcp;
mod models;
mod output;
mod parse;
mod query;
mod transaction;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Result, anyhow};
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
    /// Create config directories and seed defaults
    Init,
    /// Show configuration paths and where credentials resolve from
    Config,
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
    /// List DM conversations
    Dms,
    /// Show messages in a DM conversation (by id or @handle)
    Dm {
        /// Conversation id or the other participant's @handle
        conversation: String,
    },
    /// Show local database stats
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Export or restore the local store as git-friendly text
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Import a downloaded X data export (a directory containing data/)
    Import {
        /// Path to the unzipped export root
        dir: String,
    },
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
    /// Show tweets: yours by default, or another user's with a handle
    Tweets {
        /// @handle (with or without @); omit for your own tweets
        handle: Option<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show your chronological home timeline
    Home {
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Sync your bookmarks (incremental) and show the newest; archives to SQLite
    Bookmarks {
        /// How many to display from the archive
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
        /// Re-fetch every bookmark instead of stopping at ones already archived
        #[arg(long)]
        all: bool,
    },
    /// Show a list's tweets by list id
    List {
        /// List id
        list: String,
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Delete one of your own tweets
    Delete {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Like a tweet
    Like {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a like from a tweet
    Unlike {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Retweet a tweet
    Retweet {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a retweet
    Unretweet {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Bookmark a tweet
    Bookmark {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a bookmark
    Unbookmark {
        tweet: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Upload a media file and print its media_id
    Upload {
        /// Path to an image/gif/video
        file: String,
        /// Alt text (images only)
        #[arg(long)]
        alt: Option<String>,
    },
    /// Follow a user
    Follow {
        handle: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Unfollow a user
    Unfollow {
        handle: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Block or unblock a user (action: add | remove)
    Blocks {
        action: String,
        handle: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Mute or unmute a user (action: add | remove)
    Mutes {
        action: String,
        handle: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Dancing cat
    Mania,
    /// Serve the archive to agents over MCP (stdio)
    Mcp,
}

#[derive(Subcommand)]
enum DbAction {
    /// Row counts and on-disk size
    Stats,
}

#[derive(Subcommand)]
enum BackupAction {
    /// Dump the store to <dir> as JSONL
    Export { dir: String },
    /// Restore the store from <dir>
    Import { dir: String },
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("❌ {e}");
        std::process::exit(1);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "a dispatch table: one flat arm per command, long by nature rather than complex"
)]
async fn run() -> Result<()> {
    let cli = Cli::parse();

    // check must report on partial/missing credentials, so it resolves its own.
    if matches!(cli.command, Command::Check) {
        return check(cli.auth_token, cli.ct0, cli.timeout, cli.plain).await;
    }
    // These commands need no credentials.
    match &cli.command {
        Command::Init => return init_command(),
        Command::Config => {
            config_command(cli.json);
            return Ok(());
        }
        Command::Mania => return mania_command(),
        Command::Mcp => {
            // The archive tools need no credentials; read_tweet does. Resolve
            // them if present so read_tweet can work, and serve regardless: an
            // agent asking what you bookmarked should not need a login to answer.
            let client = config::resolve_credentials(cli.auth_token.clone(), cli.ct0.clone())
                .ok()
                .and_then(|creds| {
                    TwitterClient::new(
                        creds.auth_token,
                        creds.ct0,
                        Duration::from_millis(cli.timeout),
                    )
                    .ok()
                });
            return mcp::serve(client).await;
        }
        Command::Find { query, count } => {
            let db = db::Db::open_default()?;
            let tweets = db.find(query, *count)?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No matching tweets archived.");
            return Ok(());
        }
        Command::Log { count } => {
            let tweets = db::Db::open_default()?.recent(*count)?;
            output::print_tweets(&tweets, cli.json, cli.plain, "Nothing archived yet.");
            return Ok(());
        }
        Command::Import { dir } => {
            let root = std::path::Path::new(dir);
            let account = std::fs::read_to_string(root.join("data/account.js"))
                .map_err(|e| anyhow!("cannot read data/account.js: {e}"))?;
            let author = parse::parse_archive_account(&account)
                .ok_or_else(|| anyhow!("could not parse account.js"))?;
            let tweets_js = std::fs::read_to_string(root.join("data/tweets.js"))
                .map_err(|e| anyhow!("cannot read data/tweets.js: {e}"))?;
            let tweets = parse::parse_archive_tweets(&tweets_js, &author);
            let n = db::Db::open_default()?.archive_collection(&tweets, &author.id, "tweets")?;
            println!("✅ imported {n} tweets from @{}'s archive", author.username);
            return Ok(());
        }
        Command::Backup { action } => {
            let mut db = db::Db::open_default()?;
            match action {
                BackupAction::Export { dir } => {
                    let n = db.export_backup(std::path::Path::new(dir))?;
                    println!("✅ exported {n} tables to {dir}");
                }
                BackupAction::Import { dir } => {
                    let n = db.import_backup(std::path::Path::new(dir))?;
                    println!("✅ imported {n} rows from {dir}");
                }
            }
            return Ok(());
        }
        Command::Db {
            action: DbAction::Stats,
        } => {
            let s = db::Db::open_default()?.stats()?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({ "tweets": s.tweets, "profiles": s.profiles, "collections": s.collections, "edges": s.edges, "dms": s.dms, "bytes": s.bytes })
                );
            } else {
                println!("tweets:      {}", s.tweets);
                println!("profiles:    {}", s.profiles);
                println!("collections: {}", s.collections);
                println!("edges:       {}", s.edges);
                println!("dms:         {}", s.dms);
                println!("size:        {} KiB", s.bytes / 1024);
            }
            return Ok(());
        }
        _ => {}
    }

    let creds = config::resolve_credentials(cli.auth_token.clone(), cli.ct0.clone())?;
    let client = TwitterClient::new(
        creds.auth_token,
        creds.ct0,
        Duration::from_millis(cli.timeout),
    )?;

    match cli.command {
        Command::Check
        | Command::Init
        | Command::Config
        | Command::Mania
        | Command::Mcp
        | Command::Find { .. }
        | Command::Log { .. }
        | Command::Db { .. }
        | Command::Backup { .. }
        | Command::Import { .. } => unreachable!("handled above"),
        Command::Graph {
            direction,
            handle,
            count,
        } => {
            let profiles = match direction.as_str() {
                "following" => client.following(&handle, count).await?,
                "followers" => client.followers(&handle, count).await?,
                other => {
                    return Err(anyhow!(
                        "unknown direction '{other}' (use: following | followers)"
                    ));
                }
            };
            output::print_profiles(&profiles, cli.json, "No accounts found.");
            if !cli.no_db {
                let account = client.user(&handle).await?;
                let n = db::Db::open_default()?.save_follow_edges(
                    &account.id,
                    &direction,
                    &profiles,
                )?;
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
        Command::Dms => {
            let (convs, msgs) = client.dm_inbox().await?;
            output::print_dm_conversations(&convs, cli.json);
            if !cli.no_db {
                db::Db::open_default()?.save_dms(&convs, &msgs)?;
            }
            Ok(())
        }
        Command::Dm { conversation } => {
            let (convs, msgs) = client.dm_inbox().await?;
            // accept a conversation id or the other participant's @handle
            let needle = format!("@{}", conversation.trim_start_matches('@'));
            let target = convs
                .iter()
                .find(|c| c.id == conversation || c.title == needle || c.title.contains(&needle))
                .map(|c| c.id.clone())
                .unwrap_or(conversation);
            let thread: Vec<_> = msgs
                .iter()
                .filter(|m| m.conversation_id == target)
                .cloned()
                .collect();
            output::print_dm_messages(&thread, cli.json);
            if !cli.no_db {
                db::Db::open_default()?.save_dms(&convs, &msgs)?;
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
        Command::Tweet {
            text,
            media,
            alt,
            dry_run,
        } => {
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
        Command::Reply {
            tweet,
            text,
            media,
            alt,
            dry_run,
        } => {
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
        Command::Tweets { handle, count } => {
            let handle = match handle {
                Some(handle) => handle,
                None => client.current_user().await?.username,
            };
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
        Command::Bookmarks { count, all } => {
            // --no-db: a live view, no archive, no incremental (the stop needs the
            // archive to compare against). Fetch `count` and show.
            if cli.no_db {
                let tweets = client.bookmarks(count, &HashSet::new()).await?;
                output::print_tweets(&tweets, cli.json, cli.plain, "No bookmarks found.");
                return Ok(());
            }
            let user = client.current_user().await?;
            let mut db = db::Db::open_default()?;
            // Known bookmarks stop the fetch early; an empty set (first sync, or
            // --all) fetches the whole timeline.
            let known = if all {
                std::collections::HashSet::new()
            } else {
                db.bookmark_ids(&user.id)?
            };
            let fetched = client.bookmarks(u32::MAX, &known).await?;
            // The collection records everything you ever bookmarked: unbookmark
            // something in X and the archive keeps it.
            db.archive_collection(&fetched, &user.id, "bookmarks")?;
            // Show the newest from the archive, so a caught-up sync that fetched
            // nothing new still displays your recent bookmarks.
            let newest = db.collection("bookmarks", count)?;
            output::print_tweets(&newest, cli.json, cli.plain, "No bookmarks archived yet.");
            Ok(())
        }
        Command::List { list, count } => {
            let tweets = client.list_tweets(&list, count).await?;
            output::print_tweets(&tweets, cli.json, cli.plain, "No list tweets found.");
            archive(&tweets, cli.no_db);
            Ok(())
        }
        Command::Delete { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would delete {id}");
                return Ok(());
            }
            client.delete_tweet(&id).await?;
            println!("🗑️ deleted {id}");
            Ok(())
        }
        Command::Like { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would like {id}");
                return Ok(());
            }
            client.like(&id).await?;
            println!("❤️ liked {id}");
            Ok(())
        }
        Command::Unlike { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would unlike {id}");
                return Ok(());
            }
            client.unlike(&id).await?;
            println!("✅ unliked {id}");
            Ok(())
        }
        Command::Retweet { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would retweet {id}");
                return Ok(());
            }
            client.retweet(&id).await?;
            println!("🔁 retweeted {id}");
            Ok(())
        }
        Command::Unretweet { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would unretweet {id}");
                return Ok(());
            }
            client.unretweet(&id).await?;
            println!("✅ unretweeted {id}");
            Ok(())
        }
        Command::Bookmark { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would bookmark {id}");
                return Ok(());
            }
            client.bookmark(&id).await?;
            println!("🔖 bookmarked {id}");
            Ok(())
        }
        Command::Unbookmark { tweet, dry_run } => {
            let id = extract::extract_tweet_id(&tweet);
            if dry_run {
                println!("📝 [dry-run] would unbookmark {id}");
                return Ok(());
            }
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
                println!(
                    "📝 [dry-run] would follow @{}",
                    handle.trim_start_matches('@')
                );
                return Ok(());
            }
            client.follow(&handle).await?;
            println!("✅ followed @{}", handle.trim_start_matches('@'));
            Ok(())
        }
        Command::Unfollow { handle, dry_run } => {
            if dry_run {
                println!(
                    "📝 [dry-run] would unfollow @{}",
                    handle.trim_start_matches('@')
                );
                return Ok(());
            }
            client.unfollow(&handle).await?;
            println!("✅ unfollowed @{}", handle.trim_start_matches('@'));
            Ok(())
        }
        Command::Blocks {
            action,
            handle,
            dry_run,
        } => moderate(&client, "block", &action, &handle, dry_run).await,
        Command::Mutes {
            action,
            handle,
            dry_run,
        } => moderate(&client, "mute", &action, &handle, dry_run).await,
    }
}

/// block/unblock or mute/unmute a user by add|remove action, with dry-run.
async fn moderate(
    client: &TwitterClient,
    kind: &str,
    action: &str,
    handle: &str,
    dry_run: bool,
) -> Result<()> {
    let h = handle.trim_start_matches('@');
    let add = match action {
        "add" => true,
        "remove" => false,
        other => return Err(anyhow!("unknown action '{other}' (use: add | remove)")),
    };
    let (verb, past) = match (kind, add) {
        ("block", true) => ("block", "blocked"),
        ("block", false) => ("unblock", "unblocked"),
        ("mute", true) => ("mute", "muted"),
        _ => ("unmute", "unmuted"),
    };
    if dry_run {
        println!("📝 [dry-run] would {verb} @{h}");
        return Ok(());
    }
    match (kind, add) {
        ("block", true) => client.block(handle).await?,
        ("block", false) => client.unblock(handle).await?,
        ("mute", true) => client.mute(handle).await?,
        _ => client.unmute(handle).await?,
    }
    println!("✅ {past} @{h}");
    Ok(())
}

/// Upload each media file, pairing it with alt text by position, and return the
/// resulting `media_ids` in order.
async fn upload_all(
    client: &TwitterClient,
    media: &[String],
    alt: &[String],
) -> Result<Vec<String>> {
    let mut ids = Vec::with_capacity(media.len());
    for (i, path) in media.iter().enumerate() {
        let alt_text = alt.get(i).map(String::as_str).filter(|s| !s.is_empty());
        let id = client
            .upload_media(std::path::Path::new(path), alt_text)
            .await?;
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
    let ok = |e: &str| {
        if plain {
            format!("[ok] {e}")
        } else {
            format!("✅ {e}")
        }
    };
    let bad = |e: &str| {
        if plain {
            format!("[missing] {e}")
        } else {
            format!("❌ {e}")
        }
    };

    let creds = match config::resolve_credentials(flag_auth, flag_ct0) {
        Ok(creds) => creds,
        Err(e) => {
            println!("{}", bad("no credentials found"));
            return Err(e);
        }
    };

    println!(
        "{}",
        ok(&format!("auth_token: {}…", head(&creds.auth_token)))
    );
    println!("{}", ok(&format!("ct0: {}…", head(&creds.ct0))));
    println!(
        "{} {}",
        if plain { "source:" } else { "📍 Source:" },
        creds.source
    );

    let client = TwitterClient::new(creds.auth_token, creds.ct0, Duration::from_millis(timeout))?;
    match client.current_user().await {
        Ok(user) => println!(
            "{}",
            ok(&format!("valid — logged in as @{}", user.username))
        ),
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

/// A fresh `config.toml`: empty credentials plus the full query-id table (edit an
/// id if X rotates it and a call starts 404ing).
fn config_template(auth_token: &str, ct0: &str) -> String {
    let mut s = format!(
        "# kicau config\n\n[credentials]\nauth_token = \"{auth_token}\"\nct0 = \"{ct0}\"\n\n[query_ids]\n"
    );
    for (op, id) in config::QUERY_IDS {
        let _ = writeln!(s, "{op} = \"{id}\"");
    }
    s
}

/// Where the two cookies live in a browser.
fn print_cookie_guidance() {
    println!("kicau signs in with two of your own x.com session cookies.\n");
    println!("  1. Sign in to x.com in your browser.");
    println!("  2. Open developer tools (F12), then:");
    println!("       Chrome / Edge   Application → Storage → Cookies → https://x.com");
    println!("       Firefox         Storage → Cookies → https://x.com");
    println!("       Safari          Develop → Show Web Inspector → Storage → Cookies");
    println!("  3. Copy the value of the auth_token cookie, then the ct0 cookie.\n");
    println!("Together they grant full access to your account — treat them like a");
    println!("password. Press Enter to skip and fill the file in by hand.\n");
}

fn stty(arg: &str) -> bool {
    std::process::Command::new("stty")
        .arg(arg)
        .status()
        .is_ok_and(|s| s.success())
}

/// Read both cookies with terminal echo off, so a pasted secret does not stay on
/// screen or in scrollback.
fn prompt_credentials() -> Result<(String, String)> {
    use std::io::{BufRead, Write};

    let hidden = stty("-echo");
    if !hidden {
        println!("Terminal echo could not be turned off — pasted values will be visible.\n");
    }
    let read = |label: &str| -> Result<String> {
        print!("{label}: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if hidden {
            println!();
        }
        Ok(line.trim().to_string())
    };

    let both = (|| Ok((read("auth_token")?, read("ct0")?)))();
    if hidden {
        stty("echo");
    }
    both
}

/// Cookie values are opaque tokens. Anything that would break out of a TOML
/// string means a bad paste, and a malformed config only surfaces later as a
/// confusing "missing credentials".
fn check_cookie(label: &str, value: &str) -> Result<()> {
    if value.contains(['"', '\\']) || value.chars().any(char::is_control) {
        return Err(anyhow!(
            "{label} contains unexpected characters — copy just the cookie value"
        ));
    }
    Ok(())
}

/// Create `~/.kicau` and drop a `config.toml`. Idempotent — never overwrites an
/// existing config.
fn init_command() -> Result<()> {
    use std::io::IsTerminal;

    let state = config::state_dir();
    std::fs::create_dir_all(&state)?;
    let config_file = config::config_toml_path();

    if config_file.exists() {
        println!("✅ kicau initialized");
        println!("   state:  {}", state.display());
        println!("   config: {} (already present)", config_file.display());
        return Ok(());
    }

    // Only ask when someone is there to answer; piped or scripted runs get the
    // blank template instead of a hung prompt.
    let (auth_token, ct0) = if std::io::stdin().is_terminal() {
        print_cookie_guidance();
        prompt_credentials()?
    } else {
        (String::new(), String::new())
    };
    check_cookie("auth_token", &auth_token)?;
    check_cookie("ct0", &ct0)?;

    std::fs::write(&config_file, config_template(&auth_token, &ct0))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_file, std::fs::Permissions::from_mode(0o600));
    }

    println!("✅ kicau initialized");
    println!("   state:  {}", state.display());
    println!("   config: {}", config_file.display());
    if auth_token.is_empty() || ct0.is_empty() {
        println!("\nAdd auth_token and ct0 to that file, then run: kicau whoami");
    } else {
        println!("\nRun kicau whoami to check the cookies work.");
    }
    Ok(())
}

/// Dancing cat. That is the whole feature.
///
/// Frames are rendered from the source video at build time and embedded, so this
/// stays a single binary with nothing to install. Chars are ~2:1, so the square
/// video is half as many rows as columns.
fn mania_command() -> Result<()> {
    use std::io::{IsTerminal, Write};

    const FRAMES: &str = include_str!("mania.txt");
    const FPS_DELAY: Duration = Duration::from_millis(83); // 12fps, the rate they were rendered at
    const LOOPS: usize = 3;

    let frames: Vec<&str> = FRAMES.split('\u{c}').collect();
    let mut out = std::io::stdout().lock();

    // Piped or redirected: one still frame, no escape codes to garble the pipe.
    if !out.is_terminal() {
        writeln!(out, "{}", frames[0])?;
        return Ok(());
    }

    write!(out, "\x1b[2J")?;
    for frame in frames.iter().cycle().take(frames.len() * LOOPS) {
        write!(out, "\x1b[H")?;
        for line in frame.lines() {
            // Erase to end of line: frames are ragged, so a shorter line would
            // otherwise leave the previous frame's tail on screen.
            writeln!(out, "{line}\x1b[K")?;
        }
        out.flush()?;
        std::thread::sleep(FPS_DELAY);
    }
    writeln!(out)?;
    Ok(())
}

/// Show where things live and where credentials resolve from./// Show where things live and where credentials resolve from.
fn config_command(json: bool) {
    let state = config::state_dir();
    let config_file = config::config_toml_path();
    let db = state.join("kicau.sqlite");
    let source = config::resolve_credentials(None, None)
        .map_or_else(|_| "not configured".to_string(), |c| c.source);

    if json {
        println!(
            "{}",
            serde_json::json!({
                "credentials": source,
                "config": config_file.display().to_string(),
                "database": db.display().to_string(),
            })
        );
        return;
    }
    let row = |label: &str, path: &std::path::Path| {
        let mark = if path.exists() { "✓" } else { "–" };
        println!("{label:14} {} {mark}", path.display());
    };
    println!("credentials:   {source}");
    row("config:", &config_file);
    row("database:", &db);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_embeds_cookies_and_parses_back() {
        let toml = config_template("AUTH", "CT0");
        assert!(toml.contains("auth_token = \"AUTH\""));
        assert!(toml.contains("ct0 = \"CT0\""));
        let parsed: toml::Value = toml::from_str(&toml).unwrap();
        assert_eq!(parsed["credentials"]["ct0"].as_str(), Some("CT0"));
        assert_eq!(
            parsed["query_ids"]["TweetDetail"].as_str(),
            config::QUERY_IDS[0].1.into()
        );
    }

    #[test]
    fn skipped_prompt_leaves_a_fillable_template() {
        let parsed: toml::Value = toml::from_str(&config_template("", "")).unwrap();
        assert_eq!(parsed["credentials"]["auth_token"].as_str(), Some(""));
    }

    #[test]
    fn rejects_pastes_that_would_break_the_toml() {
        assert!(check_cookie("ct0", "a1b2c3d4e5").is_ok());
        assert!(check_cookie("ct0", "ab\"; evil = \"").is_err());
        assert!(check_cookie("ct0", "ab\\c").is_err());
        assert!(check_cookie("ct0", "ab\ncd").is_err());
    }
}
