use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::models::{Author, Tweet};

// Normalized tweet archive. profiles is keyed by the stable X user id; handle is
// deliberately not unique because X recycles handles across ids over time.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS profiles (
    id text primary key,
    handle text not null,
    display_name text not null,
    bio text not null default '',
    followers_count integer not null default 0,
    following_count integer not null default 0,
    public_metrics_json text not null default '{}',
    avatar_hue integer not null default 0,
    avatar_url text,
    location text,
    url text,
    verified_type text,
    entities_json text not null default '{}',
    raw_json text not null default '{}',
    created_at text not null
);
CREATE TABLE IF NOT EXISTS tweets (
    id text primary key,
    author_profile_id text not null,
    text text not null,
    created_at text not null,
    is_replied integer not null default 0,
    reply_to_id text,
    like_count integer not null default 0,
    media_count integer not null default 0,
    entities_json text not null default '{}',
    media_json text not null default '[]',
    quoted_tweet_id text
);
CREATE INDEX IF NOT EXISTS idx_tweets_created ON tweets(created_at);
CREATE VIRTUAL TABLE IF NOT EXISTS tweets_fts USING fts5(tweet_id UNINDEXED, text);
CREATE TABLE IF NOT EXISTS accounts (
    id text primary key,
    handle text not null,
    created_at text not null
);
CREATE TABLE IF NOT EXISTS tweet_collections (
    account_id text not null,
    tweet_id text not null,
    kind text not null,
    collected_at text,
    source text not null default 'kicau',
    updated_at text not null,
    primary key (account_id, tweet_id, kind)
);
";

const SELECT: &str = "
SELECT t.id, t.text, t.created_at, t.reply_to_id, t.like_count,
       p.id, p.handle, p.display_name
