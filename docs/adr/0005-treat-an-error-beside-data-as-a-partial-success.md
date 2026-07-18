# 5. Treat a GraphQL error beside data as a partial success, for reads only

Status: Accepted

## Context

GraphQL allows a response to carry `data` and `errors` at the same time: a query
can half-fail and still return what it resolved. X uses this, and not rarely.

Paginating bookmarks, roughly one page in twenty comes back like this:

```json
{ "data": { "bookmark_timeline_v2": { "timeline": { "instructions": [ ...100 tweets... ] } } },
  "errors": [ { "message": "Query: Unspecified", "kind": "Operational" } ] }
```

A full page of tweets, the cursor for the next page, and an error beside them.
Retrying does not help: the same page answers the same way, deterministically.

kicau originally failed on any non-empty `errors` array. The effect was invisible
and severe. A 1,671-bookmark timeline stopped at the first such page and returned
**395** — 76% of the data dropped, reported as a clean error. It was mistaken for
a limit in X's API ("bookmarks wall at ~400") and that conclusion was wrong; the
wall was this error handling.

The opposite mistake is equally available. `TwitterClient::action` — like,
retweet, bookmark, delete — discards the response body entirely:

```rust
self.fetch_graphql(operation, variables, Value::Null, Call::Write).await?;
Ok(())
```

Tolerating an error there would print `❤️ liked` for a like that never happened.

There is a third shape. A *failed* read answers with the requested field present
but null, beside an error:

```json
{ "data": { "bookmark_collection_timeline": null },
  "errors": [ { "message": "Query: Unspecified" } ] }
```

Read as "data present", that error gets swallowed and the caller sees an empty
timeline — which, for anything that mirrors a collection, means deleting it.

## Decision

An error beside data is fatal unless the call is a read **and** the data block
actually resolved something:

```rust
if matches!(call, Call::Write) || !has_content(&data) {
    return Err(GqlError::Api(msg));
}
```

`has_content` requires at least one non-null value. An empty object, a null, or
an object whose every field is null is not an answer, whatever sits beside it.

Writes never get the benefit of the doubt. For a write, the error *is* the
result: the operation either happened or it did not, and the body is thrown away
anyway.

## Consequences

- A long backfill survives X's periodic hiccups. On the account this was found on,
  the same command went from 395 bookmarks to about 1,670 from this change alone.
  X's exact total drifts by a few between runs, which is its own reason not to
  quote a precise figure.
- Reads can return a page X reported an error about. In practice that page is
  complete and the error is noise, but the guarantee is "what X sent", not "what
  X meant".
- Writes stay honest and loud, at the cost of failing on an error that might have
  been survivable.
- The rule is per-`Call`, so a new operation inherits the right behaviour from its
  request shape rather than from a list someone maintains.
- If X ever returns a genuinely truncated page beside an error, a read will accept
  it silently. Nothing in the response distinguishes "partial page" from "whole
  page with a warning", so this is unfixable from the client side; the mitigation
  is that pagination continues from the cursor X supplies with it.
