//! Unattended mode: poll for new pull requests, review each once, and post
//! the review without prompting. This is the only path in prr that writes to
//! Azure DevOps without a per-comment confirmation, so it is opt-in
//! (`prr auto`) and remembers what it has handled to never post twice.

use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ado::{author_matches, auto_complete_body, AdoClient, PrFilter, PullRequest};
use crate::config::Config;
use crate::flow::{self, Lead};
use crate::pipeline::{self, Progress};
use crate::review::{Review, Severity, Verdict};

/// A PR whose review keeps failing is left alone after this many attempts.
pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoState {
    /// PRs already reviewed (or deliberately skipped); never touched again.
    #[serde(default)]
    pub handled: BTreeSet<u64>,
    /// Failed review attempts per PR.
    #[serde(default)]
    pub failures: BTreeMap<u64, u32>,
}

impl AutoState {
    /// None if auto mode has never run.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .with_context(|| format!("Auto-mode state file {} is corrupt", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("Could not read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Could not create {}", dir.display()))?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("Could not write {}", path.display()))
    }
}

/// PRs auto mode may review at all: not drafts, and raised by a configured author.
pub fn eligible<'a>(prs: &'a [PullRequest], authors: &[String]) -> Vec<&'a PullRequest> {
    prs.iter()
        .filter(|pr| !pr.is_draft && author_matches(authors, &pr.created_by))
        .collect()
}

/// Eligible PRs that still need a review.
pub fn pending<'a>(eligible: &[&'a PullRequest], state: &AutoState) -> Vec<&'a PullRequest> {
    eligible
        .iter()
        .copied()
        .filter(|pr| {
            let id = pr.pull_request_id;
            !state.handled.contains(&id)
                && state.failures.get(&id).copied().unwrap_or(0) < MAX_ATTEMPTS
        })
        .collect()
}

/// The vote auto mode would cast: none if the review rejects the PR or
/// found any critical or major issue; otherwise 10 (approved) for a clean
/// review or 5 (approved with suggestions) when only minor issues remain.
pub fn approval_vote(review: &Review) -> Option<i32> {
    let blocking = review.verdict == Verdict::ChangesRequested
        || review.comments.iter().any(|c| c.severity != Severity::Minor)
        || review.unmet_criteria() > 0;
    if blocking {
        None
    } else if review.comments.is_empty() {
        Some(10)
    } else {
        Some(5)
    }
}

#[derive(Debug, Clone)]
pub struct AutoOptions {
    pub interval: Duration,
    pub once: bool,
    pub dry_run: bool,
    /// Also review PRs that were already open the first time auto mode runs.
    pub include_existing: bool,
    /// Approve PRs with no critical or major issues.
    pub approve: bool,
    /// Set those PRs to auto-complete.
    pub autocomplete: bool,
}

fn log(message: &str) {
    println!("[{}] {message}", Local::now().format("%H:%M:%S"));
}

pub struct Runner {
    state_path: PathBuf,
    state: AutoState,
    first_run: bool,
    /// The signed-in user's identity id, looked up when first needed.
    my_id: Option<String>,
}

impl Runner {
    pub fn new(state_path: PathBuf) -> Result<Self> {
        let loaded = AutoState::load(&state_path)?;
        Ok(Self {
            first_run: loaded.is_none(),
            state: loaded.unwrap_or_default(),
            state_path,
            my_id: None,
        })
    }

    pub fn state(&self) -> &AutoState {
        &self.state
    }

    fn persist(&self, opts: &AutoOptions) -> Result<()> {
        // A dry run must leave no trace, so a later real run still reviews everything.
        if opts.dry_run {
            return Ok(());
        }
        self.state.save(&self.state_path)
    }

