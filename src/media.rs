//! Saving tweet images to `~/.kicau/media/<tweet_id>/`, beside the archive.
//!
//! The database keeps the metadata (`media_json`); the image bytes live on disk,
//! so they stay browsable and out of SQLite. Downloads are best-effort — a
//! failure warns and is skipped, never fatal — and a file already on disk is
//! never re-fetched.

use std::path::{Path, PathBuf};

use crate::config;
use crate::models::{Media, Tweet};

/// How many images download at once. Bounds the fan-out so archiving a
/// media-heavy timeline does not open a socket per image.
const CONCURRENCY: usize = 6;

/// Download every image these tweets carry that is not already saved.
pub async fn save(http: &reqwest::Client, tweets: &[Tweet]) {
    let base = config::state_dir().join("media");
    download_all(http, plan_downloads(&base, tweets)).await;
}

/// The downloads still needed: `(target file, image URL)` for every attached
/// image whose file is not already on disk under `base`.
fn plan_downloads(base: &Path, tweets: &[Tweet]) -> Vec<(PathBuf, String)> {
    let mut jobs = Vec::new();
    for tweet in tweets {
        let dir = base.join(&tweet.id);
        for (index, media) in tweet.media.iter().enumerate() {
            let path = dir.join(file_name(index, media));
            if !path.exists() {
                jobs.push((path, media.url.clone()));
            }
        }
    }
    jobs
}

/// An image's filename within its tweet directory: `cover.<ext>` for an article
/// cover, else `<index>.<ext>`.
fn file_name(index: usize, media: &Media) -> String {
    let ext = extension(&media.url);
    if media.kind == "cover" {
        format!("cover.{ext}")
    } else {
        format!("{index}.{ext}")
    }
}

/// The file extension of an image URL, ignoring the query string; `jpg` when the
/// URL carries none, as some video thumbnails do not.
fn extension(url: &str) -> &str {
    url.split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty() && ext.len() <= 5)
        .unwrap_or("jpg")
}

/// Run the downloads with at most `CONCURRENCY` in flight.
async fn download_all(http: &reqwest::Client, jobs: Vec<(PathBuf, String)>) {
    let mut set = tokio::task::JoinSet::new();
    let mut jobs = jobs.into_iter();
    for _ in 0..CONCURRENCY {
        let Some(job) = jobs.next() else { break };
        set.spawn(download_one(http.clone(), job));
    }
    while set.join_next().await.is_some() {
        if let Some(job) = jobs.next() {
            set.spawn(download_one(http.clone(), job));
        }
    }
}

/// Best-effort: a failed image is warned about and skipped, never fatal.
async fn download_one(http: reqwest::Client, (path, url): (PathBuf, String)) {
    if let Err(e) = try_download(&http, &url, &path).await {
        eprintln!("⚠️  image not saved ({url}): {e}");
    }
}

async fn try_download(http: &reqwest::Client, url: &str, path: &Path) -> anyhow::Result<()> {
    let bytes = http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    tokio::fs::write(path, &bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Author;

    fn media(kind: &str, url: &str) -> Media {
        Media {
            kind: kind.into(),
            url: url.into(),
            alt: None,
        }
    }

    fn tweet(id: &str, media: Vec<Media>) -> Tweet {
        Tweet {
            id: id.into(),
            text: String::new(),
            author: Author {
                id: "u".into(),
                username: "u".into(),
                name: "u".into(),
            },
            created_at: None,
            reply_count: None,
            retweet_count: None,
            like_count: None,
            conversation_id: None,
            in_reply_to_status_id: None,
            media,
        }
    }

    #[test]
    fn names_index_photos_name_the_cover_and_keep_the_extension() {
        assert_eq!(
            file_name(
                0,
                &media("photo", "https://pbs.twimg.com/media/A.jpg?name=orig")
            ),
            "0.jpg"
        );
        assert_eq!(file_name(2, &media("photo", "https://x/B.png")), "2.png");
        assert_eq!(
            file_name(0, &media("cover", "https://x/C.jpg")),
            "cover.jpg"
        );
        // A video thumbnail URL can lack an extension; default to jpg.
        assert_eq!(file_name(1, &media("video", "https://x/thumb")), "1.jpg");
    }

    #[test]
    fn plan_lists_missing_files_and_skips_saved_ones() {
        let base = std::env::temp_dir().join("kicau-media-plan-test");
        let _ = std::fs::remove_dir_all(&base);
        let t = tweet(
            "77",
            vec![
                media("photo", "https://pbs.twimg.com/media/A.jpg?name=orig"),
                media("cover", "https://x/C.jpg"),
            ],
        );

        let jobs = plan_downloads(&base, std::slice::from_ref(&t));
        assert_eq!(
            jobs,
            vec![
                (
                    base.join("77").join("0.jpg"),
                    "https://pbs.twimg.com/media/A.jpg?name=orig".to_string()
                ),
                (
                    base.join("77").join("cover.jpg"),
                    "https://x/C.jpg".to_string()
                ),
            ]
        );

        // Saving one file drops it from the next plan; the rest stay.
        std::fs::create_dir_all(base.join("77")).unwrap();
        std::fs::write(base.join("77").join("0.jpg"), b"x").unwrap();
        let jobs = plan_downloads(&base, std::slice::from_ref(&t));
        assert_eq!(
            jobs,
            vec![(
                base.join("77").join("cover.jpg"),
                "https://x/C.jpg".to_string()
            )]
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
