# kicau

A fast, single-binary CLI for X (Twitter) that talks to the private GraphQL API
using your existing session cookies — no developer app, no OAuth dance. Every
tweet it fetches is archived to a local SQLite database and is searchable
offline.

Written in Rust: static binary, quick startup, low memory, no Node/Python
runtime.

## Install

```sh
cargo build --release
# binary at ./target/release/kicau
```

## Authentication

kicau uses two cookies from a logged-in X session: `auth_token` and `ct0`.
It looks for them in this order:

1. `--auth-token` / `--ct0` flags
2. `KICAU_AUTH_TOKEN` / `KICAU_CT0`, or `AUTH_TOKEN` / `CT0` environment variables
3. `~/.kicau/cookies.env` — a shell-style file:
   ```sh
   AUTH_TOKEN=...
   CT0=...
   ```
4. `~/.config/kicau/config.json` — `{"authToken": "...", "ct0": "..."}`

Check what resolved with `kicau check`.

## Usage

```sh
kicau whoami
kicau read 2074208949205881033           # id or full URL
kicau search "rust async"
kicau user ClaudeDevs
kicau home -n 30
kicau tweet "hello from kicau"
kicau tweet "with a picture" --media photo.png --alt "a description"
kicau reply <id-or-url> "nice thread"
kicau like <id> ; kicau retweet <id> ; kicau bookmark <id>
kicau find "loops"                        # offline, over your local archive
```

### Commands

**Read**
`read` · `search` · `mentions` · `replies` · `thread` · `user` · `user-tweets` ·
`home` · `bookmarks` · `list` · `dms` · `dm`

**Write**
`tweet` · `reply` · `delete` · `like`/`unlike` · `retweet`/`unretweet` ·
`bookmark`/`unbookmark` · `follow`/`unfollow` · `blocks` · `mutes` · `upload`

**Local data**
`find` (FTS over archived tweets) · `log` (recent archived) · `sync`
(bookmarks/tweets → SQLite) · `graph` (follow-graph snapshot) · `profiles`
(profile snapshot) · `db stats` · `backup export|import` · `import` (X data export)

**Account / maintenance**
`whoami` · `check` · `update-query-ids`

Run `kicau <command> --help` for options. Every write supports `--dry-run`.

### Global flags

| Flag | Effect |
|---|---|
| `--json` | machine-readable JSON output |
| `--plain` | no color/emoji, stable text |
| `--no-db` | skip archiving to SQLite |
| `--auth-token` / `--ct0` | override cookies |
| `--timeout <ms>` | request timeout (default 30000) |

## Local storage

- `~/.kicau/kicau.sqlite` — the tweet archive (tweets, profiles, collections,
  follow edges, profile snapshots, DMs), with FTS5 full-text search.
- Reads archive by default; `find` and `log` query it with no network.
- `kicau backup export <dir>` writes each table as git-friendly JSONL;
  `backup import <dir>` restores.

## Files

| Path | Purpose |
|---|---|
| `~/.kicau/cookies.env` | session cookies |
| `~/.kicau/kicau.sqlite` | local archive |
| `~/.config/kicau/query-ids.json` | GraphQL query ids (seeded, editable) |
| `~/.config/kicau/query-ids-cache.json` | auto-refreshed query-id cache |

### Query ids

X routes each GraphQL operation through a rotating build-hash id. kicau ships
curated defaults, seeds them into `~/.config/kicau/query-ids.json` (yours to
edit), and self-heals on a 404 by scraping x.com for the current id
(`kicau update-query-ids` forces a refresh).

## Notes

- **Posting works.** Content-creating writes require X's `x-client-transaction-id`
  anti-automation header; kicau computes it. X changes that algorithm
  periodically, so posting can break until the header logic is updated.
- This uses X's private endpoints with your own session. Respect X's terms and
  rate limits; treat your cookies like passwords.
