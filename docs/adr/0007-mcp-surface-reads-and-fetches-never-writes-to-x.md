# 7. The MCP surface reads and fetches, never writes to X

Status: Accepted

## Context

kicau exposes the archive to agents over MCP (`kicau mcp`, stdio transport) so a
tool like Claude Code can query what you have saved without shelling out to the
CLI and parsing text. An MCP server hands an autonomous model a set of tools it
can call on its own initiative, which changes the threat model from every other
part of kicau: the caller is not a person typing one command, it is a model
emitting tool calls in a loop.

That shift decides the surface. kicau can post, like, follow, retweet, delete,
and fetch live from X with your session cookies. Handing any of those to an
agent means an agent can act as you on X. The line that matters is between
reading and writing: a model that reads your archive or fetches a public post is
useful and recoverable; a model that posts or deletes as you is not.

## Decision

The MCP surface reads and fetches. It never writes to X.

- **Four archive tools** (`search_archive`, `list_bookmarks`, `recent_tweets`,
  `archive_stats`) read the local SQLite store. They need no credentials and
  touch no network, so a server registered with no cookies still answers
  everything about what you have saved.
- **One network tool**, `read_tweet`, fetches a single post live and archives
  it, exactly as `kicau read` does. It is available by default; its only
  requirement is resolved credentials, and without them it returns a result
  telling the model to run `kicau login` rather than failing the request.
- **No write tools.** Nothing in the surface posts, likes, follows, retweets,
  bookmarks, or deletes. This is the boundary the ADR exists to defend: a future
  contributor adding a `post_tweet` tool should meet this decision first.

`read_tweet` carries no runaway cap. X's own rate limiting, surfaced to the
model as an `isError` result, is the backpressure; a second, home-grown limiter
would duplicate what X already does. The deliberate choice is that the agent may
read from X freely and act on X never.

The protocol is hand-rolled against the 2025-11-25 spec rather than taken from
`rmcp`. The surface is four JSON-RPC methods of newline-delimited JSON; the SDK
that would supply them brings proc-macros and a schema generator for code that
fits on a page, against a project that keeps its dependency list deliberately
short.

## Consequences

- An agent registered with `claude mcp add kicau -- kicau mcp` can read your
  archive and fetch live posts with your cookies. It cannot act as you: the
  surface has no tool that changes anything on X.
- A tool that ran and failed returns `isError: true`, never a JSON-RPC error, so
  the model reads the reason and recovers. Only failing to find a tool is a
  protocol error.
- stdout carries protocol and nothing else. A client parses every line as JSON
  and one stray write ends the session, so all logging goes to stderr. An
  integration test drives the real subprocess to hold that line.
- The surface stays read-only against X by construction, so no archive tool can
  ever be made to act on your behalf. `read_tweet` still writes to the local
  archive, matching the CLI, so an agent's reads enrich what you can later search.
- No new crate enters the tree: the stdio transport uses tokio's `io-std` and
  `io-util` features, which are features, not dependencies.
