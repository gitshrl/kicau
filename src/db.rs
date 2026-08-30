use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::models::{Author, DmConversation, DmMessage, Profile, Tweet};

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
CREATE TABLE IF NOT EXISTS follow_edges (
    account_id text not null,
    direction text not null,
    profile_id text not null,
    handle text not null,
    first_seen_at text not null,
    updated_at text not null,
    primary key (account_id, direction, profile_id)
);
CREATE TABLE IF NOT EXISTS profile_snapshots (
    profile_id text not null,
    snapshot_hash text not null,
    observed_at text not null,
    handle text not null,
    display_name text not null,
    bio text not null,
    followers_count integer not null default 0,
    following_count integer not null default 0,
    location text,
    primary key (profile_id, snapshot_hash)
);
CREATE TABLE IF NOT EXISTS dm_conversations (
    id text primary key,
    kind text not null,
    title text not null,
    updated_at text not null
);
CREATE TABLE IF NOT EXISTS dm_messages (
    id text primary key,
    conversation_id text not null,
    sender_id text not null,
    sender_handle text not null,
    text text not null,
    created_at text not null
);
";

/// Base tables included in backup export/import (FTS is derived and rebuilt).
const BACKUP_TABLES: &[&str] = &[
    "accounts",
    "profiles",
    "tweets",
    "tweet_collections",
    "follow_edges",
    "profile_snapshots",
    "dm_conversations",
    "dm_messages",
];

const SELECT: &str = "
SELECT t.id, t.text, t.created_at, t.reply_to_id, t.like_count,
       p.id, p.handle, p.display_name, t.media_json
