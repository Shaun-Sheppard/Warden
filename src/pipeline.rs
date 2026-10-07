//! The review pipeline shared by `prr review` and the full-screen UI:
//! PR details -> code -> Claude -> parsed, saved review. It only reads from
//! Azure DevOps; posting is a separate, confirmed step.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use std::path::PathBuf;
use std::time::Duration;

use crate::ado::{AdoClient, PullRequest, Repository, WorkItem};
use crate::cache::{self, SavedReview};
use crate::config::{self, Config};
use crate::review::{self, PromptInput};
use crate::{claude, git};

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// A new step has started.
    Stage(String),
    /// Something Claude is doing within the current step.
    Activity(String),
}

pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<Progress>;

#[derive(Debug)]
pub struct ReviewRun {
    pub pr: PullRequest,
    pub saved: SavedReview,
    pub path: PathBuf,
    /// (files changed, lines added, lines deleted), when git could tell.
    pub stats: Option<(u32, u32, u32)>,
    /// Linked work items the review was checked against.
    pub work_items: Vec<WorkItem>,
    /// Why linked work items could not be read, if they could not.
    pub work_items_note: Option<String>,
}

/// Directory name for prr's clone of a repo. The id keeps it unique across
/// organizations and renames; the name keeps it recognisable.
pub fn managed_clone_name(repo: &Repository) -> String {
    let safe: String = repo
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let id: String = repo.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    format!("{safe}-{id}")
}

/// Git calls block, so they run off the async threads to keep the UI responsive.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("Background git task failed: {e}"))?
}

/// `stream_activity` makes Claude report its tool use as `Progress::Activity`.
pub async fn run_review(
    config: &Config,
    client: &AdoClient,
    pat: &str,
    id: u64,
    fresh: bool,
    stream_activity: bool,
    tx: &ProgressTx,
) -> Result<ReviewRun> {
    let stage = |text: String| {
        let _ = tx.send(Progress::Stage(text));
    };

    stage(format!("Fetching PR #{id} from Azure DevOps"));
    let pr = client.get_pr(id).await?;
    git::ensure_tool("git")?;
    git::ensure_tool("claude")?;
    let cache_dir = config::cache_dir()?;

    stage("Reading linked work items".to_string());
    let note = |text: String| {
        let _ = tx.send(Progress::Activity(text));
    };
    // The review still runs without them; it just cannot check acceptance criteria.
    let (work_items, work_items_note) = match client.linked_work_items(&pr.repository.id, id).await {
        Ok(items) => {
            if items.is_empty() {
                note("No work items are linked to this pull request".to_string());
            }
            for item in &items {
                let criteria = if item.acceptance_criteria.trim().is_empty() { " · no acceptance criteria" } else { "" };
                note(format!("{} {}: {}{criteria}", item.kind, item.id, item.title));
            }
            (items, None)
        }
        Err(e) => {
            let reason = format!(
                "Linked work items could not be read, so acceptance criteria were not checked. The token needs Work Items (Read) scope. ({e:#})"
            );
            note(reason.clone());
            (Vec::new(), Some(reason))
        }
    };
    let source = pr.source_branch().to_string();
    let target = pr.target_branch().to_string();

    // A repo mapped under [repos] is the user's own clone: refs only, never
    // the working tree. Otherwise prr uses a clone it owns and checks out the PR.
    let (repo_path, checked_out) = match config.repo_path(&pr.repository.name) {
        Some(path) => {
            if fresh {
                bail!(
                    "--fresh only applies to prr's own clone, but `{}` is mapped to {} under [repos].",
                    pr.repository.name,
                    path.display()
                );
            }
            stage(format!("Fetching {source} and {target} in {}", path.display()));
            let (p, s, t) = (path.clone(), source.clone(), target.clone());
            blocking(move || {
                git::ensure_repo(&p)?;
                git::fetch(&p, &s, &t, None)
            })
            .await?;
            (path, false)
        }
        None => {
            let path = cache_dir.join("repos").join(managed_clone_name(&pr.repository));
            if fresh && path.exists() {
                std::fs::remove_dir_all(&path)
                    .with_context(|| format!("Could not remove {}", path.display()))?;
            }
            stage(if path.exists() {
                format!("Updating the local clone of {}", pr.repository.name)
            } else {
                format!("Cloning {} (first review of this repo)", pr.repository.name)
            });
            let url = client.clone_url(&pr.repository.name);
            let (p, s, t, pat) = (path.clone(), source.clone(), target.clone(), pat.to_string());
            blocking(move || {
                git::sync_managed_clone(&url, &p, &s, &t, Some(&git::GitAuth::from_pat(&pat)))
            })
            .await?;
            (path, true)
        }
    };

    // Conventions come from the target branch, not from the PR under review.
    let conventions = git::show_file(&repo_path, &format!("origin/{target}"), "REVIEW.md");
    let inline = config.review.prompt.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let guidance = match (inline, &config.review.prompt_file) {
        (Some(prompt), _) => prompt.to_string(),
        (None, Some(file)) => {
            let path = config::expand_tilde(file);
            std::fs::read_to_string(&path)
                .with_context(|| format!("Could not read review.prompt_file {}", path.display()))?
        }
        (None, None) => review::DEFAULT_GUIDANCE.to_string(),
    };
    let prompt = review::build_prompt(&PromptInput {
        guidance: &guidance,
        pr_id: pr.pull_request_id,
        title: &pr.title,
        description: pr.description.as_deref(),
        source: &source,
        target: &target,
        conventions: conventions.as_deref(),
        work_items: &work_items,
        checked_out,
    });

    let timeout = Duration::from_secs(config.review.timeout_seconds);
    let activity = stream_activity.then_some(tx);
    stage("Claude is reviewing".to_string());
    let raw = claude::run(&repo_path, &prompt, timeout, activity).await?;
    let parsed = match review::parse_review(&raw) {
        Ok(parsed) => parsed,
        Err(first) => {
            stage(format!("Output was not valid review JSON ({first}); asking Claude again"));
            let retry_prompt = review::build_retry_prompt(&raw, &first.to_string());
            let retry_raw = claude::run(&repo_path, &retry_prompt, timeout, activity).await?;
            match review::parse_review(&retry_raw) {
                Ok(parsed) => parsed,
                Err(second) => {
                    let dump = format!("--- first attempt ---\n{raw}\n\n--- retry ---\n{retry_raw}\n");
                    let path = cache::save_raw_failure(&cache_dir, id, &dump)?;
                    bail!(
                        "Could not parse Claude's review after a retry ({second}). Raw output saved to {}",
                        path.display()
                    );
                }
            }
        }
    };

    stage("Saving review".to_string());
    let saved = SavedReview {
        pr_id: pr.pull_request_id,
        repo: pr.repository.name.clone(),
        title: pr.title.clone(),
        source_branch: source,
        target_branch: target,
        source_commit: pr.last_merge_source_commit.as_ref().map(|c| c.commit_id.clone()),
        reviewed_at: Utc::now(),
        review: parsed,
    };
    let path = cache::save(&cache::reviews_dir(&cache_dir), &saved)?;
    let stats = git::diff_stat(&repo_path, &saved.target_branch, &saved.source_branch);
    Ok(ReviewRun { pr, saved, path, stats, work_items, work_items_note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_clone_name_is_filesystem_safe() {
        let repo = Repository {
            id: "ab12-cd34/..".into(),
            name: "My Repo/../x".into(),
            project: Default::default(),
        };
        assert_eq!(managed_clone_name(&repo), "My_Repo____x-ab12-cd34");
    }
}
