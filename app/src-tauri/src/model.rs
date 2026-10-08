use chrono::{DateTime, Utc};
use prr::review::Review;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub organization: String,
    /// Projects to watch; empty means every project in the organization.
    pub projects: Vec<String>,
    /// Only review PRs raised by these people; empty means everyone.
    pub people: Vec<String>,
    pub poll_seconds: u64,
    /// Vote on each PR, and set clean ones to auto-complete.
    pub approve_and_complete: bool,
    /// `squash`, `noFastForward` or `rebase`.
    pub merge_strategy: String,
    pub delete_source_branch: bool,
    /// Path to the `claude` executable; found on PATH when empty.
    pub cli_path: String,
    /// Review, but never post, vote or complete.
    pub dry_run: bool,
    pub paused: bool,
    /// Whether PRs already open when monitoring first starts get reviewed.
    pub review_existing: bool,
    pub setup_complete: bool,
    /// `system`, `light` or `dark`.
    pub theme: String,
    pub mention_author: bool,
    /// Custom review guidance; the built-in guidance is used when empty.
    pub review_prompt: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            organization: String::new(),
            projects: Vec::new(),
            people: Vec::new(),
            poll_seconds: 60,
            approve_and_complete: false,
            merge_strategy: "squash".to_string(),
            delete_source_branch: false,
            cli_path: String::new(),
            dry_run: false,
            paused: false,
            review_existing: false,
            setup_complete: false,
            theme: "system".to_string(),
            mention_author: true,
            review_prompt: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrInfo {
    pub id: u64,
    pub title: String,
    pub project: String,
    pub repo: String,
    pub repo_id: String,
    pub author: String,
    pub source: String,
    pub target: String,
    pub url: String,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub time: String,
    /// info | cmd | dim | ok | warn | critical | major | minor
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Approved,
    Rejected,
    Failed,
    /// Already open when monitoring first started; deliberately left alone.
    Baseline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub critical: usize,
    pub major: usize,
    pub minor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub files: u32,
    pub additions: u32,
    pub deletions: u32,
}

/// One completed (or failed) review, as shown in History.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub record_id: String,
    pub pr: PrInfo,
    pub status: Outcome,
    pub dry_run: bool,
    pub review: Option<Review>,
    pub counts: Counts,
    /// The comment text, whether or not it was posted.
    pub comment: Option<String>,
    pub posted: bool,
    /// approved | approvedWithSuggestions | waitingForAuthor
    pub vote: Option<String>,
    pub auto_complete: bool,
    pub merged: bool,
    pub stats: Option<Stats>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_secs: u64,
    /// Why the review failed, or what went wrong after it (posting, voting).
    pub error: Option<String>,
    pub lines: Vec<LogLine>,
    /// Linked work items the review was checked against.
    #[serde(default)]
    pub work_items: Vec<WorkItemInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkItemInfo {
    pub id: u64,
    pub kind: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Current {
    pub pr: PrInfo,
    /// 0 Detected · 1 Cloned · 2 Claude review · 3 Decision · 4 Comment · 5 Vote & merge
    pub step: u8,
    pub started_at: DateTime<Utc>,
    pub lines: Vec<LogLine>,
    pub done: bool,
    pub record_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    /// setup | paused | reviewing | error | idle
    pub status: String,
    /// A check for new pull requests is in flight right now.
    pub checking: bool,
    pub last_check: Option<DateTime<Utc>>,
    /// Why the last check failed, if it did.
    pub error: Option<String>,
    pub current: Option<Current>,
    pub queue: Vec<PrInfo>,
    /// PRs another copy of Warden is reviewing right now.
    pub claimed: Vec<Claimed>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claimed {
    pub pr: PrInfo,
    pub by: String,
}

/// The marker a copy of Warden leaves on a pull request (as a hidden
/// property) so other copies do not review the same commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Which installation wrote it.
    pub instance: String,
    /// Who that installation runs as, for display.
    pub by: String,
    /// reviewing | reviewed | released
    pub state: String,
    pub commit: String,
    pub at: DateTime<Utc>,
    /// approved | rejected, once reviewed.
    #[serde(default)]
    pub decision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tracked {
    /// Source commit that was reviewed.
    pub commit: Option<String>,
    pub outcome: Outcome,
    /// Consecutive failed attempts.
    #[serde(default)]
    pub attempts: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tracking {
    /// Whether the "already open" PRs have been dealt with.
    pub baselined: bool,
    /// Identifies this installation to other copies of Warden.
    pub instance: String,
    pub prs: BTreeMap<u64, Tracked>,
}