FROM tweets t JOIN profiles p ON p.id = t.author_profile_id";

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open_default() -> Result<Db> {
        let dir = crate::config::state_dir();
        std::fs::create_dir_all(&dir)?;
        Self::open(dir.join("kicau.sqlite"))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        Self::init(Connection::open(path)?)
    }

    fn init(conn: Connection) -> Result<Db> {
        // Several kicau commands (home, bookmarks, tweets) run concurrently from
        // cron. SQLite's default `delete` journal takes a database-wide write lock,
        // so a long `home -n 500` archive starves every other process and they die
        // with "database is locked". WAL lets readers and writers proceed together;
        // the longer busy_timeout absorbs the remaining writer-vs-writer overlap.
        //
        // journal_mode persists in the database file; busy_timeout is per-connection
        // and must be set on every open. In-memory databases (tests) ignore WAL and
        // report "memory", which is fine — hence query_row, not execute.
        let _: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .unwrap_or_else(|_| "unknown".to_string());
        conn.busy_timeout(std::time::Duration::from_secs(30))?;

        // profile_snapshots switched from a time key to a content-hash key; drop
        // the old regenerable table so the new schema takes effect.
        let old_snapshots: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type='table' AND name='profile_snapshots' AND sql NOT LIKE '%snapshot_hash%'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .is_ok_and(|n| n > 0);
        if old_snapshots {
            conn.execute("DROP TABLE profile_snapshots", [])?;
        }
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
    pub fn archive_collection(
        &mut self,
        tweets: &[Tweet],
        account_id: &str,
        kind: &str,
    ) -> Result<usize> {
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

    /// Record a follow-graph snapshot: upsert the profiles and their edges.
    pub fn save_follow_edges(
        &mut self,
        account_id: &str,
        direction: &str,
        profiles: &[Profile],
    ) -> Result<usize> {
        let now = now_secs().to_string();
        let tx = self.conn.transaction()?;
        let mut saved = 0;
        for p in profiles {
            if p.id.is_empty() {
                continue;
            }
            upsert_profile(&tx, p, &now)?;
            tx.execute(
                "INSERT INTO follow_edges(account_id, direction, profile_id, handle, first_seen_at, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(account_id, direction, profile_id) DO UPDATE SET handle=excluded.handle, updated_at=excluded.updated_at",
                params![account_id, direction, p.id, p.handle, now],
            )?;
            saved += 1;
        }
        tx.commit()?;
        Ok(saved)
    }

    /// Upsert a profile and append a point-in-time snapshot of its metrics.
    pub fn save_profile(&mut self, p: &Profile) -> Result<()> {
        let now = now_secs().to_string();
        // Snapshot identity is its content: re-saving an unchanged profile is a
        // no-op, and a changed one records a new snapshot — regardless of timing.
        let hash = snapshot_hash(p);
        let tx = self.conn.transaction()?;
        upsert_profile(&tx, p, &now)?;
        tx.execute(
            "INSERT INTO profile_snapshots(profile_id, snapshot_hash, observed_at, handle, display_name, bio, followers_count, following_count, location)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(profile_id, snapshot_hash) DO NOTHING",
            params![p.id, hash, now, p.handle, p.name, p.bio, p.followers, p.following, p.location],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Persist DM conversations and messages (idempotent upserts).
    pub fn save_dms(
        &mut self,
        conversations: &[DmConversation],
        messages: &[DmMessage],
    ) -> Result<()> {
        let now = now_secs().to_string();
        let tx = self.conn.transaction()?;
        for c in conversations {
            tx.execute(
                "INSERT INTO dm_conversations(id, kind, title, updated_at) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, title=excluded.title, updated_at=excluded.updated_at",
                params![c.id, c.kind, c.title, now],
            )?;
        }
        for m in messages {
            if m.id.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT INTO dm_messages(id, conversation_id, sender_id, sender_handle, text, created_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO NOTHING",
                params![m.id, m.conversation_id, m.sender_id, m.sender_handle, m.text, m.created_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Dump every base table to `<dir>/<table>.jsonl` (git-friendly: one JSON
    /// object per line, ordered by primary key). Returns the number of tables.
    pub fn export_backup(&self, dir: &Path) -> Result<usize> {
        std::fs::create_dir_all(dir)?;
        for table in BACKUP_TABLES {
            std::fs::write(dir.join(format!("{table}.jsonl")), self.dump_table(table)?)?;
        }
        Ok(BACKUP_TABLES.len())
    }

    /// Restore base tables from a backup directory (INSERT OR REPLACE), then
    /// rebuild the FTS index. Returns the number of rows loaded.
    pub fn import_backup(&mut self, dir: &Path) -> Result<usize> {
        let mut total = 0;
        for table in BACKUP_TABLES {
            if let Ok(content) = std::fs::read_to_string(dir.join(format!("{table}.jsonl"))) {
                total += self.load_table(table, &content)?;
            }
        }
        self.conn.execute("DELETE FROM tweets_fts", [])?;
        self.conn.execute(
            "INSERT INTO tweets_fts(tweet_id, text) SELECT id, text FROM tweets",
            [],
        )?;
        Ok(total)
    }

    fn dump_table(&self, table: &str) -> Result<String> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY 1"))?;
        let cols: Vec<String> = (0..stmt.column_count())
            .map(|i| stmt.column_name(i).unwrap_or("").to_string())
            .collect();
        let mut out = String::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let mut obj = serde_json::Map::new();
            for (i, col) in cols.iter().enumerate() {
                obj.insert(
                    col.clone(),
                    val_to_json(row.get::<_, rusqlite::types::Value>(i)?),
                );
            }
            out.push_str(&serde_json::Value::Object(obj).to_string());
            out.push('\n');
        }
        Ok(out)
    }

    fn load_table(&mut self, table: &str, jsonl: &str) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
            let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(line)?;
            let cols: Vec<&String> = obj.keys().collect();
            let col_list = cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = (1..=cols.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let vals: Vec<rusqlite::types::Value> =
                cols.iter().map(|c| json_to_val(&obj[*c])).collect();
            tx.execute(
                &format!("INSERT OR REPLACE INTO \"{table}\"({col_list}) VALUES({placeholders})"),
                rusqlite::params_from_iter(vals),
            )?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
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
            edges: count("SELECT count(*) FROM follow_edges")?,
            dms: count("SELECT count(*) FROM dm_messages")?,
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

    /// The ids already recorded as bookmarks for an account.
    ///
    /// Keyed on collection membership, not on the `tweets` table: a tweet the
    /// archive holds from some other timeline is not a bookmark until it is one
    /// here. The incremental fetch uses this to know where it can stop.
    pub fn bookmark_ids(&self, account_id: &str) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT tweet_id FROM tweet_collections WHERE account_id = ?1 AND kind = 'bookmarks'",
        )?;
        let rows = stmt.query_map(params![account_id], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?)
    }

    /// Every tweet in a named collection, most-recently-collected first. `kind` is
    /// the stored key ("bookmarks", "tweets"). Ordering by `collected_at` — not the
    /// tweet's authored time — keeps a freshly bookmarked old tweet at the top where
    /// the user expects to see it.
    pub fn collection(&self, kind: &str, limit: u32) -> Result<Vec<Tweet>> {
        let sql = format!(
            "{SELECT} JOIN tweet_collections c ON c.tweet_id = t.id
             WHERE c.kind = ?1 ORDER BY c.collected_at DESC, t.created_at DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![kind, limit], row_to_tweet)?;
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
    pub edges: i64,
    pub dms: i64,
    pub bytes: i64,
}

/// Upsert a full profile row (id, handle, name, bio, follower counts, location).
fn upsert_profile(tx: &rusqlite::Transaction, p: &Profile, now: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO profiles(id, handle, display_name, bio, followers_count, following_count, location, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET handle=excluded.handle, display_name=excluded.display_name,
             bio=excluded.bio, followers_count=excluded.followers_count,
             following_count=excluded.following_count, location=excluded.location",
        params![p.id, p.handle, p.name, p.bio, p.followers, p.following, p.location, now],
    )?;
    Ok(())
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
        params![
            tweet.author.id,
            tweet.author.username,
            tweet.author.name,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO tweets(id, author_profile_id, text, created_at, reply_to_id, like_count, media_json, media_count)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET text=excluded.text, created_at=excluded.created_at,
             reply_to_id=excluded.reply_to_id, like_count=excluded.like_count,
             media_json=excluded.media_json, media_count=excluded.media_count",
        params![
            tweet.id,
            tweet.author.id,
            tweet.text,
            tweet.created_at.as_deref().unwrap_or_default(),
            tweet.in_reply_to_status_id,
            tweet.like_count.unwrap_or(0),
            serde_json::to_string(&tweet.media).unwrap_or_else(|_| "[]".to_string()),
            i64::try_from(tweet.media.len()).unwrap_or(0),
        ],
    )?;
    tx.execute(
        "DELETE FROM tweets_fts WHERE tweet_id = ?1",
        params![tweet.id],
    )?;
    tx.execute(
        "INSERT INTO tweets_fts(tweet_id, text) VALUES(?1, ?2)",
        params![tweet.id, tweet.text],
    )?;
    Ok(true)
}

fn row_to_tweet(row: &rusqlite::Row) -> rusqlite::Result<Tweet> {
    Ok(Tweet {
        id: row.get(0)?,
        text: row.get(1)?,
        created_at: row.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
        in_reply_to_status_id: row.get(3)?,
        like_count: row
            .get::<_, Option<i64>>(4)?
            .map(|n| u64::try_from(n).unwrap_or(0)),
        author: Author {
            id: row.get(5)?,
            username: row.get(6)?,
            name: row.get(7)?,
        },
        // the tweets table stores neither of these, so archived reads drop them.
        retweet_count: None,
        reply_count: None,
        conversation_id: None,
        media: row
            .get::<_, String>(8)
            .ok()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
    })
}

fn val_to_json(v: rusqlite::types::Value) -> serde_json::Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => serde_json::Value::Null,
        V::Integer(i) => serde_json::Value::from(i),
        V::Real(f) => serde_json::Value::from(f),
        V::Text(s) => serde_json::Value::from(s),
        V::Blob(b) => serde_json::Value::from(String::from_utf8_lossy(&b).into_owned()),
    }
}

fn json_to_val(v: &serde_json::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        serde_json::Value::Null => V::Null,
        serde_json::Value::Bool(b) => V::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map_or_else(|| V::Real(n.as_f64().unwrap_or(0.0)), V::Integer),
        serde_json::Value::String(s) => V::Text(s.clone()),
        other => V::Text(other.to_string()),
    }
}

/// Stable content fingerprint of a profile's snapshot-worthy fields.
fn snapshot_hash(p: &Profile) -> String {
    let mut h = Sha256::new();
    for field in [
        p.handle.as_str(),
        p.name.as_str(),
        p.bio.as_str(),
        &p.followers.to_string(),
        &p.following.to_string(),
        p.location.as_deref().unwrap_or(""),
    ] {
        h.update(field.as_bytes());
        h.update([0x1f]);
    }
    h.finalize().iter().take(8).fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
    use crate::models::Media;

    // created_at is ISO 8601 here because the parser normalizes it before archiving.
    fn tweet(id: &str, uid: &str, handle: &str, text: &str, iso_date: &str) -> Tweet {
        Tweet {
            id: id.into(),
            text: text.into(),
            author: Author {
                id: uid.into(),
                username: handle.into(),
                name: handle.into(),
            },
            created_at: Some(iso_date.into()),
            reply_count: Some(1),
            retweet_count: Some(2),
            like_count: Some(3),
            conversation_id: Some(id.into()),
            in_reply_to_status_id: None,
            media: Vec::new(),
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
        db.archive(&[tweet(
            "9",
            "u9",
            "carol",
            "designing agent loops",
            "2026-07-06T10:00:00.000Z",
        )])
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
        db.archive(&[tweet(
            "9",
            "u9",
            "carol",
            "designing agent loops carefully",
            "2026-07-06T10:00:00.000Z",
        )])
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
        db.archive(std::slice::from_ref(&t)).unwrap();
        let mut updated = t;
        updated.text = "edited".into();
        db.archive(&[updated]).unwrap();
        let all = db.recent(10).unwrap();
        assert_eq!(all.len(), 1, "same id upserts, not duplicates");
        assert_eq!(all[0].text, "edited");
        assert_eq!(db.find("edited", 10).unwrap().len(), 1);
        assert_eq!(
            db.find("first", 10).unwrap().len(),
            0,
            "fts reindexed on update"
        );
    }

    #[test]
    fn archiving_records_and_reads_back_media() {
        let mut db = db();
        let mut t = tweet("1", "u", "h", "hi", "2026-07-06T10:00:00.000Z");
        t.media = vec![
            Media {
                kind: "photo".into(),
                url: "https://pbs.twimg.com/media/A.jpg?name=orig".into(),
                alt: Some("a cat".into()),
            },
            Media {
                kind: "cover".into(),
                url: "https://pbs.twimg.com/media/C.jpg".into(),
                alt: None,
            },
        ];
        db.archive(std::slice::from_ref(&t)).unwrap();

        // media survives a write/read round-trip through the media_json column
        let back = db.recent(10).unwrap();
        assert_eq!(back[0].media, t.media);

        // media_count is denormalized beside it
        let count: i64 = db
            .conn
            .query_row("SELECT media_count FROM tweets WHERE id = '1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn backup_round_trips_and_rebuilds_fts() {
        let mut src = db();
        src.archive_collection(
            &[tweet(
                "42",
                "u42",
                "dave",
                "backup me",
                "2026-07-06T10:00:00.000Z",
            )],
            "acct1",
            "bookmarks",
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!("kicau-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        src.export_backup(&dir).unwrap();

        let mut restored = db();
        restored.import_backup(&dir).unwrap();
        let s = restored.stats().unwrap();
        assert_eq!(s.tweets, 1);
        assert_eq!(s.collections, 1);
        assert_eq!(
            restored.find("backup", 10).unwrap().len(),
            1,
            "fts rebuilt on import"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_profile_snapshots_only_on_change() {
        let mut db = db();
        let mut p = Profile {
            id: "u1".into(),
            handle: "alice".into(),
            name: "Alice".into(),
            bio: "hi".into(),
            followers: 5,
            following: 2,
            location: None,
            verified: false,
        };
        let count = |db: &Db| -> i64 {
            db.conn
                .query_row("SELECT count(*) FROM profile_snapshots", [], |r| r.get(0))
                .unwrap()
        };
        db.save_profile(&p).unwrap();
        db.save_profile(&p).unwrap(); // identical → no new snapshot
        assert_eq!(count(&db), 1, "re-saving unchanged profile is idempotent");
        p.followers = 6; // profile changed
        db.save_profile(&p).unwrap();
        assert_eq!(count(&db), 2, "a changed profile records a new snapshot");
    }

    #[test]
    fn bookmark_ids_are_the_bookmarked_ones_not_merely_archived() {
        // The correctness the incremental stop rests on: "already seen" must mean
        // "already a bookmark", not "the tweet exists in the archive". A tweet
        // archived via another path but freshly bookmarked is NOT yet a bookmark,
        // so the incremental fetch must still reach and record it.
        let mut db = db();
        db.archive(&[tweet(
            "1",
            "u1",
            "a",
            "seen in home only",
            "2026-07-01T00:00:00.000Z",
        )])
        .unwrap();
        db.archive_collection(
            &[tweet(
                "2",
                "u2",
                "b",
                "an actual bookmark",
                "2026-07-02T00:00:00.000Z",
            )],
            "me",
            "bookmarks",
        )
        .unwrap();

        let ids = db.bookmark_ids("me").unwrap();
        assert!(ids.contains("2"), "the bookmarked tweet is included");
        assert!(
            !ids.contains("1"),
            "a merely-archived tweet is NOT a bookmark"
        );
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn bookmark_ids_are_scoped_to_the_account() {
        let mut db = db();
        db.archive_collection(
            &[tweet("1", "u1", "a", "mine", "2026-07-01T00:00:00.000Z")],
            "me",
            "bookmarks",
        )
        .unwrap();
        db.archive_collection(
            &[tweet("2", "u2", "b", "yours", "2026-07-02T00:00:00.000Z")],
            "you",
            "bookmarks",
        )
        .unwrap();
        assert_eq!(
            db.bookmark_ids("me").unwrap(),
            std::collections::HashSet::from(["1".to_string()])
        );
    }

    #[test]
    fn archive_collection_records_membership_idempotently() {
        let mut db = db();
        let t = tweet("3", "u3", "carol", "saved", "2026-07-06T10:00:00.000Z");
        assert_eq!(
            db.archive_collection(std::slice::from_ref(&t), "acct1", "bookmarks")
                .unwrap(),
            1
        );
        // re-syncing the same tweet doesn't duplicate the collection row
        db.archive_collection(&[t], "acct1", "bookmarks").unwrap();
        let s = db.stats().unwrap();
        assert_eq!(s.tweets, 1);
        assert_eq!(s.collections, 1);
        assert_eq!(
            db.find("saved", 10).unwrap().len(),
            1,
            "collection tweets are also searchable"
        );
    }

    #[test]
    fn archive_skips_records_without_stable_keys() {
        let mut db = db();
        let mut no_author = tweet("8", "", "ghost", "no author id", "2026-07-06T10:00:00.000Z");
        no_author.author.id = String::new();
        let no_id = tweet("", "u1", "alice", "no tweet id", "2026-07-06T10:00:00.000Z");
        db.archive(&[no_author, no_id]).unwrap();
        assert_eq!(
            db.recent(10).unwrap().len(),
            0,
            "unkeyable records are skipped, not fragmented"
        );
    }

    #[test]
    fn find_tolerates_quotes_in_query() {
        let mut db = db();
        db.archive(&[tweet(
            "7",
            "u7",
            "erin",
            "she said hi today",
            "2026-07-06T10:00:00.000Z",
        )])
        .unwrap();
        // Must not raise an FTS5 syntax error.
        let _ = db.find("said \"hi\"", 10).unwrap();
    }
}
