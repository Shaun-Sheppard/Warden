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
    /// Show a system notification when a review starts and when it finishes.
    pub notifications: bool,
    /// Triage the work items assigned to the user.
    pub triage_enabled: bool,
    /// Projects whose boards are triaged; falls back to `projects`, then all.
    pub triage_projects: Vec<String>,
    /// Fix items triage marked for an AI agent without being asked.
    pub triage_auto_fix: bool,
    /// Triage and fixing start nothing new (what is running finishes).
    /// Pull request reviews carry on.
    pub triage_paused: bool,
    /// How many work items are fixed at once (1 to 5).
    pub triage_max_fixes: u32,
    /// Tag triaged items and comment on the ones for a person. Off keeps the
    /// results in Warden only; fixes still push and open pull requests.
    pub triage_post_results: bool,
    /// Triage without tagging or commenting on work items. Separate from `dry_run`, and on until turned off.
    pub triage_dry_run: bool,
    /// Custom triage guidance; the built-in guidance is used when empty.
    pub triage_prompt: String,
    /// Only items in each project's current sprint.
    pub triage_current_sprint: bool,
    /// Board states that count as "to do".
    pub triage_states: Vec<String>,
    /// Work item types to triage.
    pub triage_types: Vec<String>,
    /// Which repository and branch the work in each area path belongs to.
    pub area_mappings: Vec<AreaMapping>,
    /// Everything configured per project: review, triage and its areas.
    /// `projects`, `people`, `triage_projects` and `area_mappings` above are only
    /// read once, to move older settings into these (see `normalise`).
    pub project_configs: Vec<ProjectConfig>,
}

/// One project: whether its pull requests are reviewed and how, and whether
/// its board is triaged and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectConfig {
    pub name: String,
    /// Review this project's pull requests.
    pub review: bool,
    /// Only review PRs raised by these people; empty means everyone.
    pub people: Vec<String>,
    /// Vote on each PR, and set clean ones to auto-complete.
    pub approve_and_complete: bool,
    /// `squash`, `noFastForward` or `rebase`.
    pub merge_strategy: String,
    pub delete_source_branch: bool,
    /// Triage the work items assigned to the user on this project's board.
    pub triage: bool,
    /// Fix the items triage marks for an AI agent without being asked.
    pub auto_fix: bool,
    /// Only items in the current sprint.
    pub current_sprint: bool,
    /// Tag triaged items and comment on the ones for a person.
    pub post_results: bool,
    /// Board states that count as "to do".
    pub states: Vec<String>,
    /// Work item types to triage.
    pub types: Vec<String>,
    /// Which repository and branch the work in each area path belongs to.
    pub areas: Vec<AreaMapping>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            review: true,
            people: Vec::new(),
            approve_and_complete: false,
            merge_strategy: "noFastForward".to_string(),
            delete_source_branch: false,
            triage: false,
            auto_fix: false,
            current_sprint: true,
            post_results: false,
            states: vec!["To Do".to_string(), "New".to_string()],
            types: vec!["Bug".to_string()],
            areas: Vec::new(),
        }
    }
}

impl Settings {
    /// Moves settings from before projects were configured one by one into
    /// `project_configs`, and keeps the derived values in step.
    pub fn normalise(&mut self) {
        let has = |list: &[String], name: &str| list.iter().any(|p| p.eq_ignore_ascii_case(name));
        let legacy = !self.projects.is_empty() || !self.triage_projects.is_empty() || !self.area_mappings.is_empty();
        // Also covers first-time setup, which picks projects and people in the old form.
        if legacy {
            let mut names: Vec<String> = Vec::new();
            for name in self.projects.iter().chain(&self.triage_projects).chain(self.area_mappings.iter().map(|m| &m.project)) {
                let known = self.project_configs.iter().any(|c| c.name.eq_ignore_ascii_case(name));
                if !name.trim().is_empty() && !has(&names, name) && !known {
                    names.push(name.clone());
                }
            }
            for name in names {
                let triaged = if self.triage_projects.is_empty() { has(&self.projects, &name) } else { has(&self.triage_projects, &name) };
                self.project_configs.push(ProjectConfig {
                    review: has(&self.projects, &name),
                    people: self.people.clone(),
                    approve_and_complete: self.approve_and_complete,
                    merge_strategy: self.merge_strategy.clone(),
                    delete_source_branch: self.delete_source_branch,
                    triage: self.triage_enabled && triaged,
                    auto_fix: self.triage_auto_fix,
                    current_sprint: self.triage_current_sprint,
                    post_results: self.triage_post_results,
                    states: self.triage_states.clone(),
                    types: self.triage_types.clone(),
                    areas: self.area_mappings.iter().filter(|m| m.project.eq_ignore_ascii_case(&name)).cloned().collect(),
                    name,
                });
            }
            // Moved; from now on each project's own settings are the only ones.
            self.projects.clear();
            self.people.clear();
            self.triage_projects.clear();
            self.area_mappings.clear();
        }
        for config in &mut self.project_configs {
            for area in &mut config.areas {
                area.project = config.name.clone();
            }
        }
        self.triage_enabled = self.project_configs.iter().any(|c| c.triage);
        // Triage had a pause of its own; the one pause in the sidebar now covers everything.
        self.triage_paused = false;
    }

