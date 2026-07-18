# 9. No bookmark folders — the API is premium-gated

Status: Accepted

## Context

X lets you file bookmarks into named folders. The GraphQL operations that back
them — `BookmarkFoldersSlice` (the folder list) and `BookmarkFolderTimeline` (a
folder's contents) — are gated to X Premium. kicau reaches the private GraphQL
API with a normal account's session cookies, so on a non-premium account those
two operations do not answer; the feature works for a minority of users and
fails quietly for the rest.

kicau had modelled folders anyway: a `folder:<name>` collection mirrored from X
on every bookmark sync, a `folders` command, a `--folder` scope on `find`, and
folder arguments on the MCP `search_archive`, `list_bookmarks`, and
`archive_stats` tools.

## Decision

kicau does not model bookmark folders. Bookmarks are one flat collection.

Removed: the `folders` command and `find --folder`; the folder pass on
`kicau bookmarks` (`sync_bookmark_folders`) and the `replace_labels` mirror it
drove; the client fetches (`bookmark_folders`, `bookmark_folder_ids`); the `Db`
folder methods (`folders`, `folder_tweets`, `find_in_folder`, `folder_kind`);
the two premium query ids; and the folder arguments across the MCP surface.

## Consequences

- `kicau bookmarks` fetches, archives, and shows the newest, with no folder pass
  — one fewer round of per-folder timeline reads on every sync.
- `find` searches the whole archive; the MCP tools carry no folder arguments and
  `archive_stats` no longer reports folder sizes.
- A pre-existing archive may still hold `folder:%` rows written by an earlier
  version. Nothing reads or writes them now, so they are inert; kicau does not
  migrate user data, so they stay until the owner clears them.
- Re-adding folders means re-adding calls that fail for every non-premium
  account. Do not, unless kicau first learns to detect a premium session and
  offer the feature only where it works.
