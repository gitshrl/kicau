# 4. Default-on local SQLite archive, idempotent, id-keyed

Status: Accepted

## Context

kicau should retain what it fetches — searchable offline, backed up, and usable
by other tools — without depending on an external service or a running daemon.

## Decision

A single embedded SQLite database at `~/.kicau/kicau.sqlite` (via `rusqlite`,
bundled — no system SQLite). Every read command archives the tweets it fetched
**by default**; `--no-db` opts out; a DB failure is best-effort (warns, never
fails the command or suppresses output).

Schema is normalized: `tweets`, `profiles`, `tweet_collections`, `follow_edges`,
`profile_snapshots`, DM tables, plus an FTS5 index for full-text search. Keys are
chosen for stability, not convenience:

- tweets keyed by X tweet id; profiles keyed by X **user id**, with `handle`
  deliberately **not unique** because X recycles handles across accounts.
- records missing a stable id (rare limited-visibility results) are skipped
  rather than collapsed onto an empty key.

**All writes are idempotent.** Tweets/profiles/collections/DMs/follow-edges use
`ON CONFLICT` upserts; profile snapshots are keyed by a **content hash**
`(profile_id, snapshot_hash)`, so re-saving an unchanged profile is a no-op and
only a genuine change records a new snapshot — regardless of timing.

## Consequences

- Offline `find` (FTS) and `log`, git-friendly `backup export/import`, and
  re-runnable `sync` with no duplication.
- Only fetched data is archived; there is no automatic backfill (use `import`
  for a downloaded X data export).
- Schema changes to regenerable tables (e.g. profile_snapshots) are handled by a
  one-time drop-and-recreate in `Db::init`, since the store is a local cache, not
  a system of record.
