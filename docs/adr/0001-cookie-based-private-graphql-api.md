# 1. Use X's private GraphQL API with session cookies

Status: Accepted

## Context

X's official public API (v2) requires a developer app, is heavily rate-limited,
and cannot reach much of what a logged-in session can — the home timeline, DMs,
another user's tweets without special access, and most write actions. kicau's
goal is full personal access to one's own account from the terminal.

## Decision

Authenticate as the web client does: send the user's `auth_token` and `ct0`
session cookies plus the public web bearer token, and call X's private
`/i/api/graphql/*` and `/i/api/1.1/*` endpoints directly. kicau presents a
browser-like user-agent and the headers the web app sends.

## Consequences

- Full capability: everything the logged-in web app can do (reads, engagement,
  follow graph, bookmarks, DMs, media, posting).
- Coupled to a private, undocumented, moving protocol: query ids rotate
  (see ADR-0002), content writes need an anti-automation header (ADR-0003),
  and the API server can lag the shipped web bundle. Fragility is inherent and
  is mitigated with runtime self-healing, not eliminated.
- This is single-account, human-scale use. It is not a platform for automated
  abuse or high-volume scraping. Cookies are credentials and are treated as
  secrets (stored `0600`, never committed).
