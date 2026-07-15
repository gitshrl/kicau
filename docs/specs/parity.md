# kicau — full CLI + local data-layer parity

## Goal

Extend kicau from the read/write MVP to a complete cookie-based X client with a
local SQLite data layer: every timeline, engagement, graph, bookmark, media,
DM, and moderation operation, plus sync/import/backup of the local store. No AI
features, no web server.

## Existing (done)

whoami, check, read, search, mentions, tweet, reply, replies, thread,
update-query-ids, find, log. Default-on SQLite archive of fetched tweets.

## New surface

### Phase 5 — read timelines (GET, reuse the timeline parser + archive)
- `user <handle>` — profile via UserByScreenName.
- `user-tweets <handle> [-n]` — a user's tweets via UserTweets.
- `home [-n]` — home timeline via HomeTimeline / HomeLatestTimeline.
- `lists` — the account's lists; `list <id> [-n]` — a list timeline via ListLatestTweetsTimeline.
- `bookmarks [-n]` — the account's bookmarks via Bookmarks.

### Phase 6 — engagement + graph writes (POST)
- `like <id>` / `unlike <id>` — FavoriteTweet / UnfavoriteTweet.
- `retweet <id>` / `unretweet <id>` — CreateRetweet / DeleteRetweet.
- `follow <handle>` / `unfollow <handle>` — friendships create/destroy.
- `bookmark <id>` / `unbookmark <id>` — CreateBookmark / DeleteBookmark.
- All support `--dry-run`.

### Phase 7 — media
- `tweet`/`reply` gain `--media <path>` (repeatable) with `--alt <text>`.
- Chunked upload: INIT → APPEND → FINALIZE against the upload host; attach media_ids.

### Phase 8 — local data/sync layer
- `sync <bookmarks|likes|tweets> [--limit]` — fetch a live collection and persist it
  into SQLite `tweet_collections` (kind) + `tweets` + `profiles`.
- `blocks [add|remove] <handle>` / `mutes [add|remove] <handle>` — REST create/destroy,
  mirrored into local `blocks`/`mutes` tables.
- `graph <followers|following> <handle>` — snapshot the follow graph into `follow_edges`.
- `profiles <handle>` — capture a profile snapshot into `profile_snapshots`.
- `db stats` — row counts and storage size of the local store.

### Phase 9 — DMs
- `dms` — list conversations via the DM inbox GraphQL; `dm <conversation>` — messages.
- Persist into `dm_conversations` / `dm_messages` (+ FTS).

### Phase 10 — import / backup
- `import <path>` — ingest a downloaded X data export (tweets, likes, DMs) into SQLite.
- `backup export <dir>` / `backup import <dir>` — git-friendly text dump/restore of the store.

## Constraints
- Every network op routes through the existing client (query-id resolution, 404
  self-heal, transient retry, Call::Read/Search/Write shapes) — extend, don't fork.
- All read commands archive by default and honor `--no-db`, `--json`, `--plain`.
- Writes support `--dry-run`. A retried write must never double-act.
- SQLite tables follow the normalized column shapes already established.
- No AI, no web server, no background daemon.

## Verification
- Per phase: `cargo build/test/clippy` clean; live smoke of each new command
  against the real account; read commands archive and are queryable via find/log.
