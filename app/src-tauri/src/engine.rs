//! The monitoring loop: poll Azure DevOps, review each new pull request
//! once, post the review, and (if enabled) vote and set auto-complete.

use anyhow::{Context, Result};
use chrono::{Local, Utc};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

use chrono::DateTime;
use prr::ado::{author_matches, auto_complete_body, AdoClient, Identity, PrFilter, PullRequest};
use prr::auto::approval_vote;
use prr::config::{Config, ReviewConfig};
use prr::flow::{self, Lead};
use prr::pipeline::{self, Progress};
use prr::review::{Comment, CriterionStatus, Review, Severity};

use crate::model::*;
use crate::store::Store;

/// A PR whose review keeps failing is left alone after this many attempts.
pub const MAX_ATTEMPTS: u32 = 3;
const MAX_HISTORY: usize = 200;
const MAX_LINES: usize = 300;
/// Hidden pull request property holding a copy's claim.
const CLAIM_KEY: &str = "Warden.Review";
/// A claim older than this is treated as abandoned (the copy crashed or quit).
const CLAIM_TTL_MINUTES: i64 = 20;
/// How long to wait after claiming before checking nobody else claimed too.
const CLAIM_SETTLE: Duration = Duration::from_secs(3);
/// Vote value for "Waiting for author".
const VOTE_WAITING: i32 = -5;

pub enum Change {
    Live(Live),
    History(Vec<Record>),
}

pub type Notifier = Box<dyn Fn(Change) + Send + Sync>;

struct Shared {
    settings: Settings,
    live: Live,
    history: Vec<Record>,
    tracking: Tracking,
}

pub struct Engine {
    shared: Mutex<Shared>,
    store: Store,
    wake: Notify,
    notifier: Notifier,
    /// The signed-in user, looked up when first needed.
    me: Mutex<Option<Identity>>,
    /// PRs the user asked to review again, whatever other copies have done.
    forced: Mutex<std::collections::BTreeSet<u64>>,
}

#[derive(Debug, PartialEq)]
pub enum ClaimCheck {
    /// Nobody else has this commit; go ahead.
    Free,
    /// Another copy is reviewing it right now.
    Busy(String),
    /// Another copy already reviewed this commit.
    Done(Outcome),
}

/// What another copy's marker on a PR means for this copy.
pub fn check_claim(existing: Option<&Claim>, instance: &str, commit: &str, now: DateTime<Utc>) -> ClaimCheck {
    let Some(claim) = existing else { return ClaimCheck::Free };
    // Our own marker, or one about an older commit, does not hold us back.
    if claim.instance == instance || claim.commit != commit {
        return ClaimCheck::Free;
    }
    match claim.state.as_str() {
        "reviewing" if (now - claim.at).num_minutes() < CLAIM_TTL_MINUTES => ClaimCheck::Busy(claim.by.clone()),
        "reviewed" => ClaimCheck::Done(if claim.decision.as_deref() == Some("approved") {
            Outcome::Approved
        } else {
            Outcome::Rejected
        }),
        _ => ClaimCheck::Free,
    }
}

/// Whether a listed PR is due a review, given what was done with it before.
pub fn needs_review(pr: &PullRequest, tracked: Option<&Tracked>) -> bool {
    let Some(tracked) = tracked else { return true };
    let commit = pr.last_merge_source_commit.as_ref().map(|c| c.commit_id.as_str());
    let moved = commit.is_some() && commit != tracked.commit.as_deref();
    match tracked.outcome {
        Outcome::Baseline => false,
        // New commits always get a fresh look: a fix should clear a stale
        // "waiting" vote, and an approval must not cover code nobody reviewed.
        Outcome::Approved | Outcome::Rejected => moved,
        Outcome::Failed => moved || tracked.attempts < MAX_ATTEMPTS,
    }
}

/// PRs monitoring cares about at all: not drafts, raised by a chosen person.
pub fn eligible<'a>(prs: &'a [PullRequest], people: &[String]) -> Vec<&'a PullRequest> {
    prs.iter()
        .filter(|pr| !pr.is_draft && author_matches(people, &pr.created_by))
        .collect()
}

pub fn plan(prs: &[PullRequest], people: &[String], tracking: &Tracking) -> Vec<PullRequest> {
    let mut todo: Vec<PullRequest> = eligible(prs, people)
        .into_iter()
        .filter(|pr| needs_review(pr, tracking.prs.get(&pr.pull_request_id)))
        .cloned()
        .collect();
    todo.sort_by_key(|pr| pr.creation_date);
    todo
}

