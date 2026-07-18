# 8. Stop a bookmark re-sync on a fully-archived page

Status: Accepted

## Context

`kicau bookmarks` fetches the bookmarks timeline, archives it, mirrors the
folder labels, and shows the newest. A re-sync used to page the entire timeline
every run to re-archive what it already held. The archive is id-keyed and
idempotent, so this lost no data — it just spent hundreds of requests to X to
rediscover bookmarks already on disk. The goal is to fetch less: stop paging
once the fetch reaches the region it already has.

X returns the bookmarks timeline newest-first by *bookmark* time, with promoted
content off, so a freshly added bookmark sits above every one archived earlier.
That ordering is what makes an early stop possible: the new bookmarks are
contiguous at the top, and below them is the archived region.

The tempting stop is the first already-known bookmark — the moment paging
touches an id you hold, assume everything below is older and quit. It is wrong,
and the failure is silent.

## Decision

A re-sync stops when a whole page is already known, never on the first known id.

The known set is the ids recorded as bookmarks for the account
(`tweet_collections` where `kind = 'bookmarks'`), not mere presence in the
`tweets` table — a tweet archived through the home timeline and later bookmarked
must still be seen as new. The stop predicate is pure:

```rust
fn page_all_known(page: &[Tweet], known: &HashSet<String>) -> bool {
    !page.is_empty() && page.iter().all(|tweet| known.contains(&tweet.id))
}
```

First-known breaks because a known id can legitimately reappear at the *top* of
the timeline. Re-bookmarking an old tweet — unbookmark, then bookmark again, a
normal way to re-file — gives it a fresh bookmark-time, so X lifts that known id
above new bookmarks still waiting on a later page. First-known would stop on it
and drop everything below, exit zero, and print the archive's newest as if all
were well. A single bumped id cannot make an *entire* page known, so the
whole-page rule survives it. The cost is one extra page per sync past the
boundary; correctness is worth a page.

The design stays stateless. The known set is derived from existing rows each run
— no watermark, no cursor, no schema change. `--all` forces a full re-sync by
passing an empty known set, which no page can ever be a subset of, so nothing
stops early; it is the recovery for any ordering anomaly this rule does not
anticipate.

`timeline()` stays DB-free: archive knowledge reaches it as a `&HashSet<String>`
argument, never a database handle. Every non-bookmark caller passes an empty set
and so never stops early.

## Consequences

- A steady re-sync fetches one page past the newest already-archived bookmark
  and stops. Nothing below a fully-archived page is refetched.
- Articles are hydrated only when new. A bookmarked Article arrives as a bare
  t.co stub that a second request turns into title and body; the incremental
  path skips ids already held, since re-fetching a body it has is the cost the
  feature exists to avoid. The trade-off: an Article whose body fetch failed
  transiently stays a stub until `--all` revisits it.
- `kicau bookmarks` shows the archive ordered by when each entry was collected,
  not by the tweet's authored time, so a freshly bookmarked old tweet appears at
  the top where you expect it rather than buried among newer-authored posts.
- Folder labels are still fetched in full and mirrored — the early stop never
  touches the folder pass, which must see every folder to delete stale labels.