    /// One poll: list PRs, review and post for each new one.
    pub async fn cycle(
        &mut self,
        config: &Config,
        client: &AdoClient,
        pat: &str,
        opts: &AutoOptions,
    ) -> Result<()> {
        let filter = PrFilter { repo: config.default_repo.clone(), ..Default::default() };
        let prs = client.list_prs(&filter).await?;
        let eligible = eligible(&prs, &config.authors);

        if std::mem::take(&mut self.first_run) && !opts.include_existing {
            // Starting auto mode should not suddenly comment on every open PR.
            self.state.handled.extend(eligible.iter().map(|pr| pr.pull_request_id));
            self.persist(opts)?;
            log(&format!(
                "First run: {} pull request(s) already open were left alone (use --include-existing to review them). Watching for new ones.",
                eligible.len()
            ));
            return Ok(());
        }

        let todo: Vec<PullRequest> = pending(&eligible, &self.state).into_iter().cloned().collect();
        if todo.is_empty() {
            log("No new pull requests.");
            return Ok(());
        }
        log(&format!("{} new pull request(s).", todo.len()));

        for pr in todo {
            let id = pr.pull_request_id;
            log(&format!("Reviewing #{id} {} ({})", pr.title, pr.created_by.display_name));

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let stages = tokio::spawn(async move {
                while let Some(progress) = rx.recv().await {
                    if let Progress::Stage(stage) = progress {
                        log(&format!("  {stage}"));
                    }
                }
            });
            let result = pipeline::run_review(config, client, pat, id, false, false, &tx).await;
            drop(tx);
            let _ = stages.await;

            let run = match result {
                Ok(run) => run,
                Err(e) => {
                    let attempts = self.state.failures.entry(id).or_insert(0);
                    *attempts += 1;
                    let note = if *attempts >= MAX_ATTEMPTS {
                        "giving up on this PR".to_string()
                    } else {
                        format!("attempt {attempts} of {MAX_ATTEMPTS}, will retry next cycle")
                    };
                    log(&format!("  Review of #{id} failed ({note}): {e:#}"));
                    self.persist(opts)?;
                    continue;
                }
            };

            let review = &run.saved.review;
            log(&format!(
                "  Decision: {} · {} issue(s)",
                review.verdict,
                review.comments.len()
            ));
            // Recorded before posting, so a crash part-way can never post twice.
            self.state.handled.insert(id);
            self.state.failures.remove(&id);
            self.persist(opts)?;

            let lead = Lead::for_pr(&config.review.comment_prefix, config.review.mention_author, &run.pr);
            // One comment per PR, so a long review is a single notification.
            let comment = flow::single_comment(review, &lead);
            if opts.dry_run {
                log("  Dry run: would post one comment with the decision and all issues. Nothing posted.");
                continue;
            }
            let results = flow::post_all(client, &run.pr, vec![comment]).await;
            match results.first().map(|r| &r.result) {
                Some(Ok(_)) => log(&format!("  Posted the review comment: {}", client.pr_web_url(&run.pr))),
                Some(Err(e)) => log(&format!("  Failed to post the review comment: {e:#}")),
                None => {}
            }
            if opts.approve || opts.autocomplete {
                if let Err(e) = self.approve_if_clean(config, client, &run.pr, review, opts).await {
                    log(&format!("  Could not approve / auto-complete #{id}: {e:#}"));
                }
            }
        }
        Ok(())
    }

    async fn approve_if_clean(
        &mut self,
        config: &Config,
        client: &AdoClient,
        pr: &PullRequest,
        review: &Review,
        opts: &AutoOptions,
    ) -> Result<()> {
        let Some(vote) = approval_vote(review) else {
            log("  Not approved: the review found critical or major issues.");
            return Ok(());
        };
        let me = match &self.my_id {
            Some(id) => id.clone(),
            None => {
                let id = client.current_user().await?.id;
                self.my_id = Some(id.clone());
                id
            }
        };
        let (repo, id) = (&pr.repository.id, pr.pull_request_id);
        if opts.approve {
            client.set_vote(repo, id, &me, vote).await?;
            log(if vote == 10 { "  Approved." } else { "  Approved with suggestions." });
        }
        if opts.autocomplete {
            let body = auto_complete_body(
                &me,
                config.auto.merge_strategy.as_deref(),
                config.auto.delete_source_branch,
            );
            client.set_auto_complete(repo, id, &body).await?;
            log("  Set to auto-complete.");
        }
        Ok(())
    }
}

