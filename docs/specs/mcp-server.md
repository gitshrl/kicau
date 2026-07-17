# mcp-server

## Goal

Finish the MCP server on the `mcp-server` branch so it is safe to merge: keep the
surface read-and-fetch only (no tool ever writes to X), prove the transport with
an automated round-trip test, and record the security posture.

## Non-goals

- **No write-to-X tools.** The MCP surface reads and fetches; it never posts,
  likes, follows, retweets, bookmarks, or deletes. This boundary is the point of
  the ADR, not an omission to fill later.
- **No `--live` flag or gating on `read_tweet`.** It is available by default; its
  only requirement is that credentials resolve.
- **No runaway cap or throttle on `read_tweet`.** X's own 429, surfaced as
  `isError`, is the backpressure.
- **No change to `read_tweet`'s archiving.** It persists what it fetches, exactly
  as `kicau read` does.
- **No X-response wire-shape fixture harness, no `ARCHITECTURE.md` restoration,
  no `rmcp` dependency, and no guest-token auth path.**
- **No new crate in the tree** (`io-std`/`io-util` are tokio features).

## Acceptance criteria

- AC-1: `tools/list` returns exactly five tools, including `read_tweet`.
- AC-2: `read_tweet` called with resolved credentials fetches the post live and
  returns its text (exercised manually against a real account; the hermetic test
  covers the no-credential form).
- AC-3: `read_tweet` called with no credentials returns a result with
  `isError: true` whose text names `kicau init`; it is not a crash and not a
  protocol error.
- AC-4: `initialize` returns a `protocolVersion` in
  {2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05, 2024-10-07}.
- AC-5: An unknown method returns JSON-RPC error -32601; an unknown tool returns
  -32602.
- AC-6: A tool that runs and fails returns `isError: true`, never a JSON-RPC error.
- AC-7: A malformed stdin line does not terminate the server; it logs to stderr
  and keeps serving subsequent requests. A notification (no id) draws no reply.
- AC-8: Every byte written to stdout is a valid single-line JSON message, even
  when a tweet's text contains newlines.
- AC-9: `tests/mcp_protocol.rs` spawns the built binary, drives the real
  stdin/stdout protocol, and asserts AC-1, AC-3, AC-4, AC-5, and AC-7. It is
  hermetic: it sets `HOME` to a temp dir and strips credential env, so it never
  reads the real archive or resolves cookies.
- AC-10: `docs/adr/0007-*.md` exists, records the read-and-fetch-only posture and
  the hand-rolled-not-`rmcp` choice; it is brand-clean and matches 0001-0006.
- AC-11: `README.md` documents the MCP server and a `claude mcp add` line.
- AC-12: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features
  -- -D warnings`, and `cargo test --locked` all pass; the two transaction golden
  tests still pass unchanged.
- AC-13: The dependency tree gains no new crate versus the pre-branch tree.

## Verification

cargo build --release
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | ./target/release/kicau mcp | python3 -c "import json,sys; t=json.load(sys.stdin)['result']['tools']; assert len(t)==5 and any(x['name']=='read_tweet' for x in t), t"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_tweet","arguments":{"tweet":"1"}}}' | env -u KICAU_AUTH_TOKEN -u KICAU_CT0 HOME=$(mktemp -d) ./target/release/kicau mcp | python3 -c "import json,sys; r=json.load(sys.stdin)['result']; assert r['isError'] and 'kicau init' in r['content'][0]['text'], r"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/kicau mcp | python3 -c "import json,sys; v=json.load(sys.stdin)['result']['protocolVersion']; assert v in {'2025-11-25','2025-06-18','2025-03-26','2024-11-05','2024-10-07'}, v"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"bogus"}' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope"}}' | ./target/release/kicau mcp | python3 -c "import json,sys; e=[json.loads(l)['error']['code'] for l in sys.stdin if l.strip()]; assert e==[-32601,-32602], e"
printf '%s\n' 'not json' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"archive_stats"}}' | ./target/release/kicau mcp 2>/dev/null | python3 -c "import json,sys; l=[x for x in sys.stdin if x.strip()]; assert len(l)==1 and json.loads(l[0]), l"
cargo test --locked --test mcp_protocol
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
test -f docs/adr/0007-*.md && ! grep -riE "bird|jawond|steipete" docs/adr/0007-*.md README.md
grep -qi "mcp" README.md
