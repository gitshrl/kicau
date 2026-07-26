# 11. Log in by reading the browser's X session, not OAuth

Status: Accepted

## Context

`kicau init` made you paste `auth_token` and `ct0` by hand — the two session
cookies kicau drives X's private GraphQL API with (ADR 0001). Digging them out of
browser devtools is the worst part of setup.

The clean "click a URL to sign in" is OAuth, and it does not fit. X's OAuth issues
developer v2-API tokens, not session cookies; kicau is built entirely on the
cookie + private GraphQL surface, which the v2 API cannot stand in for — it is
gated behind paid developer tiers and missing most of what kicau reads. And no
redirect or callback can ever hand out another origin's session cookies:
`auth_token` is `HttpOnly`, so not even a page you open can read it. The only way
to get the cookies without typing them is to read them from a browser the user is
already signed into.

## Decision

`kicau login` replaces `kicau init`. It reads `auth_token`/`ct0` from an installed
browser (Chrome, Edge, Firefox, Safari) with cookie-scoop, verifies the session
against X, shows the account, and writes the cookies to `~/.kicau/config.toml` —
no paste.

- **Graphical session required.** Reading a browser needs a desktop, so login
  detects a headless machine — Linux with neither `DISPLAY` nor `WAYLAND_DISPLAY`;
  macOS is always treated as graphical because its cookie stores stay readable
  over SSH — and there skips the scan and prompts for a manual paste. It also
  falls back to manual when no session is found, the user declines, or the session
  fails to verify.
- **Storage unchanged.** Cookies stay in `config.toml` at mode 0600; the OS
  keychain is not used, and how cookies resolve at runtime (flags → env → config)
  is untouched.
- **No OAuth, no dev app.** That would be a different, weaker API and would
  reverse ADR 0001.
- **One bundled SQLite.** rusqlite is pinned to 0.31 so it and cookie-scoop share
  a single `libsqlite3-sys` — two versions both claim the `sqlite3` native link
  and will not co-link.

## Consequences

- On a desktop, setup is `kicau login`, pick the account, done; re-run to refresh
  an expired session.
- On a headless box login is honest: it says browser login needs a desktop and
  takes a manual paste. There is no remote or OAuth path — bringing a session over
  from a desktop is the user's to do.
- cookie-scoop adds a runtime need for the OS keyring CLI (`secret-tool` /
  `security`) to decrypt Chromium cookies; Firefox needs none. It links no C
  library, so the static musl release build is unaffected.