    /// The settings for a project, if it is configured.
    pub fn project(&self, name: &str) -> Option<&ProjectConfig> {
        self.project_configs.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Projects whose pull requests are reviewed.
    pub fn reviewed_projects(&self) -> Vec<&ProjectConfig> {
        self.project_configs.iter().filter(|c| c.review).collect()
    }

    /// Projects whose boards are triaged.
    pub fn triaged_projects(&self) -> Vec<&ProjectConfig> {
        self.project_configs.iter().filter(|c| c.triage).collect()
    }

    /// Every project's area mappings, for matching an item to its repository.
    pub fn all_areas(&self) -> Vec<AreaMapping> {
        self.project_configs.iter().flat_map(|c| c.areas.iter().cloned()).collect()
    }

    /// Whether a project fixes items automatically.
    pub fn auto_fixes(&self, project: &str) -> bool {
        self.project(project).is_some_and(|c| c.triage && c.auto_fix)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AreaMapping {
    pub project: String,
    /// Covers this area and everything below it.
    pub area_path: String,
    pub repo: String,
    pub branch: String,
    /// The user's own checkout of the repository, to copy local-only files from.
    #[serde(default)]
    pub local_path: String,
    /// Files and folders, relative to the repository, copied from `local_path`
    /// into each fix's worktree (local settings and the like). Never committed.
    #[serde(default)]
    pub copy_paths: Vec<String>,
    /// Also copy every file git does not track from `local_path` (local
    /// settings, `.env` files), except dependencies and build output.
    #[serde(default)]
    pub copy_local_only: bool,
    /// Runs once in each fix's worktree before Claude starts, e.g.
    /// `dotnet restore && cd Acme.Portal && npm ci`.
    #[serde(default)]
    pub setup_command: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            organization: String::new(),
            projects: Vec::new(),
            people: Vec::new(),
            poll_seconds: 60,
            approve_and_complete: false,
            // "Merge" in the app: keeps the branch's commits and adds a merge commit.
            merge_strategy: "noFastForward".to_string(),
            delete_source_branch: false,
            cli_path: String::new(),
            dry_run: false,
            paused: false,
            review_existing: false,
            setup_complete: false,
            theme: "system".to_string(),
            mention_author: true,
            review_prompt: String::new(),
            notifications: true,
            triage_enabled: false,
            triage_projects: Vec::new(),
            triage_prompt: String::new(),
            triage_current_sprint: true,
            triage_dry_run: true,
            triage_post_results: false,
            triage_auto_fix: false,
            triage_paused: false,
            triage_max_fixes: 1,
            triage_states: vec!["To Do".to_string(), "New".to_string()],
            triage_types: vec!["Bug".to_string()],
            area_mappings: Vec::new(),
            project_configs: Vec::new(),
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
    /// Steps that went wrong (shown red).
    #[serde(default)]
    pub failed: Vec<u8>,
    /// Steps deliberately not carried out, e.g. in a dry run (shown grey).
    #[serde(default)]
    pub skipped: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    /// setup | paused | reviewing | error | idle
    pub status: String,
    /// A check for new pull requests is in flight right now.
    pub checking: bool,
    pub last_check: Option<DateTime<Utc>>,
    /// When the next check is due, and when the wait for it began.
    #[serde(default)]
    pub next_check: Option<DateTime<Utc>>,
    #[serde(default)]
    pub wait_started: Option<DateTime<Utc>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FixStatus {
    /// Claude is editing a private copy of the branch.
    Generating,
    /// Changes are waiting for the user to approve or discard.
    Ready,
    Pushing,
    /// Committed to the pull request's branch.
    Pushed,
    /// Claude decided nothing could be changed safely.
    NoChanges,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixIssue {
    pub severity: String,
    pub title: String,
    pub location: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixFile {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

/// A fix Claude prepared for issues the user picked from a review. Nothing
/// reaches the repository until the user approves the diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub id: String,
    pub record_id: String,
    pub pr: PrInfo,
    pub status: FixStatus,
    pub issues: Vec<FixIssue>,
    /// The branch commit the fix was made on top of.
    pub base_commit: Option<String>,
    pub diff: String,
    /// The diff was too large to keep in full.
    pub diff_truncated: bool,
    pub files: Vec<FixFile>,
    /// Claude's own account of what it did and did not fix.
    pub report: String,
    pub error: Option<String>,
    /// The commit that was pushed.
    pub commit: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub lines: Vec<LogLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriageStatus {
    Pending,
    Triaging,
    Done,
    Failed,
    /// No longer assigned to the user in a "to do" state.
    Left,
}

/// One work item from the user's board and what triage made of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageRecord {
    pub id: u64,
    pub project: String,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub area_path: String,
    pub priority: Option<i64>,
    pub url: String,
    pub status: TriageStatus,
    /// low | medium | high
    pub risk: Option<String>,
    pub ai_suitable: Option<bool>,
    /// low | medium | high
    pub confidence: Option<String>,
    /// ai | human
    pub recommendation: Option<String>,
    pub summary: String,
    pub reason: String,
    pub approach: String,
    pub files: Vec<String>,
    /// The repository and branch the item maps to, if any.
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub manipulation: Option<String>,
    pub tagged: bool,
    pub commented: bool,
    pub error: Option<String>,
    pub attempts: u32,
    pub triaged_at: Option<DateTime<Utc>>,
    pub duration_secs: u64,
    /// The latest attempt to fix the item, if any.
    #[serde(default)]
    pub fix: Option<ItemFix>,
    /// For an item that needs a person: a few words on why ("Needs decision").
    #[serde(default)]
    pub tag: String,
    /// The user moved the item to another group than triage chose.
    #[serde(default)]
    pub overridden: bool,
    /// Earlier fixes, newest last, for an item that came back to the board
    /// after being fixed (failed testing, say).
    #[serde(default)]
    pub history: Vec<ItemFix>,
    /// The item this one sits under, such as the PBI a "Dev" task belongs to,
    /// which often holds the detail.
    #[serde(default)]
    pub parent: Option<ParentItem>,
    /// For an item that left the to-do states but is still the user's and in
    /// progress on the board (moved there by hand): its board state.
    #[serde(default)]
    pub elsewhere: Option<String>,
    /// When it was first seen in that state.
    #[serde(default)]
    pub elsewhere_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemFixStatus {
    /// Asked for; starts when the current fix finishes.
    Queued,
    Fixing,
    Testing,
    Pushing,
    /// Pushed, with a pull request open.
    Done,
    /// Fixed and tested, but nothing pushed because triage dry run is on.
    DryRun,
    Failed,
}

/// The repository's test command, as it last ran on a fix.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TestRun {
    pub command: String,
    pub passed: bool,
    /// The end of the output.
    pub output: String,
    pub duration_secs: u64,
}

/// Claude fixing a work item: its own branch and worktree, the repository's
/// tests, and a pull request linked to the item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemFix {
    pub status: ItemFixStatus,
    /// Started without being asked, by auto-fix.
    #[serde(default)]
    pub automatic: bool,
    /// The queued or running work addresses comments on the open pull
    /// request, rather than making the first fix.
    #[serde(default)]
    pub feedback: bool,
    /// What the user told Claude when starting the fix, to help with it.
    #[serde(default)]
    pub notes: String,
    /// Each time Claude addressed comments on the pull request.
    #[serde(default)]
    pub feedback_rounds: Vec<FeedbackRound>,
    pub branch: String,
    pub target: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub pr_id: Option<u64>,
    #[serde(default)]
    pub pr_url: Option<String>,
    /// The state the item was moved to when the fix started, and the one it
    /// was in before, to go back to if the fix fails.
    #[serde(default)]
    pub moved_to: Option<String>,
    #[serde(default)]
    pub moved_from: Option<String>,
    /// Where the pull request's review stands: open, approved, rejected,
    /// waiting (for the author), completed or abandoned.
    #[serde(default)]
    pub review: Option<String>,
    #[serde(default)]
    pub review_checked_at: Option<DateTime<Utc>>,
    /// When `review` last changed, which is when the item entered its column.
    #[serde(default)]
    pub review_changed_at: Option<DateTime<Utc>>,
    /// The state the item was moved to once the pull request was approved.
    #[serde(default)]
    pub moved_to_review: Option<String>,
    /// Claude's report of what it changed.
    #[serde(default)]
    pub report: String,
    #[serde(default)]
    pub files: Vec<FixFile>,
    #[serde(default)]
    pub diff: String,
    #[serde(default)]
    pub diff_truncated: bool,
    #[serde(default)]
    pub tests: Option<TestRun>,
    /// Rounds of fixing: the first, plus one per failed test run Claude was asked to fix.
    #[serde(default)]
    pub rounds: u32,
    #[serde(default)]
    pub error: Option<String>,
    /// Problems after the pull request was opened, such as the item's state not changing.
    #[serde(default)]
    pub warning: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub lines: Vec<LogLine>,
}

/// The work item another sits under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentItem {
    pub id: u64,
    pub kind: String,
    pub title: String,
}

/// One pass over the review comments on a fix's pull request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRound {
    pub at: DateTime<Utc>,
    /// Open comment threads Claude was given.
    pub threads: usize,
    pub report: String,
    /// The pushed commit; none for a dry run.
    #[serde(default)]
    pub commit: Option<String>,
}

impl ItemFix {
    pub fn in_flight(&self) -> bool {
        matches!(self.status, ItemFixStatus::Fixing | ItemFixStatus::Testing | ItemFixStatus::Pushing)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TriageState {
    pub records: Vec<TriageRecord>,
    /// The item being triaged right now.
    pub current: Option<u64>,
    pub lines: Vec<LogLine>,
    pub last_check: Option<DateTime<Utc>>,
    pub error: Option<String>,
    /// Fixing is paused because a fix failed on something every fix would hit
    /// (a settings or setup problem), rather than on the item itself.
    pub fix_problem: Option<String>,
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