pub async fn run(
    config: &Config,
    client: &AdoClient,
    pat: &str,
    state_path: PathBuf,
    opts: &AutoOptions,
) -> Result<()> {
    let mut runner = Runner::new(state_path)?;
    loop {
        if let Err(e) = runner.cycle(config, client, pat, opts).await {
            if opts.once {
                return Err(e);
            }
            log(&format!("Check failed, will try again: {e:#}"));
        }
        if opts.once {
            return Ok(());
        }
        tokio::time::sleep(opts.interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr(id: u64, author: &str, draft: bool) -> PullRequest {
        serde_json::from_value(json!({
            "pullRequestId": id,
            "title": "t",
            "sourceRefName": "refs/heads/s",
            "targetRefName": "refs/heads/main",
            "creationDate": "2026-10-01T10:00:00Z",
            "createdBy": { "id": "u", "displayName": author },
            "repository": { "id": "r", "name": "repo" },
            "isDraft": draft
        }))
        .unwrap()
    }

    #[test]
    fn eligibility_skips_drafts_and_other_authors() {
        let prs = vec![pr(1, "Shaun Sheppard", false), pr(2, "Shaun Sheppard", true), pr(3, "Ann", false)];
        let ids = |v: Vec<&PullRequest>| v.iter().map(|p| p.pull_request_id).collect::<Vec<_>>();
        assert_eq!(ids(eligible(&prs, &[])), [1, 3]);
        assert_eq!(ids(eligible(&prs, &["shaun sheppard".to_string()])), [1]);
    }

    #[test]
    fn pending_excludes_handled_and_repeatedly_failing_prs() {
        let prs = vec![pr(1, "a", false), pr(2, "a", false), pr(3, "a", false), pr(4, "a", false)];
        let mut state = AutoState::default();
        state.handled.insert(1);
        state.failures.insert(2, MAX_ATTEMPTS);
        state.failures.insert(3, MAX_ATTEMPTS - 1);
        let todo = pending(&eligible(&prs, &[]), &state);
        assert_eq!(todo.iter().map(|p| p.pull_request_id).collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn only_reviews_without_critical_or_major_issues_are_approved() {
        let review = |verdict: &str, severities: &[&str]| {
            let comments: Vec<_> = severities
                .iter()
                .map(|s| json!({ "severity": s, "body": "b" }))
                .collect();
            crate::review::parse_review(
                &json!({ "verdict": verdict, "summary": "s", "comments": comments }).to_string(),
            )
            .unwrap()
        };
        assert_eq!(approval_vote(&review("approve", &[])), Some(10));
        assert_eq!(approval_vote(&review("approve_with_suggestions", &["minor", "minor"])), Some(5));
        assert_eq!(approval_vote(&review("approve_with_suggestions", &["minor", "major"])), None);
        assert_eq!(approval_vote(&review("approve", &["critical"])), None);
        // An unmet acceptance criterion blocks approval on its own.
        let mut unmet = review("approve", &[]);
        unmet.criteria = crate::review::parse_review(
            r#"{"verdict":"approve","summary":"s","criteria":[{"criterion":"c","status":"not_met"}]}"#,
        )
        .unwrap()
        .criteria;
        assert_eq!(approval_vote(&unmet), None);
        unmet.criteria[0].status = crate::review::CriterionStatus::Unclear;
        assert_eq!(approval_vote(&unmet), Some(10));
        // A rejection is never overridden, whatever the listed issues.
        assert_eq!(approval_vote(&review("changes_requested", &[])), None);
        assert_eq!(approval_vote(&review("changes_requested", &["minor"])), None);
    }

    #[test]
    fn state_round_trips_and_missing_file_means_first_run() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/auto-state.json");
        assert!(AutoState::load(&path).unwrap().is_none());
        let mut state = AutoState::default();
        state.handled.insert(7);
        state.failures.insert(9, 2);
        state.save(&path).unwrap();
        assert_eq!(AutoState::load(&path).unwrap(), Some(state));
    }
}
