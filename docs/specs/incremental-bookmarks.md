# incremental-bookmarks

## Goal

Fetch less from X on a bookmark re-sync by stopping pagination once it reaches
already-archived bookmarks, and collapse the redundant commands: `kicau bookmarks`
becomes the one bookmark command (fetch, archive, folders, display), `sync` is
removed, and `user-tweets` becomes `tweets`.

## Non-goals

- **No early-stop on `tweets` or any other timeline.** The incremental stop is
  bookmarks-only. `home`, `tweets`, `list` keep their count-based fetch.
- **No change to folder mirror semantics.** Folders are still fetched in full and
  mirrored with `replace_labels`; early-stop never touches the folder pass.
- **No `--full` flag.** The escape hatch is `--all`.
- **No change to write-to-X paths, DMs, or the transaction id derivation.**
- **`timeline()` stays DB-free.** Archive knowledge reaches it as data (a set of
  ids), never a DB handle.
- **No new crate in the tree.**

## Acceptance criteria

- AC-1: `kicau sync ...` is not a command; running it errors as an unrecognized
  subcommand. No `Sync` variant remains.
- AC-2: `kicau user-tweets` is not a command. `kicau tweets <handle>` fetches that
  user's tweets; `kicau tweets` with no handle fetches the authenticated user's
  own tweets.
- AC-3: `kicau bookmarks` archives fetched bookmarks to SQLite (unless `--no-db`),
  records their folder membership, and displays the newest N from the archive.
- AC-4: `Db::bookmark_ids` returns exactly the tweet ids recorded in the
  `bookmarks` collection for an account, and nothing else.
- AC-5: A tweet present in `tweets` but NOT in the `bookmarks` collection is
  treated as new: `bookmark_ids` excludes it, so incremental fetch does not stop
  on it and it gets recorded. (The load-bearing correctness test.)
- AC-6: The page-stop predicate is a pure function: a page with at least one id
  outside the known set does not stop; a non-empty page whose every id is known
  does. Covered by a unit test.
- AC-7: `kicau bookmarks --all` fetches the whole timeline (the known set is
  empty, so no page ever stops early).
- AC-8: `kicau bookmarks --no-db` fetches live and neither writes the archive nor
  runs the folder pass.
- AC-9: With a fully-synced archive, `kicau bookmarks` still displays the newest
  bookmarks from the archive rather than an empty result (browse survives a
  zero-new fetch).
- AC-10: `src/client.rs` imports nothing from `crate::db`; the incremental stop is
  driven by a `&HashSet<String>` argument.
- AC-11: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features
  -- -D warnings`, and `cargo test --locked` all pass; the two transaction golden
  tests pass unchanged.
- AC-12: The dependency tree gains no new crate versus `main`.
- AC-13: `README.md` documents `tweets` and the collapsed `bookmarks`, and no
  longer mentions `sync` or `user-tweets`.

## Verification

cargo build --release
./target/release/kicau sync bookmarks 2>&1 | grep -qi "unrecognized\|unexpected"
./target/release/kicau user-tweets x 2>&1 | grep -qi "unrecognized\|unexpected"
./target/release/kicau tweets --help >/dev/null
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
grep -qi "kicau tweets" README.md && ! grep -qE "kicau sync|user-tweets" README.md
! grep -q "crate::db" src/client.rs
