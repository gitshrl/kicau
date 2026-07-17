# kicau

A single-binary CLI for X (Twitter) that talks to the private GraphQL API
using your existing session cookies. No developer app, no OAuth dance. Every
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

kicau sync bookmarks -n 2000 # every bookmark, plus which folder each is filed in
kicau folders # your bookmark folders, and how many are in each
kicau folders "AI Engineering" # what is filed in one
kicau find "rust" --folder "AI Engineering" # search inside one folder
```

### Commands

**Read**
`read` · `search` · `mentions` · `replies` · `thread` · `user` · `user-tweets` ·
`home` · `bookmarks` · `list` · `dms` · `dm`

**Write**
`tweet` · `reply` · `delete` · `like`/`unlike` · `retweet`/`unretweet` ·
`bookmark`/`unbookmark` · `follow`/`unfollow` · `blocks` · `mutes` · `upload`

**Local data**
`find` (FTS over archived tweets, `--folder` to scope it) · `folders` (your
bookmark folders) · `log` (recent archived) · `sync` (bookmarks/tweets → SQLite) ·
`graph` (follow-graph snapshot) · `profiles` (profile snapshot) · `db stats` ·
`backup export|import` · `import` (X data export)

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

### Bookmarks and folders

`kicau sync bookmarks -n 2000` fetches every bookmark, follows X's cursor to the
end, and pulls in the body of any Article it finds. Articles arrive from some
timelines as a bare t.co link, so those get fetched individually: expect roughly
a second per Article on a first run.

It then records which folder each bookmark is filed in, as a label beside the
bookmark rather than a copy of it. Folder membership mirrors X, so unfile
something there and the label goes on the next sync. If you keep no folders,
nothing extra is fetched or written.

The bookmark list itself is a record, not a mirror: unbookmark something in X and
the archive keeps it.
