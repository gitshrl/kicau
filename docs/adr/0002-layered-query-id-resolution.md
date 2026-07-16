# 2. Resolve GraphQL query ids in layers, curated before scraped

Status: Accepted

## Context

Every X GraphQL operation is addressed by an opaque build-hash `queryId` in the
URL (`/i/api/graphql/<queryId>/<Operation>`). X rotates these whenever it ships
a new web frontend, and a stale id returns HTTP 404. kicau is not a browser, so
it cannot rely on the current bundle serving the id automatically.

A surprising wrinkle observed in practice: **deploy-skew**. X's API server
sometimes 404s an id that is present in the *currently shipped* `main.js` bundle
while still honoring an *older* id. So "scrape the newest bundle id" is not
reliably correct — the newest id can be the wrong one until the server catches
up.

## Decision

Resolve an operation's id through ordered candidates, trying each until one does
not 404:

1. **User config** — `~/.kicau/config.toml` `[query_ids]` (seeded from defaults,
   hand-editable; a pinned id is never auto-clobbered).
2. **Compiled-in curated defaults** — a typed const table, known-good values.
3. **Scraped this process** — ids recovered by scraping x.com bundles, held in
   memory for the life of the command.

If every candidate 404s, scrape current ids and retry once. Crucially, the
curated default is tried **before** the scraped id, because a curated
older-but-honored id beats a freshly-shipped-but-not-yet-deployed one.

Scraped ids are not persisted. A rotation is rare and a scrape is cheap next to
a stale id cached on disk; an id worth keeping goes in `config.toml`, which the
program never rewrites.

## Consequences

- Robust to both id rotation and deploy-skew; works offline from the defaults.
- A user can pin a working id in the config file without rebuilding.
- Curated defaults can go stale over time, but they fall through to the scrape.
- Cold self-heal costs an extra scrape (home page + bundle) on first 404.
- Some content-write ids (e.g. `CreateBookmark`) can still 404 during deploy-skew
  with no honored alternative available; those are transient and self-heal when
  X aligns.
