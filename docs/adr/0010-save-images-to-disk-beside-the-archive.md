# 10. Save post and article images to disk beside the archive

Status: Accepted

## Context

The archive held a tweet's text but not its pictures. X serves media inside the
same `TweetDetail` response the archive already reads:

- **Post photos** at `legacy.extended_entities.media[]`, `type = "photo"`, URL
  `media_url_https`; full resolution is that URL with `?name=orig`.
- **Video / animated GIF** carry a poster image at `media_url_https`; the `.mp4`
  lives in `video_info.variants[]`.
- **Article cover** at `article.article_results.result.cover_media.media_info
  .original_img_url`. An article's inline body images are not in the response —
  it carries only `cover_media`, `plain_text`, `preview_text`, `title`,
  `metadata`, so there is nothing else to read.

## Decision

On every archive, extract each tweet's media and download the image files to
disk beside the SQLite database, recording the media list in the row.

- **Files on disk, not blobs.** Images go to `~/.kicau/media/<tweet_id>/
  <index>.<ext>` (an article cover to `<tweet_id>/cover.<ext>`). Keeping them as
  files leaves the database small and the pictures usable directly. A file that
  already exists is not re-downloaded, so re-archiving is idempotent and there is
  no separate backfill command.
- **Full resolution for photos** (`?name=orig` on `pbs.twimg.com/media` URLs).
  Video and GIF contribute their poster image only; the `.mp4` is out of scope.
  An article saves its cover only, because its inline images are not in the API
  surface kicau fetches.
- **Existing columns, no migration.** The list and count are written to the
  `media_json` and `media_count` columns, which have existed in the `tweets`
  schema since the first release. kicau runs no migrations — `init()` only issues
  `CREATE TABLE IF NOT EXISTS`, and SQLite never adds a column to an existing
  table — so a feature that reuses declared columns upgrades every archive with
  no error, while a feature that needed a *new* column would break them. This one
  was built to need none.
- **`db.rs` stays network-free.** Metadata is written from parsed data; the file
  download runs from the client/orchestration with bounded concurrency (six in
  flight), never one task per image.
- **A failed download warns and is skipped**, never failing the archive: the
  tweet, its text, and its `media_json` still persist, matching the
  error-beside-data posture of ADR 0005.

## Consequences

- Every command that archives (`read`, `home`, `user`, `search`, `tweets`,
  `bookmarks`) now also saves images; `--no-db` saves nothing.
- The archive holds the pictures, full-text search still runs on the text, and a
  future contributor knows why articles keep only a cover and videos only a
  poster: the rest is not in the response.
- Adding any genuinely new column later needs a migration story first, since
  there is none today.
