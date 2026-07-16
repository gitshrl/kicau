# kicau

A single-binary CLI for X (Twitter) that talks to the private GraphQL API
using your existing session cookies. No developer app, no OAuth dance. Every
tweet it fetches is archived to a local SQLite database and is searchable
offline.

<p align="center">
  <img src="docs/mania.gif" alt="a cat dancing in ASCII">
</p>

<p align="center">
  <em>to make the cat dance, type <code>kicau mania</code> in your terminal</em>
</p>

## Install

Download the binary. No Rust, no build, no dependencies.

```sh
# Linux x86_64 (static, runs on any distro)
curl -sL https://github.com/gitshrl/kicau/releases/latest/download/kicau-linux-x86_64.tar.gz | tar xz

# macOS, Apple silicon
curl -sL https://github.com/gitshrl/kicau/releases/latest/download/kicau-macos-arm64.tar.gz | tar xz

# macOS, Intel
curl -sL https://github.com/gitshrl/kicau/releases/latest/download/kicau-macos-x86_64.tar.gz | tar xz

sudo mv kicau /usr/local/bin/
kicau mania
```

With Rust already installed:

```sh
cargo install --git https://github.com/gitshrl/kicau.git --locked

# or from a checkout:
cargo build --release
```

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
kicau mania # show a cat dancing in ASCII
kicau init # create the config and ask for your cookies
kicau whoami
kicau read 2074208949205881033 # id or full URL
kicau search "rust async"
kicau user ClaudeDevs
kicau home -n 30
kicau tweet "hello from kicau"
kicau tweet "with a picture" --media photo.png --alt "a description"
kicau reply <id-or-url> "nice thread"
kicau like <id> ; kicau retweet <id> ; kicau bookmark <id>
kicau find "loops" # offline, over your local archive
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

**Setup / maintenance**
`init` (create config, prompt for cookies) · `config` (show paths and
credential source) · `whoami` · `check` · `update-query-ids`

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

- `~/.kicau/kicau.sqlite` holds the tweet archive (tweets, profiles, collections,
  follow edges, profile snapshots, DMs), with FTS5 full-text search.
- Reads archive by default; `find` and `log` query it with no network.
- `kicau backup export <dir>` writes each table as git-friendly JSONL;
  `backup import <dir>` restores.