FROM tweets t JOIN profiles p ON p.id = t.author_profile_id";

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open_default() -> Result<Db> {
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = PathBuf::from(home).join(".kicau");
        std::fs::create_dir_all(&dir)?;
        Self::open(dir.join("kicau.sqlite"))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        Self::init(Connection::open(path)?)
    }

    fn init(conn: Connection) -> Result<Db> {
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn })
    }

    /// Upsert every tweet's author and the tweet itself, keeping the FTS index in
    /// sync. Idempotent: re-archiving a tweet updates its row rather than duplicating.
    pub fn archive(&mut self, tweets: &[Tweet]) -> Result<()> {
        let now = now_secs().to_string();
        let tx = self.conn.transaction()?;
        for tweet in tweets {
            upsert_tweet(&tx, tweet, &now)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Archive tweets and record their membership in a named collection (e.g.
    /// "bookmarks", "likes") for the given account. Idempotent.
    pub fn archive_collection(&mut self, tweets: &[Tweet], account_id: &str, kind: &str) -> Result<usize> {
        let now = now_secs().to_string();
        let tx = self.conn.transaction()?;
        let mut saved = 0;
        for tweet in tweets {
            if !upsert_tweet(&tx, tweet, &now)? {
                continue;
            }
            tx.execute(
                "INSERT INTO tweet_collections(account_id, tweet_id, kind, collected_at, source, updated_at)
                 VALUES(?1, ?2, ?3, ?4, 'kicau', ?5)
                 ON CONFLICT(account_id, tweet_id, kind) DO UPDATE SET updated_at=excluded.updated_at",
                params![account_id, tweet.id, kind, now, now],
            )?;
            saved += 1;
        }
        tx.commit()?;
        Ok(saved)
    }

    /// Row counts and on-disk size of the local store.
    pub fn stats(&self) -> Result<Stats> {
        let count = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        let page_count: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(Stats {
            tweets: count("SELECT count(*) FROM tweets")?,
            profiles: count("SELECT count(*) FROM profiles")?,
            collections: count("SELECT count(*) FROM tweet_collections")?,
            bytes: page_count * page_size,
        })
    }

    /// Full-text search over archived tweets, newest first. Terms are combined
    /// with AND (every word must appear, in any order).
    pub fn find(&self, query: &str, limit: u32) -> Result<Vec<Tweet>> {
        let phrase = fts_match(query);
        if phrase.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "{SELECT} JOIN tweets_fts f ON f.tweet_id = t.id
             WHERE tweets_fts MATCH ?1 ORDER BY t.created_at DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![phrase, limit], row_to_tweet)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The most recently created archived tweets, newest first.
    pub fn recent(&self, limit: u32) -> Result<Vec<Tweet>> {
        let sql = format!("{SELECT} ORDER BY t.created_at DESC LIMIT ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], row_to_tweet)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

pub struct Stats {
    pub tweets: i64,
    pub profiles: i64,
    pub collections: i64,
    pub bytes: i64,
}

/// Upsert one tweet + its author + FTS entry. Returns false (skipped) when the
/// tweet or author id is missing — those can't be keyed stably.
fn upsert_tweet(tx: &rusqlite::Transaction, tweet: &Tweet, now: &str) -> rusqlite::Result<bool> {
    if tweet.id.is_empty() || tweet.author.id.is_empty() {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO profiles(id, handle, display_name, created_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET handle=excluded.handle, display_name=excluded.display_name",
        params![tweet.author.id, tweet.author.username, tweet.author.name, now],
    )?;
    tx.execute(
        "INSERT INTO tweets(id, author_profile_id, text, created_at, reply_to_id, like_count)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET text=excluded.text, created_at=excluded.created_at,
             reply_to_id=excluded.reply_to_id, like_count=excluded.like_count",
        params![
            tweet.id,
            tweet.author.id,
            tweet.text,
            tweet.created_at.as_deref().unwrap_or_default(),
            tweet.in_reply_to_status_id,
            tweet.like_count.unwrap_or(0),
        ],
    )?;
    tx.execute("DELETE FROM tweets_fts WHERE tweet_id = ?1", params![tweet.id])?;
    tx.execute("INSERT INTO tweets_fts(tweet_id, text) VALUES(?1, ?2)", params![tweet.id, tweet.text])?;
    Ok(true)
}

fn row_to_tweet(row: &rusqlite::Row) -> rusqlite::Result<Tweet> {
    Ok(Tweet {
        id: row.get(0)?,
        text: row.get(1)?,
        created_at: row.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
        in_reply_to_status_id: row.get(3)?,
        like_count: row.get::<_, Option<i64>>(4)?.map(|n| n as u64),
        author: Author {
            id: row.get(5)?,
            username: row.get(6)?,
            name: row.get(7)?,
        },
        // the tweets table stores neither of these, so archived reads drop them.
        retweet_count: None,
        reply_count: None,
        conversation_id: None,
    })
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Build an FTS5 MATCH expression that ANDs every whitespace-separated term.
/// Each term is quoted as a single-token phrase, which both keeps arbitrary user
/// input from being an FTS5 syntax error and avoids forcing whole-query adjacency.
fn fts_match(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // created_at is ISO 8601 here because the parser normalizes it before archiving.
    fn tweet(id: &str, uid: &str, handle: &str, text: &str, iso_date: &str) -> Tweet {
        Tweet {
            id: id.into(),
            text: text.into(),
            author: Author { id: uid.into(), username: handle.into(), name: handle.into() },
            created_at: Some(iso_date.into()),
            reply_count: Some(1),
            retweet_count: Some(2),
            like_count: Some(3),
            conversation_id: Some(id.into()),
            in_reply_to_status_id: None,
        }
    }

    fn db() -> Db {
        Db::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn recent_orders_newest_first() {
        let mut db = db();
        db.archive(&[
            tweet("1", "u1", "alice", "older post", "2026-07-06T10:00:00.000Z"),
            tweet("2", "u2", "bob", "newer post", "2026-07-15T10:00:00.000Z"),
        ])
        .unwrap();
        let got = db.recent(10).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "2", "newest first");
        assert_eq!(got[1].id, "1");
    }

    #[test]
    fn find_matches_text_and_rehydrates_author() {
        let mut db = db();
        db.archive(&[tweet("9", "u9", "carol", "designing agent loops", "2026-07-06T10:00:00.000Z")])
            .unwrap();
        let hits = db.find("loops", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "9");
        assert_eq!(hits[0].author.username, "carol");
        assert_eq!(hits[0].like_count, Some(3));
        assert!(db.find("nonexistent", 10).unwrap().is_empty());
    }

    #[test]
    fn find_ands_terms_regardless_of_order() {
        let mut db = db();
        db.archive(&[tweet("9", "u9", "carol", "designing agent loops carefully", "2026-07-06T10:00:00.000Z")])
            .unwrap();
        // Words present but reordered / non-adjacent must still match.
        assert_eq!(db.find("loops designing", 10).unwrap().len(), 1);
        assert_eq!(db.find("agent carefully", 10).unwrap().len(), 1);
        // A word not present excludes the tweet.
        assert_eq!(db.find("loops rockets", 10).unwrap().len(), 0);
    }

    #[test]
    fn archive_is_idempotent() {
        let mut db = db();
        let t = tweet("5", "u5", "dave", "first", "2026-07-06T10:00:00.000Z");
        db.archive(&[t.clone()]).unwrap();
        let mut updated = t;
        updated.text = "edited".into();
        db.archive(&[updated]).unwrap();
        let all = db.recent(10).unwrap();
        assert_eq!(all.len(), 1, "same id upserts, not duplicates");
        assert_eq!(all[0].text, "edited");
        assert_eq!(db.find("edited", 10).unwrap().len(), 1);
        assert_eq!(db.find("first", 10).unwrap().len(), 0, "fts reindexed on update");
    }

    #[test]
    fn archive_collection_records_membership_idempotently() {
        let mut db = db();
        let t = tweet("3", "u3", "carol", "saved", "2026-07-06T10:00:00.000Z");
        assert_eq!(db.archive_collection(&[t.clone()], "acct1", "bookmarks").unwrap(), 1);
        // re-syncing the same tweet doesn't duplicate the collection row
        db.archive_collection(&[t], "acct1", "bookmarks").unwrap();
        let s = db.stats().unwrap();
        assert_eq!(s.tweets, 1);
        assert_eq!(s.collections, 1);
        assert_eq!(db.find("saved", 10).unwrap().len(), 1, "collection tweets are also searchable");
    }

    #[test]
    fn archive_skips_records_without_stable_keys() {
        let mut db = db();
        let mut no_author = tweet("8", "", "ghost", "no author id", "2026-07-06T10:00:00.000Z");
        no_author.author.id = String::new();
        let no_id = tweet("", "u1", "alice", "no tweet id", "2026-07-06T10:00:00.000Z");
        db.archive(&[no_author, no_id]).unwrap();
        assert_eq!(db.recent(10).unwrap().len(), 0, "unkeyable records are skipped, not fragmented");
    }

    #[test]
    fn find_tolerates_quotes_in_query() {
        let mut db = db();
        db.archive(&[tweet("7", "u7", "erin", "she said hi today", "2026-07-06T10:00:00.000Z")])
            .unwrap();
        // Must not raise an FTS5 syntax error.
        let _ = db.find("said \"hi\"", 10).unwrap();
    }
}
