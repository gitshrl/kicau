# browser-login

## Goal

Replace `kicau init` with `kicau login`, which reads the X session cookies
(`auth_token`, `ct0`) straight from an installed browser where the user is
already signed in, so nobody copies cookies by hand. Manual paste stays as the
fallback. Everything about how cookies are stored and used is unchanged:
`~/.kicau/config.toml`, mode 0600, resolved flags → env → config.

Extraction uses the `cookie-scoop` crate: pure Rust, no native library to link
(it shells out to `secret-tool`/`security` for the keyring and bundles SQLite via
rusqlite), so the static musl release still builds.

## Non-goals

- **No OS-keychain storage.** Cookies still live in `~/.kicau/config.toml` at
  mode 0600. Moving secrets into the Keychain/secret-service is out of scope.
- **No password or OAuth login.** The only mechanism is reading an existing
  browser session's cookies.
- **No change to cookie *use*.** The resolution order (flags → env → config) and
  every read path are untouched.
- **No `init` alias.** `init` is removed, not kept as a hidden alias.
- **No Windows.** kicau targets Linux + macOS; the crate's Windows path is unused.
- **No new stored fields.** Only `auth_token` and `ct0` are written, as today.

## Acceptance criteria

- AC-1: `kicau init` is not a command (unrecognized subcommand). `kicau login`
  exists and its `--help` works.
- AC-2: On an interactive terminal with a graphical session, `kicau login` scans
  installed browsers (Chrome, Edge, Firefox, Safari) for an `x.com` session
  carrying both `auth_token` and `ct0`, attributing each found session to its
  browser.
- AC-3: With exactly one session found, `login` verifies it against X
  (`current_user`), prints the resolved `@handle` and the source browser, asks
  for confirmation, and on yes writes the config.
- AC-4: With more than one session found, `login` lists them by browser and
  prompts the user to choose one; the chosen session is verified and written.
- AC-5: With no session found, or the user declines, or verification fails,
  `login` falls back to the existing manual prompt (`print_cookie_guidance` +
  `prompt_credentials`).
- AC-6: On success the state dir exists and `auth_token`/`ct0` are written to
  `~/.kicau/config.toml` at mode 0600, byte-for-byte the same template as before.
- AC-7: `kicau login` is re-runnable: an existing `config.toml` does not short-
  circuit it; it re-runs and overwrites after confirming the overwrite.
- AC-8: Non-interactive (`!stdin.is_terminal()`) `kicau login` never blocks on a
  prompt: it auto-selects when exactly one session exists and writes it; with
  zero or multiple sessions it writes the blank template, as `init` did when
  piped.
- AC-9: Every user-facing string that said "kicau init" now says "kicau login" —
  the MCP `read_tweet` failure text, the post-write guidance, and `README.md`.
- AC-10: The session-selection logic is a pure function over the extracted
  cookies (grouped by browser) and has a unit test: a browser with both cookies
  yields a session; one with only `auth_token` (or only `ct0`) yields none.
- AC-11: A browser that cannot be read (absent, locked keyring, Safari without
  Full Disk Access) never crashes `login`; it is skipped with a note and the
  scan continues to the next browser, then to the manual fallback.
- AC-12: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features
  -- -D warnings`, and `cargo test --locked` pass; the two transaction golden
  tests pass unchanged.
- AC-13: The only new direct dependency is `cookie-scoop`. `README.md` documents
  `kicau login` and no longer instructs `kicau init`.
- AC-14: `kicau login` detects a headless machine — Linux with neither `DISPLAY`
  nor `WAYLAND_DISPLAY` set; macOS is always treated as graphical — and there
  skips the browser scan, going straight to the manual prompt (interactive) or
  the blank template (piped), with a line saying browser login needs a desktop.
  The GUI test is a pure, unit-tested function.

## Verification

cargo build --release
./target/release/kicau init 2>&1 | grep -qi "unrecognized"
./target/release/kicau login --help >/dev/null
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
grep -q "kicau login" README.md && ! grep -qE "kicau init" README.md
grep -rq "kicau login" src/mcp.rs
# interactive, with a browser signed into x.com:
./target/release/kicau login   # detects the session, verifies @handle, writes config
