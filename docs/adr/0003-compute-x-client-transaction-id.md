# 3. Compute x-client-transaction-id to enable posting

Status: Accepted

## Context

Content-creating writes (`CreateTweet`) are silently dropped by X for
cookie-authenticated requests that lack a valid `x-client-transaction-id`
header: the call returns HTTP 200 with an empty `tweet_results` and no error, so
no tweet is created. Reads and simple actions (like, retweet, bookmark, follow)
are not gated this way. The header is computed client-side from the home page:
a `twitter-site-verification` key, indices parsed from an `ondemand.s` bundle,
and the loading-animation SVG frames run through cubic-bezier interpolation to
derive an animation key, then `sha256(method!path!time{keyword}{animkey})`
assembled with the key + time bytes into an XOR-obfuscated, base64 blob.

Without it, `tweet` and `reply` cannot post — the same wall every purely
cookie-based tool hits.

## Decision

Port the algorithm to Rust (`src/transaction.rs`) and send the header on
content-creating writes. Derive the per-page state once per process (fetch the
home page + ondemand bundle on first write) and mint a fresh id per request.
Best-effort: if the page layout ever stops parsing, degrade to sending no header
rather than failing the command.

Correctness is guarded by **differential golden tests**: the pure math
(`animate`) and the full assembly are asserted bit-for-bit against values
captured from a reference implementation on a live page.

## Consequences

- Posting works — the headline capability that cookie tools generally lack;
  verified with live post → read → delete round-trips.
- This is an arms race. X changes the algorithm periodically; when it does, the
  golden tests fail (catching the drift) and the port must be updated. The
  header logic is the most brittle part of kicau by design.
- One extra home-page + bundle fetch per process, on the first content write.
- Adds `sha2` and `base64` dependencies.
