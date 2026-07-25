# save-media

## Goal

Save the images attached to posts and articles to the local archive, so the
archive holds the pictures, not just the text. On every archive, extract each
tweet's media and download the image files to disk beside the SQLite database,
and record the media metadata in the row.

Grounded in a spike of the raw `TweetDetail` response (`2080710971228918066`):

- **Post photos** live at `legacy.extended_entities.media[]`, `type = "photo"`,
  URL `media_url_https`. Full resolution is the same URL with `?name=orig`.
- **Video / animated GIF** carry a poster image at `media_url_https`; the `.mp4`
  is in `video_info.variants[]` and is out of scope.
- **Article cover** is at
  `article.article_results.result.cover_media.media_info.original_img_url`.
- **Article inline body images are not in the response** — the article result is
  only `cover_media`, `plain_text`, `preview_text`, `title`, `metadata`. There is
  no `content_state` or inline media map to read, so an article saves its cover
  only.

## Non-goals

- **No `.mp4` / video files.** Video and GIF contribute their poster image only.
- **No article inline images.** Not present in the API surface kicau fetches;
  only the cover is saved.
- **No schema migration.** The `tweets` table already declares `media_json` and
  `media_count`; this populates them. No `ALTER TABLE`, no new column.
- **No `entities_json` population.** URLs/hashtags/mentions stay out of scope.
- **No backfill command.** Media is saved going forward; re-archiving an existing
  tweet (e.g. the next `kicau bookmarks`) downloads its media then, idempotently.
- **`db.rs` stays network-free.** Metadata is written from parsed data; the file
  download lives in the client/orchestration, never in `db.rs`.
- **No new crate.** The existing HTTP client downloads the bytes.
- **No unbounded fan-out.** Downloads run with bounded concurrency.

## Acceptance criteria

- AC-1: The `Tweet` model carries a `media` field. `parse.rs` extracts, from
  `legacy.extended_entities.media[]`, each item's `type` (photo | video |
  animated_gif), image URL, and `ext_alt_text`; and, for an article, the
  `cover_media` image URL.
- AC-2: A photo's saved URL is full resolution: `?name=orig` is appended for
  `pbs.twimg.com/media` URLs. A video or GIF contributes its poster image; no
  `.mp4` URL is downloaded.
- AC-3: Archiving a tweet with media writes its media list to the existing
  `media_json` column and sets `media_count`. The `tweets` `CREATE TABLE` is
  unchanged versus `main` — no migration.
- AC-4: On archive, each image is downloaded to
  `~/.kicau/media/<tweet_id>/<index>.<ext>` (the article cover to
  `<tweet_id>/cover.<ext>`). A file that already exists is not re-downloaded.
- AC-5: A failed image download prints a warning to stderr and does not fail the
  archive or the command; the tweet, its text, and its `media_json` still persist.
- AC-6: `--no-db` downloads no files and writes no rows.
- AC-7: Media saving runs from the shared archive path, so it applies to every
  command that archives (`read`, `home`, `user`, `search`, `tweets`,
  `bookmarks`).
- AC-8: Image downloads run with bounded concurrency, not one task per media item.
- AC-9: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features
  -- -D warnings`, and `cargo test --locked` pass; the two transaction golden
  tests pass unchanged.
- AC-10: The dependency tree gains no new crate versus `main`.
- AC-11: `src/db.rs` imports nothing from the HTTP client; the file download is
  driven from outside `db.rs`.

## Verification

cargo build --release
# a photo post: files land under ~/.kicau/media/<id>/, media_json populated
./target/release/kicau read <PHOTO_POST_ID>
ls ~/.kicau/media/<PHOTO_POST_ID>/
python3 -c "import sqlite3,os; c=sqlite3.connect(f'file:{os.path.expanduser(\"~/.kicau/kicau.sqlite\")}?mode=ro',uri=True); print(c.execute(\"SELECT media_count, media_json FROM tweets WHERE id=?\", ('<PHOTO_POST_ID>',)).fetchone())"
# the article: cover saved, no crash on absent inline images
./target/release/kicau read 2080710971228918066
ls ~/.kicau/media/2080710971228918066/
# --no-db writes nothing
HOME=$(mktemp -d) ./target/release/kicau read <PHOTO_POST_ID> --no-db; ls ~/.kicau/media 2>&1 | grep -q "No such" && echo "no media on --no-db"
# schema unchanged, no new crate
git diff main -- src/db.rs | grep -q "CREATE TABLE IF NOT EXISTS tweets" && echo "check tweets DDL untouched"
git diff main -- Cargo.lock
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