/// Which of the six UI steps a pipeline stage belongs to.
pub fn step_for_stage(stage: &str) -> u8 {
    if stage.starts_with("Fetching PR") || stage.starts_with("Reading linked") {
        0
    } else if stage.starts_with("Claude") || stage.starts_with("Output was not valid") {
        2
    } else if stage.starts_with("Saving") {
        3
    } else {
        // Cloning, updating or fetching the code.
        1
    }
}

pub fn counts(review: &Review) -> Counts {
    let n = |s: Severity| review.comments.iter().filter(|c| c.severity == s).count();
    Counts { critical: n(Severity::Critical), major: n(Severity::Major), minor: n(Severity::Minor) }
}

fn issue_line(comment: &Comment) -> String {
    let what = comment.title.clone().unwrap_or_else(|| {
        comment.body.lines().next().unwrap_or("").chars().take(90).collect()
    });
    format!("{:<8} {what}  {}", comment.severity.to_string().to_uppercase(), comment.location())
}

fn pipeline_config(settings: &Settings, project: &str) -> Config {
    Config {
        organization: settings.organization.clone(),
        project: project.to_string(),
        default_repo: None,
        authors: Vec::new(),
        repos: Default::default(),
        review: ReviewConfig {
            mention_author: settings.mention_author,
            prompt: Some(settings.review_prompt.clone()).filter(|p| !p.trim().is_empty()),
            ..Default::default()
        },
        auto: Default::default(),
    }
}

fn pr_info(pr: &PullRequest, org: &AdoClient) -> PrInfo {
    let project = pr.repository.project.name.clone();
    PrInfo {
        id: pr.pull_request_id,
        title: pr.title.clone(),
        url: org.with_project(&project).pr_web_url(pr),
        project,
        repo: pr.repository.name.clone(),
        repo_id: pr.repository.id.clone(),
        author: pr.created_by.display_name.clone(),
        source: pr.source_branch().to_string(),
        target: pr.target_branch().to_string(),
        commit: pr.last_merge_source_commit.as_ref().map(|c| c.commit_id.clone()),
    }
}

fn line(kind: &str, text: impl Into<String>) -> LogLine {
    LogLine { time: Local::now().format("%H:%M:%S").to_string(), kind: kind.to_string(), text: text.into() }
}

