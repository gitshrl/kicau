# 6. Fetch Article bodies that a timeline withholds, one request each

Status: Accepted

## Context

X Articles are long-form posts. The tweet that carries one has no text of its
own: `legacy.full_text` is a 23-character t.co link, and the real content lives
in an `article` block beside it, whose body arrives as `plain_text` only when the
request sends the `withArticlePlainText` field toggle.

X is not consistent about serving that block, and the inconsistency is per
operation, not per account:

| Operation | `article` block | `plain_text` with the toggle |
|---|---|---|
| `TweetDetail` | yes | 17,675 chars |
| `UserTweets` | yes | 88,601 chars across 4 articles |
| `Likes` | yes | 49,599 chars across 4 articles |
| `HomeLatestTimeline` | sometimes | 5,722 chars — one of two was still a stub |
| `Bookmarks` | **never**, toggle or not | nothing |

So the toggle is necessary and not sufficient. On one account, 95 bookmarked
Articles were archived as bare t.co links — the toggle was being sent and X
ignored it.

Nothing in the bookmarks payload marks these tweets as Articles: no `article`
key, no card, no type. The only surviving evidence is the expanded url:

```
entities.urls[].expanded_url = "http://x.com/i/article/2077511961974337536"
```

Only `TweetDetail` reliably returns a body, and it takes one tweet at a time. X
publishes no batch equivalent — neither its own client bundles nor the reference
implementations contain one.

## Decision

Send the toggle to every operation that declares it, and separately repair what
comes back anyway.

Detect Article tweets by their expanded url, matching on the **host** rather than
a substring — `notx.com/i/article/1` ends with the same characters as the real
thing. Then re-fetch, through `TweetDetail`, only those whose text is still a
bare t.co link:

```rust
article_ids.contains(&tweet.id) && is_link_stub(&tweet.text)
```

The stub test is what makes the two mechanisms compose instead of duplicate. An
Article that arrived whole costs nothing; one X withheld costs one request.

Folder labelling is deliberately excluded: it reads ids only. Writing the tweets
from a folder timeline would overwrite an already-fetched body with the stub that
timeline serves.

## Consequences

- Every read path returns Articles whole: `read`, `search`, `home`, `user-tweets`,
  `bookmarks`, `list`, `thread`. The archive that held 95 bare links now holds the
  bodies, 82 of them over 2,000 characters and the longest 32,667, all full-text
  searchable offline.
- A first bookmarks sync costs roughly one request and one second per Article, so
  a large backfill takes minutes rather than seconds. Re-syncing pays it again:
  the client does not consult the archive to see what it already has.
- `user-tweets` and `likes` pay nothing, because the toggle already worked there.
- `ARTICLE_OPS` lists the operations that accept the toggle. `ListLatestTweetsTimeline`
  is in it but unverified — no list was available to test against. If X rejects it
  there, that operation errors until the entry is removed.
- Detection rests on X keeping `/i/article/` in the expanded url. If that changes,
  Articles silently become links again; nothing else in the payload identifies them.
