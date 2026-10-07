use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::review::Review;

const TIMESTAMP_FORMAT: &str = "%Y%m%dT%H%M%SZ";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedReview {
    pub pr_id: u64,
    pub repo: String,
    pub title: String,
    pub source_branch: String,
    pub target_branch: String,
    #[serde(default)]
    pub source_commit: Option<String>,
    pub reviewed_at: DateTime<Utc>,
    pub review: Review,
}

pub fn reviews_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("reviews")
}

/// Writes `<id>-<timestamp>.json` under `dir` and returns its path.
pub fn save(dir: &Path, saved: &SavedReview) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Could not create cache directory {}", dir.display()))?;
    let path = dir.join(format!(
        "{}-{}.json",
        saved.pr_id,
        saved.reviewed_at.format(TIMESTAMP_FORMAT)
    ));
    let json = serde_json::to_string_pretty(saved)?;
    std::fs::write(&path, json).with_context(|| format!("Could not write {}", path.display()))?;
    Ok(path)
}

/// Saves unparseable Claude output so it can be inspected.
pub fn save_raw_failure(cache_dir: &Path, pr_id: u64, raw: &str) -> Result<PathBuf> {
    let dir = cache_dir.join("failed");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Could not create cache directory {}", dir.display()))?;
    let path = dir.join(format!("{}-{}.txt", pr_id, Utc::now().format(TIMESTAMP_FORMAT)));
    std::fs::write(&path, raw).with_context(|| format!("Could not write {}", path.display()))?;
    Ok(path)
}

/// Most recent saved review for a PR, if any.
pub fn latest(dir: &Path, pr_id: u64) -> Result<Option<(PathBuf, SavedReview)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Could not read {}", dir.display())),
    };
    let prefix = format!("{pr_id}-");
    // The timestamp format sorts lexically, so the greatest name is the newest.
    let newest = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "json")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
        })
        .max();
    let Some(path) = newest else { return Ok(None) };
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read {}", path.display()))?;
    let saved = serde_json::from_str(&text)
        .with_context(|| format!("Saved review {} is corrupt", path.display()))?;
    Ok(Some((path, saved)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::Verdict;
    use chrono::TimeZone;

    fn saved(pr_id: u64, hour: u32, summary: &str) -> SavedReview {
        SavedReview {
            pr_id,
            repo: "repo".into(),
            title: "t".into(),
            source_branch: "s".into(),
            target_branch: "main".into(),
            source_commit: None,
            reviewed_at: Utc.with_ymd_and_hms(2026, 10, 7, hour, 0, 0).unwrap(),
            review: Review {
                verdict: Verdict::Approve,
                summary: summary.into(),
                comments: vec![],
                criteria: vec![],
            },
        }
    }

    #[test]
    fn round_trips_and_returns_newest_for_the_right_pr() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = reviews_dir(tmp.path());
        save(&dir, &saved(12, 9, "old")).unwrap();
        let newest = save(&dir, &saved(12, 11, "new")).unwrap();
        save(&dir, &saved(123, 23, "other pr")).unwrap();

        assert_eq!(newest.file_name().unwrap(), "12-20261007T110000Z.json");
        let (path, got) = latest(&dir, 12).unwrap().unwrap();
        assert_eq!(path, newest);
        assert_eq!(got, saved(12, 11, "new"));
    }

    #[test]
    fn missing_review_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(latest(&reviews_dir(tmp.path()), 1).unwrap().is_none());
        save(&reviews_dir(tmp.path()), &saved(2, 1, "x")).unwrap();
        assert!(latest(&reviews_dir(tmp.path()), 1).unwrap().is_none());
    }
}
