# Architecture

kicau is one binary, ~3.9k lines of Rust, no daemon and no server. A command
runs, does its work, and exits. State is two files under `~/.kicau`: a TOML
config and a SQLite database.

Terms used below are defined in [CONTEXT.md](CONTEXT.md).

## Modules

| Module | Lines | Role |
|---|---:|---|
| `main.rs` | 878 | clap CLI, one arm per command, offline/online split |
| `client.rs` | 977 | `TwitterClient`: request shapes, headers, every API method |
| `db.rs` | 626 | SQLite archive, FTS5, backup export/import |
| `parse.rs` | 498 | X's JSON → `models`; the shape-quirk containment layer |
| `transaction.rs` | 334 | `x-client-transaction-id` derivation |
| `query.rs` | 163 | query-id resolution and bundle scraping |
| `config.rs` | 133 | filesystem layout, `config.toml`, credential resolution |
| `output.rs` | 125 | human and `--json` rendering |
| `models.rs` | 98 | `Tweet`, `Profile`, `Author`, `DmConversation`, `DmMessage` |
| `extract.rs` | 35 | post URL → id |

Dependencies run one way, toward `models`:

```
main ─┬─ client ──┬─ parse ──┐
      │           ├─ query ──┴─ config
      │           └─ transaction
      ├─ db ──────── config
      ├─ output ─────────────┐
      └─ extract             └─ models
```

`config.rs`, `models.rs`, `transaction.rs` and `extract.rs` are leaves — they
import nothing from the crate. `models` is the shared vocabulary every layer
speaks; nothing in it knows about X's wire format.

## A command's path

```
main.rs  clap parses argv
   │
   ├── offline: init · config · find · log · db · backup · import · check
   │   └── never resolves credentials, never touches the network
   │
   └── online:
       config.rs      resolve auth_token + ct0  (flags → env → config.toml)
       client.rs      build the request
         query.rs       pick the query id
         transaction.rs derive x-client-transaction-id   (writes only)
         ↓
         GET/POST  x.com/i/api/graphql/<query id>/<Operation>
         ↓
       parse.rs       JSON → models
       output.rs      print          db.rs   archive (unless --no-db)
```

The offline set is checked before credentials are resolved, so `kicau find` on a
machine with no cookies works rather than erroring about a missing config.

## Talking to a moving API

X's private API rotates ids, gates fields behind flags, and fails in ways that
look like success. Three mechanisms absorb that, all inside `client.rs`:

**Query id candidates.** `query::candidates()` returns the ids to try in order —
your `config.toml` pin, the compiled-in curated default, then anything scraped
this process. Each 404 falls through to the next. If all of them 404, the id
rotated out: scrape x.com's bundles for current ids, then retry once. Curated
before scraped is deliberate — see *deploy skew*.

Scraped ids live in memory for the life of the process and are not persisted. An
id worth keeping goes in `config.toml`, which the program never rewrites.

**Request shape.** `Call::Read | Search | Write` selects GET, POST-hybrid, or
POST. This is a property of the operation, not of whether it mutates.

**Retry.** `is_transient()` catches X's periodic `DeadlineExceeded`. Reads retry
once; writes never do, because a retried `CreateTweet` posts twice.

Feature flags and `fieldToggles` are per-operation. `ARTICLE_OPS` lists the
operations that accept `withArticlePlainText`; sending it anywhere else is an
error, so the list is explicit rather than a blanket parameter.

## Parsing

`parse.rs` exists so the rest of the codebase never sees X's wire format. It
absorbs: `TweetWithVisibilityResults` wrappers, the legacy-vs-core split in user
fields, three different places a post's text can live, timeline entries nested
two different ways, and X's non-ISO date format. Everything downstream gets a
`models::Tweet` with a `text` that is the actual text and a `created_at` that is
ISO 8601.

Normalising at parse time rather than at use time is load-bearing: dates arrived
in two formats before, and sorting a thread by a day-of-week-prefixed string
sorted it by day name.

## Storage

Eight tables — `profiles`, `tweets`, `accounts`, `tweet_collections`,
`follow_edges`, `profile_snapshots`, `dm_conversations`, `dm_messages` — plus
`tweets_fts`, an FTS5 index mirrored from `tweets`.

Every write is an upsert. Re-running any command converges instead of
accumulating: posts update in place, snapshots key on a content hash rather than
a timestamp, collection membership is idempotent. `db.rs` owns all SQL; no
other module writes a query.

## Tests

Unit tests live beside the code — 42 of them, weighted where the risk is:
`parse.rs` (13) and `db.rs` (9) carry the most, because X's shapes and SQLite
idempotency are where silent wrongness hides.

Two in `transaction.rs` are golden differential tests, pinning exact outputs of
the animation-key and transaction-id derivations against a known-good reference.
They are the tripwire for X changing the algorithm: if they fail, the header
needs re-deriving, and posting is broken until it is.

`output.rs` has no tests — it prints, and its shape is checked by eye.

## Deliberately absent

No AI features, no web server, no background daemon, no plugin system. kicau
fetches, prints, and archives. Anything else composes from `--json`.
