# kicau

A CLI for X (Twitter) that talks to the private GraphQL API using your
existing session cookies. No developer app, no OAuth dance. Every
tweet it fetches is archived to a local SQLite database and is searchable
offline.

<p align="center">
  <img src="docs/mania.gif" alt="a cat dancing in ASCII">
</p>

<p align="center">
  <em>dancing with the cat by typing <code>kicau mania</code> in your terminal</em>
</p>

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/gitshrl/kicau/main/install.sh | sh
```

Linux x86_64 and macOS on Apple silicon. No Rust, no build, no dependencies.

## Authentication

Run `kicau init` once. It explains where to find your two x.com session cookies,
prompts for them, and writes `~/.kicau/config.toml`. Skip the prompts with Enter
to fill the file in by hand. `kicau config` shows where everything lives.

```toml
# ~/.kicau/config.toml
[credentials]
auth_token = "..."
ct0 = "..."
```

kicau resolves `auth_token` and `ct0` in this order:

1. `--auth-token` / `--ct0` flags
2. `KICAU_AUTH_TOKEN` / `KICAU_CT0` environment variables
3. `~/.kicau/config.toml` `[credentials]`

Check what resolved with `kicau check`.

## Usage

```sh
# setup
kicau init # create the config and ask for your cookies
kicau whoami # confirm which account the cookies belong to

# read from X
kicau read 2074208949205881033 # one post, by id or full URL
kicau search "rust async"
kicau user ClaudeDevs # someone's recent posts
kicau home -n 30 # your home timeline
kicau tweets # your own posts; kicau tweets ClaudeDevs for someone else's
kicau bookmarks # sync new bookmarks from X to SQLite, show the newest
kicau bookmarks -n 50 # show 50 (still syncs all new)
kicau bookmarks --all # re-fetch every bookmark, not just new ones

# write to X (every write takes --dry-run)
kicau tweet "hello from kicau"
kicau tweet "with a picture" --media photo.png --alt "a description"
kicau reply <id-or-url> "nice thread"
kicau like <id> ; kicau retweet <id> ; kicau bookmark <id>

# offline, over your local archive
kicau find "loops"

# just for fun
kicau mania # a cat dancing in ASCII
```

### Commands

**Read**
`read` · `search` · `mentions` · `replies` · `thread` · `user` · `tweets` ·
`home` · `bookmarks` · `folders` (bookmark folders and what is in them) ·
`list` · `dms` · `dm`

**Write**
`tweet` · `reply` · `delete` · `like`/`unlike` · `retweet`/`unretweet` ·
`bookmark`/`unbookmark` · `follow`/`unfollow` · `blocks` · `mutes` · `upload`

**Local data**
`find` (FTS over archived tweets) · `log` (recent archived) · `graph`
(follow-graph snapshot) · `profiles` (profile snapshot) · `db stats` ·
`backup export|import` · `import` (X data export)

**Setup / maintenance**
`init` (create config, prompt for cookies) · `config` (show paths and
credential source) · `whoami` · `check` · `update-query-ids` (re-scrape
x.com for current GraphQL query ids — run this when calls start failing
after X ships a change) · `mcp` (serve the archive to agents)

**Fun**
`mania`

Run `kicau <command> --help` for options. Every write supports `--dry-run`.

### Follow graph

`graph` snapshots who a handle follows, or who follows them, into the local
store:

```sh
kicau graph following kognosia
kicau graph followers kognosia -n 50 --json
```

One call returns a single page (~50 accounts) and does not expose a cursor,
so `-n 500` will not page through a large graph — it caps out at what one
request returns. For a complete list, paginate against the X GraphQL endpoint
directly using the query id from `~/.kicau/config.toml` and the bottom cursor
from each response.

### Global flags

| Flag | Effect |
|---|---|
| `--json` | machine-readable JSON output |
| `--plain` | no color/emoji, stable text |
| `--no-db` | skip archiving to SQLite |
| `--auth-token` / `--ct0` | override cookies |
| `--timeout <ms>` | request timeout (default 30000) |

## Local storage

- `~/.kicau/kicau.sqlite` holds the tweet archive (tweets, profiles, collections,
  follow edges, profile snapshots, DMs), with FTS5 full-text search.
- Reads archive by default; `find` and `log` query it with no network.
- `kicau backup export <dir>` writes each table as git-friendly JSONL;
  `backup import <dir>` restores.

## Use from an agent (MCP)

`kicau mcp` serves the archive over the Model Context Protocol on stdio, so an
agent can query what you have saved. Register it once:

```sh
claude mcp add kicau -- kicau mcp
```

It exposes five tools:

- `search_archive`: full-text search over your archive
- `list_bookmarks`: archived bookmarks
- `recent_tweets`: the most recently archived posts
- `archive_stats`: counts and size
- `read_tweet`: fetch one post live by id or URL (Article body included), and
  archive it

The first four read the local archive and touch no network. `read_tweet` is the
only tool that calls X, using your cookies; without them it says to run
`kicau init` rather than failing. The surface reads and fetches only: it never
posts, likes, follows, or deletes, so an agent can read your feed but cannot act
as you.