impl Engine {
    pub fn new(store: Store, notifier: Notifier) -> Self {
        let mut tracking = store.tracking();
        if tracking.instance.is_empty() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            tracking.instance = format!("{:x}-{:x}", std::process::id(), nanos);
            let _ = store.save_tracking(&tracking);
        }
        let shared = Shared {
            settings: store.settings(),
            live: Live::default(),
            history: store.history(),
            tracking,
        };
        Self {
            shared: Mutex::new(shared),
            store,
            wake: Notify::new(),
            notifier,
            me: Mutex::new(None),
            forced: Mutex::new(Default::default()),
        }
    }

    pub fn settings(&self) -> Settings {
        self.shared.lock().unwrap().settings.clone()
    }

    pub fn history(&self) -> Vec<Record> {
        self.shared.lock().unwrap().history.clone()
    }

    pub fn live(&self) -> Live {
        let shared = self.shared.lock().unwrap();
        let mut live = shared.live.clone();
        let reviewing = live.current.as_ref().is_some_and(|c| !c.done);
        live.status = if !shared.settings.setup_complete {
            "setup"
        } else if reviewing {
            "reviewing"
        } else if shared.settings.paused {
            "paused"
        } else if live.error.is_some() {
            "error"
        } else {
            "idle"
        }
        .to_string();
        live
    }

    fn emit_live(&self) {
        (self.notifier)(Change::Live(self.live()));
    }

    fn emit_history(&self) {
        (self.notifier)(Change::History(self.history()));
    }

    pub fn update_settings(&self, settings: Settings) -> Result<Settings> {
        {
            let mut shared = self.shared.lock().unwrap();
            // A different organization is a different set of PRs.
            if shared.settings.organization != settings.organization {
                shared.tracking = Tracking { instance: shared.tracking.instance.clone(), ..Default::default() };
                self.store.save_tracking(&shared.tracking)?;
                *self.me.lock().unwrap() = None;
            }
            shared.settings = settings.clone();
        }
        self.store.save_settings(&settings)?;
        self.emit_live();
        self.wake.notify_one();
        Ok(settings)
    }

    pub fn check_now(&self) {
        self.wake.notify_one();
    }

    /// Forget a PR so the next check reviews it again.
    pub fn retry(&self, pr_id: u64) -> Result<()> {
        {
            let mut shared = self.shared.lock().unwrap();
            shared.tracking.prs.remove(&pr_id);
            self.store.save_tracking(&shared.tracking)?;
        }
        // An explicit request overrides another copy's "already reviewed" marker.
        self.forced.lock().unwrap().insert(pr_id);
        self.wake.notify_one();
        Ok(())
    }

    fn log(&self, kind: &str, text: impl Into<String>) {
        {
            let mut shared = self.shared.lock().unwrap();
            if let Some(current) = shared.live.current.as_mut() {
                if current.lines.len() < MAX_LINES {
                    current.lines.push(line(kind, text));
                }
            }
        }
        self.emit_live();
    }

    fn set_step(&self, step: u8) {
        {
            let mut shared = self.shared.lock().unwrap();
            if let Some(current) = shared.live.current.as_mut() {
                current.step = current.step.max(step);
            }
        }
        self.emit_live();
    }

    fn track(&self, pr_id: u64, tracked: Tracked) {
        let mut shared = self.shared.lock().unwrap();
        shared.tracking.prs.insert(pr_id, tracked);
        if let Err(e) = self.store.save_tracking(&shared.tracking) {
            eprintln!("warden: {e:#}");
        }
    }

    pub async fn run(self: Arc<Self>) {
        loop {
            let result = self.cycle().await;
            {
                let mut shared = self.shared.lock().unwrap();
                shared.live.checking = false;
                shared.live.error = result.err().map(|e| format!("{e:#}"));
            }
            self.emit_live();
            let wait = Duration::from_secs(self.settings().poll_seconds.max(15));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.wake.notified() => {}
            }
        }
    }

    async fn cycle(self: &Arc<Self>) -> Result<()> {
        let settings = self.settings();
        if !settings.setup_complete || settings.paused || settings.organization.trim().is_empty() {
            return Ok(());
        }
        self.shared.lock().unwrap().live.checking = true;
        self.emit_live();
        let pat = prr::auth::get_pat()?;
        let org = AdoClient::new(settings.organization.trim(), "", &pat)?;
        let projects = if settings.projects.is_empty() {
            org.list_projects().await.context(
                "Could not list projects. Name the projects to watch in Settings, or use a token with Project & Team (Read) scope",
            )?
        } else {
            settings.projects.clone()
        };

        let mut prs = Vec::new();
        for project in &projects {
            let mut found = org
                .with_project(project)
                .list_prs(&PrFilter::default())
                .await
                .with_context(|| format!("Could not list pull requests in {project}"))?;
            for pr in &mut found {
                if pr.repository.project.name.is_empty() {
                    pr.repository.project.name = project.clone();
                }
            }
            prs.extend(found);
        }
        self.refresh_merged(&org).await;

        let todo = {
            let mut shared = self.shared.lock().unwrap();
            if !shared.tracking.baselined {
                // Turning monitoring on should not suddenly comment on every open PR.
                if !settings.review_existing {
                    for pr in eligible(&prs, &settings.people) {
                        shared.tracking.prs.entry(pr.pull_request_id).or_insert(Tracked {
                            commit: None,
                            outcome: Outcome::Baseline,
                            attempts: 0,
                        });
                    }
                }
                shared.tracking.baselined = true;
                self.store.save_tracking(&shared.tracking)?;
            }
            let todo = plan(&prs, &settings.people, &shared.tracking);
            shared.live.queue = todo.iter().map(|pr| pr_info(pr, &org)).collect();
            shared.live.claimed.clear();
            shared.live.last_check = Some(Utc::now());
            shared.live.error = None;
            // The check itself is over; reviews that follow show as "reviewing".
            shared.live.checking = false;
            todo
        };
        self.emit_live();

        for pr in todo {
            // Settings may have changed while the previous review ran.
            let settings = self.settings();
            if settings.paused {
                break;
            }
            {
                let mut shared = self.shared.lock().unwrap();
                shared.live.queue.retain(|q| q.id != pr.pull_request_id);
            }
            // A dry run changes nothing on Azure DevOps, so it leaves no marker either.
            if !settings.dry_run {
                let forced = self.forced.lock().unwrap().remove(&pr.pull_request_id);
                let mine = self.acquire(&org, &pr_info(&pr, &org), forced).await;
                self.emit_live();
                if !mine {
                    continue;
                }
            }
            self.process(&org, &pat, &settings, pr).await;
        }
        Ok(())
    }

    /// Marks approved PRs as merged once Azure DevOps has completed them.
    async fn refresh_merged(&self, org: &AdoClient) {
        let pending: Vec<(u64, String)> = {
            let shared = self.shared.lock().unwrap();
            let mut seen = std::collections::BTreeSet::new();
            shared
                .history
                .iter()
                .filter(|r| r.auto_complete && !r.merged)
                .filter(|r| seen.insert(r.pr.id))
                .take(25)
                .map(|r| (r.pr.id, r.pr.project.clone()))
                .collect()
        };
        let mut merged = Vec::new();
        for (id, project) in pending {
            if let Ok(pr) = org.with_project(&project).get_pr(id).await {
                if pr.status.eq_ignore_ascii_case("completed") {
                    merged.push(id);
                }
            }
        }
        if merged.is_empty() {
            return;
        }
        {
            let mut shared = self.shared.lock().unwrap();
            for record in shared.history.iter_mut().filter(|r| merged.contains(&r.pr.id)) {
                record.merged = record.auto_complete;
            }
            let _ = self.store.save_history(&shared.history);
        }
        self.emit_history();
    }

    async fn me(&self, org: &AdoClient) -> Result<Identity> {
        if let Some(me) = self.me.lock().unwrap().clone() {
            return Ok(me);
        }
        let me = org.current_user().await?;
        *self.me.lock().unwrap() = Some(me.clone());
        Ok(me)
    }

    async fn my_id(&self, org: &AdoClient) -> Result<String> {
        Ok(self.me(org).await?.id)
    }

    fn instance(&self) -> String {
        self.shared.lock().unwrap().tracking.instance.clone()
    }

    async fn read_claim(client: &AdoClient, pr: &PrInfo) -> Result<Option<Claim>> {
        let properties = client.pr_properties(&pr.repo_id, pr.id).await?;
        Ok(properties.get(CLAIM_KEY).and_then(|text| serde_json::from_str(text).ok()))
    }

    async fn write_claim(&self, org: &AdoClient, pr: &PrInfo, state: &str, decision: Option<&str>) -> Result<()> {
        let me = self.me(org).await?;
        let claim = Claim {
            instance: self.instance(),
            by: if me.display_name.is_empty() { "another copy of Warden".to_string() } else { me.display_name },
            state: state.to_string(),
            commit: pr.commit.clone().unwrap_or_default(),
            at: Utc::now(),
            decision: decision.map(str::to_string),
        };
        org.with_project(&pr.project)
            .set_pr_property(&pr.repo_id, pr.id, CLAIM_KEY, &serde_json::to_string(&claim)?)
            .await
    }

    /// Claims a PR for this copy. Returns false if another copy has it (or
    /// has already reviewed this commit), in which case this copy leaves it.
    /// If the markers cannot be read or written, the review goes ahead
    /// uncoordinated rather than not at all.
    async fn acquire(&self, org: &AdoClient, pr: &PrInfo, forced: bool) -> bool {
        let client = org.with_project(&pr.project);
        let instance = self.instance();
        let commit = pr.commit.clone().unwrap_or_default();
        let busy = |by: String| {
            let mut shared = self.shared.lock().unwrap();
            shared.live.claimed.push(Claimed { pr: pr.clone(), by });
        };

        let existing = match Self::read_claim(&client, pr).await {
            Ok(existing) => existing,
            Err(e) => {
                eprintln!("warden: could not read review markers on PR {}: {e:#}", pr.id);
                return true;
            }
        };
        if !forced {
            match check_claim(existing.as_ref(), &instance, &commit, Utc::now()) {
                ClaimCheck::Free => {}
                ClaimCheck::Busy(by) => {
                    busy(by);
                    return false;
                }
                ClaimCheck::Done(outcome) => {
                    // Remember it, so this commit is not looked up again every check.
                    self.track(pr.id, Tracked { commit: pr.commit.clone(), outcome, attempts: 0 });
                    return false;
                }
            }
        }
        if let Err(e) = self.write_claim(org, pr, "reviewing", None).await {
            eprintln!("warden: could not mark PR {} as being reviewed: {e:#}", pr.id);
            return true;
        }
        // Azure DevOps has no lock, so two copies can both get this far.
        // The last marker written wins; the other copy sees it here and stands down.
        tokio::time::sleep(CLAIM_SETTLE).await;
        match Self::read_claim(&client, pr).await {
            Ok(Some(claim)) if claim.instance != instance && claim.commit == commit && claim.state == "reviewing" => {
                busy(claim.by);
                false
            }
            _ => true,
        }
    }

    async fn process(self: &Arc<Self>, org: &AdoClient, pat: &str, settings: &Settings, pr: PullRequest) {
        let info = pr_info(&pr, org);
        let id = info.id;
        let started = Utc::now();
        {
            let mut shared = self.shared.lock().unwrap();
            shared.live.current = Some(Current {
                pr: info.clone(),
                step: 0,
                started_at: started,
                lines: vec![line(
                    "info",
                    format!("PR {id} by {} · {} → {}", info.author, info.source, info.target),
                )],
                done: false,
                record_id: None,
            });
        }
        self.emit_live();

        let client = org.with_project(&info.project);
        let config = pipeline_config(settings, &info.project);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let me = self.clone();
        let forward = tokio::spawn(async move {
            while let Some(progress) = rx.recv().await {
                match progress {
                    Progress::Stage(stage) => {
                        me.set_step(step_for_stage(&stage));
                        me.log("cmd", stage);
                    }
                    Progress::Activity(activity) => me.log("info", activity),
                }
            }
        });
        let result = pipeline::run_review(&config, &client, pat, id, false, true, &tx).await;
        drop(tx);
        let _ = forward.await;

        let mut record = Record {
            record_id: format!("{id}-{}", started.timestamp_millis()),
            pr: info.clone(),
            status: Outcome::Failed,
            dry_run: settings.dry_run,
            review: None,
            counts: Counts::default(),
            comment: None,
            posted: false,
            vote: None,
            auto_complete: false,
            merged: false,
            stats: None,
            started_at: started,
            finished_at: started,
            duration_secs: 0,
            error: None,
            lines: Vec::new(),
            work_items: Vec::new(),
        };

        match result {
            Err(e) => {
                let message = format!("{e:#}");
                let attempts = {
                    let shared = self.shared.lock().unwrap();
                    shared.tracking.prs.get(&id).filter(|t| t.outcome == Outcome::Failed).map_or(0, |t| t.attempts)
                } + 1;
                self.track(id, Tracked { commit: info.commit.clone(), outcome: Outcome::Failed, attempts });
                let note = if attempts >= MAX_ATTEMPTS {
                    "not retrying automatically".to_string()
                } else {
                    format!("attempt {attempts} of {MAX_ATTEMPTS}, will retry on the next check")
                };
                self.log("warn", format!("Review failed ({note}): {message}"));
                record.error = Some(message);
            }
            Ok(run) => {
                let review = run.saved.review.clone();
                record.pr = pr_info(&run.pr, org);
                if record.pr.project.is_empty() {
                    record.pr.project = info.project.clone();
                    record.pr.url = info.url.clone();
                }
                record.stats = run.stats.map(|(files, additions, deletions)| Stats { files, additions, deletions });
                record.counts = counts(&review);
                self.set_step(3);
                for comment in &review.comments {
                    self.log(&comment.severity.to_string(), issue_line(comment));
                }
                for criterion in review.criteria.iter().filter(|c| c.status != CriterionStatus::Met) {
                    let unmet = criterion.status == CriterionStatus::NotMet;
                    self.log(
                        if unmet { "major" } else { "dim" },
                        format!("{} {}", if unmet { "NOT MET " } else { "UNCLEAR " }, criterion.criterion),
                    );
                }
                let vote = approval_vote(&review);
                let c = record.counts;
                self.log(
                    if vote.is_some() { "ok" } else { "warn" },
                    format!(
                        "Decision → {} ({} critical, {} major, {} minor{})",
                        if vote.is_some() { "approve" } else { "reject" },
                        c.critical,
                        c.major,
                        c.minor,
                        if review.criteria.is_empty() {
                            String::new()
                        } else {
                            format!(
                                " · {} of {} acceptance criteria met",
                                review.criteria.len() - review.criteria.iter().filter(|c| c.status != CriterionStatus::Met).count(),
                                review.criteria.len()
                            )
                        }
                    ),
                );
                record.status = if vote.is_some() { Outcome::Approved } else { Outcome::Rejected };
                // Recorded before anything is posted, so a crash can never post twice.
                self.track(
                    id,
                    Tracked { commit: record.pr.commit.clone().or(info.commit.clone()), outcome: record.status, attempts: 0 },
                );

                let lead = Lead::for_pr(&config.review.comment_prefix, settings.mention_author, &run.pr);
                let comment = flow::single_comment(&review, &lead);
                record.comment = Some(comment.content.clone());
                let mut problems = Vec::new();
                problems.extend(run.work_items_note.clone());
                record.work_items = run
                    .work_items
                    .iter()
                    .map(|w| WorkItemInfo { id: w.id, kind: w.kind.clone(), title: w.title.clone() })
                    .collect();

                self.set_step(4);
                if settings.dry_run {
                    self.log("dim", "Dry run · comment not posted");
                } else {
                    match flow::post_all(&client, &run.pr, vec![comment]).await.pop().map(|r| r.result) {
                        Some(Ok(_)) => {
                            record.posted = true;
                            self.log("info", format!("Comment posted · mentioned {}", info.author));
                        }
                        Some(Err(e)) => {
                            self.log("warn", format!("Could not post the comment: {e:#}"));
                            problems.push(format!("Comment not posted: {e:#}"));
                        }
                        None => {}
                    }
                }

                self.set_step(5);
                if settings.dry_run {
                    self.log("dim", "Dry run · no vote cast");
                } else if !settings.approve_and_complete {
                    self.log("warn", "No vote cast · \"Vote, and auto-complete clean PRs\" is turned off in Settings");
                } else {
                    match self.vote(org, &client, settings, &record.pr, vote).await {
                        Ok((label, auto_complete, message)) => {
                            record.vote = Some(label.to_string());
                            record.auto_complete = auto_complete;
                            self.log(if auto_complete { "ok" } else { "warn" }, message);
                        }
                        Err(e) => {
                            self.log("warn", format!("Could not vote: {e:#}"));
                            problems.push(format!("Vote not cast: {e:#}"));
                        }
                    }
                }
                if !problems.is_empty() {
                    record.error = Some(problems.join("\n"));
                }
                record.review = Some(review);
            }
        }

        if !settings.dry_run {
            // Tell other copies the outcome, or let go of the PR if the review failed.
            let (state, decision) = match record.status {
                Outcome::Approved => ("reviewed", Some("approved")),
                Outcome::Rejected => ("reviewed", Some("rejected")),
                _ => ("released", None),
            };
            if let Err(e) = self.write_claim(org, &record.pr, state, decision).await {
                eprintln!("warden: could not update the review marker on PR {id}: {e:#}");
            }
        }

        record.finished_at = Utc::now();
        record.duration_secs = (record.finished_at - started).num_seconds().max(0) as u64;
        {
            let mut shared = self.shared.lock().unwrap();
            if let Some(current) = shared.live.current.as_mut() {
                current.done = true;
                current.step = 6;
                current.record_id = Some(record.record_id.clone());
                record.lines = current.lines.clone();
            }
            shared.history.insert(0, record);
            shared.history.truncate(MAX_HISTORY);
            if let Err(e) = self.store.save_history(&shared.history) {
                eprintln!("warden: {e:#}");
            }
        }
        self.emit_history();
        self.emit_live();
    }

    /// Approves (and sets auto-complete) or marks "waiting for author".
    /// Returns the vote label, whether auto-complete was set, and a log line.
    async fn vote(
        &self,
        org: &AdoClient,
        client: &AdoClient,
        settings: &Settings,
        pr: &PrInfo,
        vote: Option<i32>,
    ) -> Result<(&'static str, bool, String)> {
        let me = self.my_id(org).await?;
        let Some(value) = vote else {
            client.set_vote(&pr.repo_id, pr.id, &me, VOTE_WAITING).await?;
            return Ok((
                "waitingForAuthor",
                false,
                "Vote set: Waiting for author · will review again when new commits are pushed".to_string(),
            ));
        };
        client.set_vote(&pr.repo_id, pr.id, &me, value).await?;
        let label = if value == 10 { "approved" } else { "approvedWithSuggestions" };
        let body = auto_complete_body(&me, Some(&settings.merge_strategy), settings.delete_source_branch);
        client.set_auto_complete(&pr.repo_id, pr.id, &body).await.context("approved, but could not set auto-complete")?;
        Ok((
            label,
            true,
            format!(
                "Vote set: Approved · auto-complete on ({}{})",
                settings.merge_strategy,
                if settings.delete_source_branch { ", delete source branch" } else { "" }
            ),
        ))
    }

    /// Casts the vote a finished review calls for, on request, without
    /// reviewing again. Refused if the PR has moved on since the review.
    pub async fn apply_vote(&self, record_id: &str) -> Result<()> {
        let (record, settings) = {
            let shared = self.shared.lock().unwrap();
            let record = shared.history.iter().find(|r| r.record_id == record_id).cloned();
            (record.context("That review is no longer in the history")?, shared.settings.clone())
        };
        let review = record.review.as_ref().context("That review did not finish, so there is nothing to vote on")?;
        let pat = prr::auth::get_pat()?;
        let org = AdoClient::new(settings.organization.trim(), "", &pat)?;
        let client = org.with_project(&record.pr.project);

        let current = client.get_pr(record.pr.id).await?;
        if !current.status.is_empty() && !current.status.eq_ignore_ascii_case("active") {
            anyhow::bail!("This pull request is already {}.", current.status);
        }
        let head = current.last_merge_source_commit.map(|c| c.commit_id);
        if record.pr.commit.is_some() && head != record.pr.commit {
            anyhow::bail!("New commits have been pushed since this review. Use \"Review again\" so the vote reflects the current code.");
        }

        let (label, auto_complete, message) =
            self.vote(&org, &client, &settings, &record.pr, approval_vote(review)).await?;
        {
            let mut shared = self.shared.lock().unwrap();
            if let Some(r) = shared.history.iter_mut().find(|r| r.record_id == record_id) {
                r.vote = Some(label.to_string());
                r.auto_complete = auto_complete;
                r.lines.push(line(if auto_complete { "ok" } else { "warn" }, message));
            }
            self.store.save_history(&shared.history)?;
        }
        self.emit_history();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr(id: u64, author: &str, commit: &str, draft: bool) -> PullRequest {
        serde_json::from_value(json!({
            "pullRequestId": id,
            "title": "t",
            "sourceRefName": "refs/heads/s",
            "targetRefName": "refs/heads/main",
            "creationDate": format!("2026-10-01T10:{:02}:00Z", 59 - id),
            "createdBy": { "id": "u", "displayName": author },
            "repository": { "id": "r", "name": "repo", "project": { "name": "Payments" } },
            "lastMergeSourceCommit": { "commitId": commit },
            "isDraft": draft
        }))
        .unwrap()
    }

    fn tracked(commit: &str, outcome: Outcome, attempts: u32) -> Tracked {
        Tracked { commit: Some(commit.into()), outcome, attempts }
    }

    #[test]
    fn new_prs_are_reviewed_and_finished_ones_are_not() {
        let p = pr(1, "a", "c1", false);
        assert!(needs_review(&p, None));
        assert!(!needs_review(&p, Some(&tracked("c1", Outcome::Approved, 0))));
        // New commits after an approval are reviewed, so the approval never covers unseen code.
        assert!(needs_review(&p, Some(&tracked("c0", Outcome::Approved, 0))));
        assert!(!needs_review(&p, Some(&Tracked { commit: None, outcome: Outcome::Baseline, attempts: 0 })));
    }

    #[test]
    fn claims_from_other_copies_are_respected() {
        let now = Utc::now();
        let claim = |instance: &str, state: &str, commit: &str, minutes_ago: i64, decision: Option<&str>| Claim {
            instance: instance.into(),
            by: "Alex".into(),
            state: state.into(),
            commit: commit.into(),
            at: now - chrono::Duration::minutes(minutes_ago),
            decision: decision.map(str::to_string),
        };
        let check = |c: &Claim| check_claim(Some(c), "me", "c1", now);

        assert_eq!(check_claim(None, "me", "c1", now), ClaimCheck::Free);
        assert_eq!(check(&claim("other", "reviewing", "c1", 2, None)), ClaimCheck::Busy("Alex".into()));
        // An abandoned claim, one about an older commit, a released one, or our own does not block.
        assert_eq!(check(&claim("other", "reviewing", "c1", CLAIM_TTL_MINUTES + 1, None)), ClaimCheck::Free);
        assert_eq!(check(&claim("other", "reviewing", "c0", 2, None)), ClaimCheck::Free);
        assert_eq!(check(&claim("other", "released", "c1", 2, None)), ClaimCheck::Free);
        assert_eq!(check(&claim("me", "reviewing", "c1", 2, None)), ClaimCheck::Free);
        assert_eq!(check(&claim("me", "reviewed", "c1", 2, Some("approved"))), ClaimCheck::Free);
        // Someone else already reviewed this exact commit.
        assert_eq!(check(&claim("other", "reviewed", "c1", 500, Some("approved"))), ClaimCheck::Done(Outcome::Approved));
        assert_eq!(check(&claim("other", "reviewed", "c1", 500, Some("rejected"))), ClaimCheck::Done(Outcome::Rejected));
        assert_eq!(check(&claim("other", "reviewed", "c0", 5, Some("approved"))), ClaimCheck::Free);
    }

    #[test]
    fn each_installation_has_a_stable_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {})).instance();
        assert!(!first.is_empty());
        let again = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {})).instance();
        assert_eq!(first, again);
        let other = tempfile::tempdir().unwrap();
        assert_ne!(first, Engine::new(Store::new(other.path().to_path_buf()), Box::new(|_| {})).instance());
    }

    #[test]
    fn rejected_prs_are_reviewed_again_only_after_a_push() {
        let p = pr(1, "a", "c2", false);
        assert!(!needs_review(&p, Some(&tracked("c2", Outcome::Rejected, 0))));
        assert!(needs_review(&p, Some(&tracked("c1", Outcome::Rejected, 0))));
    }

    #[test]
    fn failures_retry_a_limited_number_of_times() {
        let p = pr(1, "a", "c1", false);
        assert!(needs_review(&p, Some(&tracked("c1", Outcome::Failed, MAX_ATTEMPTS - 1))));
        assert!(!needs_review(&p, Some(&tracked("c1", Outcome::Failed, MAX_ATTEMPTS))));
        assert!(needs_review(&p, Some(&tracked("c0", Outcome::Failed, MAX_ATTEMPTS))));
    }

    #[test]
    fn plan_filters_people_and_drafts_and_goes_oldest_first() {
        let prs = vec![
            pr(1, "Shaun Sheppard", "a", false),
            pr(2, "Someone Else", "b", false),
            pr(3, "Shaun Sheppard", "c", true),
            pr(4, "Shaun Sheppard", "d", false),
            pr(5, "Shaun Sheppard", "e", false),
        ];
        let mut tracking = Tracking::default();
        tracking.prs.insert(5, tracked("e", Outcome::Approved, 0));
        let ids = |v: Vec<PullRequest>| v.iter().map(|p| p.pull_request_id).collect::<Vec<_>>();
        assert_eq!(ids(plan(&prs, &["shaun sheppard".into()], &tracking)), [4, 1]);
        assert_eq!(ids(plan(&prs, &[], &tracking)), [4, 2, 1]);
    }

    #[test]
    fn custom_prompt_is_used_only_when_set() {
        let mut settings = Settings::default();
        assert_eq!(pipeline_config(&settings, "p").review.prompt, None);
        settings.review_prompt = "  \n".into();
        assert_eq!(pipeline_config(&settings, "p").review.prompt, None);
        settings.review_prompt = "Only check SQL.".into();
        assert_eq!(pipeline_config(&settings, "p").review.prompt.as_deref(), Some("Only check SQL."));
    }

    #[test]
    fn stages_map_to_ui_steps() {
        assert_eq!(step_for_stage("Fetching PR #12 from Azure DevOps"), 0);
        assert_eq!(step_for_stage("Reading linked work items"), 0);
        assert_eq!(step_for_stage("Cloning repo (first review of this repo)"), 1);
        assert_eq!(step_for_stage("Updating the local clone of repo"), 1);
        assert_eq!(step_for_stage("Claude is reviewing"), 2);
        assert_eq!(step_for_stage("Output was not valid review JSON (x); asking Claude again"), 2);
        assert_eq!(step_for_stage("Saving review"), 3);
    }

    #[test]
    fn settings_and_history_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {}));
        assert_eq!(engine.live().status, "setup");
        let mut settings = engine.settings();
        assert!(!settings.approve_and_complete, "approving must be opt-in");
        settings.organization = "org".into();
        settings.setup_complete = true;
        settings.paused = true;
        engine.update_settings(settings.clone()).unwrap();
        assert_eq!(engine.live().status, "paused");

        let reloaded = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {}));
        assert_eq!(reloaded.settings(), settings);
    }

    #[test]
    fn retry_forgets_the_pr_and_changing_organization_resets_tracking() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {}));
        engine.track(7, tracked("c", Outcome::Rejected, 0));
        engine.track(8, tracked("c", Outcome::Approved, 0));
        engine.retry(7).unwrap();
        assert_eq!(engine.shared.lock().unwrap().tracking.prs.keys().copied().collect::<Vec<_>>(), [8]);

        let mut settings = engine.settings();
        settings.organization = "other".into();
        engine.update_settings(settings).unwrap();
        assert!(engine.shared.lock().unwrap().tracking.prs.is_empty());
    }
}
