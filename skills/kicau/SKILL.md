---
name: kicau
description: Use when working with X/Twitter from the terminal (reading, searching, posting, bookmarks, DMs, timelines) or querying a local offline archive of X data. kicau is a cookie-auth CLI plus MCP server; prefer its free offline search over live calls.
---

# kicau

kicau is a CLI and MCP server for X (Twitter). It uses your browser session
cookies against X's private GraphQL API (no developer app, no OAuth) and archives
every post it fetches to a local SQLite database you can search offline for free.

## Authenticate

Run `kicau login` once. On a desktop it reads your X session from an installed
browser (Chrome, Edge, Firefox, Safari), verifies it, and saves it, with no
copy-paste. On a headless box or with no browser it prompts you to paste the two
cookies (`auth_token`, `ct0`). Re-run any time to refresh an expired session.
Confirm with `kicau whoami` or `kicau check`.

## Prefer the local archive

Every fetch is stored, so search it offline with no network and no rate limit:

- `kicau find "query"`: full-text search over your archived posts
- `kicau log`: the most recently archived posts

Reach for these before calling X live.

## Read from X (archives as it goes)

- `kicau read <id-or-url>`: one post, Article body included
- `kicau search "query"`: search X
- `kicau tweets [handle]`: a user's posts, or yours if the handle is omitted
- `kicau user <handle>`: a profile
- `kicau home`: your timeline
- `kicau bookmarks`: sync new bookmarks and show the newest (`-n N` to show N,
  `--all` to refetch every one)
- `kicau thread <id>`, `kicau replies <id>`, `kicau mentions`
- `kicau dms`, `kicau dm <id-or-handle>`

## Write to X

`tweet`, `reply`, `like`/`unlike`, `retweet`/`unretweet`, `bookmark`/`unbookmark`,
`follow`/`unfollow`, `delete`, `blocks`, `mutes`. Every write accepts `--dry-run`
to preview without posting.

## MCP server

`kicau mcp` serves the archive over MCP on stdio. Register it once:
`claude mcp add kicau -- kicau mcp`. Tools:

- `search_archive`: full-text search the local archive, offline
- `list_bookmarks`: archived bookmarks, offline
- `recent_tweets`: most recently archived posts, offline
- `archive_stats`: counts and size, offline
- `read_tweet`: fetch one post live by id or URL and archive it, the only tool
  that calls X

The first four touch no network. The surface reads and fetches only: it never
posts, likes, follows, or deletes.

These five are the whole MCP surface. Everything else above (live `search`, a
user's timeline, `home`, bookmark sync, `thread`/`replies`/`mentions`, DMs, and
every write) is CLI-only and not reachable over MCP.

## Global flags

`--json` for machine-readable output, `--plain` for stable text, `--no-db` to
skip archiving, and `--dry-run` on any write.
