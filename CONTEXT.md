# Context

Vocabulary for working on kicau. These are the terms the code turns on, and none
of them are guessable from the source alone.

## X's private GraphQL API

kicau speaks to the same API x.com's web client uses, authenticated with session
cookies rather than a developer app. That API is undocumented and moves without
notice, which is where most of this vocabulary comes from.

**auth_token** — the session cookie that authenticates you. Half of a login.

**ct0** — the CSRF cookie. Sent both as a cookie and as the `x-csrf-token`
header; X rejects the request if the two disagree. The other half of a login.
Together with `auth_token` it is equivalent to a password.

**operation** — a named GraphQL query or mutation: `TweetDetail`, `CreateTweet`,
`Bookmarks`. Each has its own URL, parameters and quirks.

**query id** — an opaque build hash in the URL for an operation
(`/graphql/<query id>/TweetDetail`). It changes whenever X ships a new client
bundle, so it is a moving part, not a constant. A rotated-out id returns 404.
Resolution is layered — see [ADR 0002](docs/adr/0002-layered-query-id-resolution.md).

**deploy skew** — X serves a client bundle containing a query id its own API has
not rolled out yet. The new id 404s while the older one still works. This is why
kicau tries its curated ids *before* anything it scrapes, and why a freshly
scraped id is not automatically the better one.

**features** — a required blob of per-operation boolean flags. Sending the wrong
set does not fail cleanly: X answers 422, or worse, 200 with an empty result.
Each operation wants its own set, so they are not interchangeable.

**fieldToggles** — a third request parameter next to `variables` and `features`,
sent only by the operations that declare it. `withArticlePlainText` is the one
that matters here: without it an Article comes back as a title and a teaser;
with it the full body arrives. X rejects toggles an operation never declared, so
they are sent per-operation rather than blanket.

**x-client-transaction-id** — an anti-automation header X derives from its
loading animation. Content-creating writes (`CreateTweet`) are accepted with a
200 and then silently dropped without it — no error, no post. Computing it is
the subject of [ADR 0003](docs/adr/0003-compute-x-client-transaction-id.md).

**request shape** — X routes operations three ways, and the shape is not implied
by the operation type. Reads are a GET with everything in the query string.
Search is a POST with `variables` still in the query string and `features` in the
body. Writes are a plain POST. Using the wrong shape returns 404.

**instructions / entries** — the shape of a timeline response. Results arrive as
a list of instructions, each holding entries, each holding either a single item
or a nested list of items. Both nestings carry posts, so both must be walked.

**TweetWithVisibilityResults** — a wrapper X puts around a post when visibility
rules apply. The real post sits one level down, under `tweet`.

**transient error** — X fails a healthy read periodically with `DeadlineExceeded`
or `Dependency: Unspecified`. Retrying once clears it. Reads only: a retried
write could double-post.

## Post bodies

X has three ways of saying "text", and `legacy.full_text` is only one of them.

**note tweet** — a long-form post. The body lives outside `legacy`, under
`note_tweet`; `full_text` holds a truncated copy.

**Article** — X's long-form publishing format, Premium-only. The payload always
carries `title` and `preview_text`, and the full body as `plain_text` only when
the request sends `withArticlePlainText`. There is also `content_state`, a
Draft.js block structure — the same text in a form that needs a renderer, which
is why kicau reads `plain_text` and ignores it.

For an Article or a note tweet, `full_text` is a t.co stub. Anything reading it
directly gets a link instead of the post.

## Local archive

**archive** — `~/.kicau/kicau.sqlite`. Reads archive by default; `--no-db` opts
out. Every write is idempotent: re-fetching a post updates its row rather than
stacking duplicates.

**FTS5** — SQLite's full-text index, mirrored from the posts table. Powers
`kicau find` with no network.

**snapshot hash** — a profile snapshot's identity is a hash of its content, not
the time it was taken. Re-observing an unchanged profile is a no-op instead of a
new row, which is what makes repeated `kicau profiles` runs idempotent.

**collection** — a named set of posts (bookmarks, likes, a list's timeline)
recorded alongside the posts themselves, so membership survives independently of
the post rows.
