//! "Fix with Claude": the user picks issues from a review, Claude edits a
//! private copy of the branch, the user reads the diff, and only then is
//! anything committed and pushed. This is the one place Warden writes code,
//! and it never does so without that approval.

use std::path::PathBuf;

use prr::ado::Repository;
use prr::claude::{self, FIX_TOOLS};
use prr::git::{self, GitAuth};

use super::*;

const FIX_TIMEOUT: Duration = Duration::from_secs(600);
/// Diffs larger than this are cut short in the app (the full change is still pushed).
const MAX_DIFF_BYTES: usize = 300_000;
const MAX_FIXES: usize = 30;

/// Fixes from the last run. One that was mid-flight when Warden quit cannot be resumed.
pub(super) fn load(store: &Store) -> Vec<Fix> {
    let mut fixes = store.fixes();
    for fix in &mut fixes {
        if matches!(fix.status, FixStatus::Generating | FixStatus::Pushing) {
            fix.status = FixStatus::Failed;
            fix.error = Some("Warden was closed before this finished. Discard it and start again.".to_string());
        }
    }
    fixes
}

fn fix_dir(id: &str) -> Result<PathBuf> {
    Ok(prr::config::cache_dir()?.join("fixes").join(id))
}

fn clone_dir(pr: &PrInfo) -> Result<PathBuf> {
    let repo = Repository { id: pr.repo_id.clone(), name: pr.repo.clone(), project: Default::default() };
    Ok(prr::config::cache_dir()?.join("repos").join(pipeline::managed_clone_name(&repo)))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| anyhow::anyhow!("Background git task failed: {e}"))?
}

pub fn fix_prompt(pr: &PrInfo, issues: &[FixIssue]) -> String {
    let mut p = format!(
        "You are fixing specific issues that a code review found in a pull request.\n\
You are in a checkout of the pull request's source branch `{}` (it targets `{}`). Pull request: {}\n\n\
Fix ONLY the issues listed below.\n\
- Make the smallest change that correctly fixes each issue. Do not refactor, reformat or touch unrelated code.\n\
- Follow the conventions of the surrounding code.\n\
- If an issue cannot be fixed safely and with confidence (it needs a design decision, information you do not have, or a large change), leave the code alone for that issue.\n\
- Add or update tests only where an issue asks for it or the fix clearly needs one.\n\
- Do not commit, push, or change git state in any way. Only edit files.\n\
- You cannot build or run tests here, so take care that your edits compile and are complete.\n\n\
Issues:\n",
        pr.source, pr.target, pr.title
    );
    for (i, issue) in issues.iter().enumerate() {
        p.push_str(&format!("\n{}. [{}] {} ({})\n{}\n", i + 1, issue.severity, issue.title, issue.location, issue.body));
    }
    p.push_str(
        "\nWhen you have finished, reply with a short plain-text report and nothing else: one line per issue, \
in the same order, starting with \"Fixed:\" or \"Not fixed:\" and a brief reason.\n",
    );
    p
}

pub fn commit_message(issues: &[FixIssue], approver: &str) -> String {
    let mut message = String::from("Fix issues from Warden review\n\n");
    for issue in issues {
        message.push_str(&format!("- [{}] {} ({})\n", issue.severity, issue.title, issue.location));
    }
    message.push_str(&format!(
        "\nChanges made by Claude Code through Warden; reviewed and approved by {approver} before pushing.\n"
    ));
    message
}

impl Engine {
    pub fn fixes(&self) -> Vec<Fix> {
        self.shared.lock().unwrap().fixes.clone()
    }

    fn emit_fixes(&self) {
        (self.notifier)(Change::Fixes(self.fixes()));
    }

    fn update_fix(&self, id: &str, change: impl FnOnce(&mut Fix)) {
        {
            let mut shared = self.shared.lock().unwrap();
            if let Some(fix) = shared.fixes.iter_mut().find(|f| f.id == id) {
                change(fix);
            }
            if let Err(e) = self.store.save_fixes(&shared.fixes) {
                eprintln!("warden: {e:#}");
            }
        }
        self.emit_fixes();
    }

    fn fix(&self, id: &str) -> Result<Fix> {
        self.shared.lock().unwrap().fixes.iter().find(|f| f.id == id).cloned().context("That fix no longer exists")
    }

    /// Starts preparing a fix for the chosen issues of a review. Returns its id.
    pub fn start_fix(self: &Arc<Self>, record_id: &str, issue_indexes: &[usize]) -> Result<String> {
        let fix = {
            let mut shared = self.shared.lock().unwrap();
            if shared.fixes.iter().any(|f| f.status == FixStatus::Generating) {
                anyhow::bail!("Another fix is being prepared. Wait for it to finish first.");
            }
            let record = shared.history.iter().find(|r| r.record_id == record_id).context("That review is no longer in the history")?;
            let review = record.review.as_ref().context("That review did not finish, so there is nothing to fix")?;
            if shared.fixes.iter().any(|f| f.pr.id == record.pr.id && f.status == FixStatus::Ready) {
                anyhow::bail!("A fix for this pull request is already waiting for your approval. Push or discard it first.");
            }
            let issues: Vec<FixIssue> = issue_indexes
                .iter()
                .filter_map(|&i| review.comments.get(i))
                .map(|c| FixIssue {
                    severity: c.severity.to_string(),
                    title: c.title.clone().unwrap_or_else(|| c.body.lines().next().unwrap_or("").to_string()),
                    location: c.location(),
                    body: match &c.snippet {
                        Some(snippet) => format!("{}\nCode:\n{snippet}", c.body),
                        None => c.body.clone(),
                    },
                })
                .collect();
            if issues.is_empty() {
                anyhow::bail!("Choose at least one issue to fix.");
            }
            let now = Utc::now();
            let fix = Fix {
                id: format!("{}-{}", record.pr.id, now.timestamp_millis()),
                record_id: record_id.to_string(),
                pr: record.pr.clone(),
                status: FixStatus::Generating,
                issues,
                base_commit: None,
                diff: String::new(),
                diff_truncated: false,
                files: Vec::new(),
                report: String::new(),
                error: None,
                commit: None,
                started_at: now,
                finished_at: None,
                lines: Vec::new(),
            };
            shared.fixes.insert(0, fix.clone());
            // Finished fixes are kept for reference; ones awaiting a decision are never dropped.
            let mut kept = 0;
            shared.fixes.retain(|f| {
                kept += 1;
                kept <= MAX_FIXES || matches!(f.status, FixStatus::Ready | FixStatus::Generating | FixStatus::Pushing)
            });
            let _ = self.store.save_fixes(&shared.fixes);
            fix
        };
        self.emit_fixes();

        let me = self.clone();
        let id = fix.id.clone();
        tokio::spawn(async move {
            if let Err(e) = me.generate(&id).await {
                let message = format!("{e:#}");
                if let Ok(fix) = me.fix(&id) {
                    if let (Ok(clone), Ok(work)) = (clone_dir(&fix.pr), fix_dir(&id)) {
                        git::worktree_remove(&clone, &work);
                    }
                }
                me.update_fix(&id, |f| {
                    f.status = FixStatus::Failed;
                    f.error = Some(message);
                    f.finished_at = Some(Utc::now());
                });
            }
        });
        Ok(fix.id)
    }

    fn fix_log(&self, id: &str, kind: &str, text: impl Into<String>) {
        let entry = line(kind, text);
        self.update_fix(id, |f| {
            if f.lines.len() < MAX_LINES {
                f.lines.push(entry);
            }
        });
    }

    async fn generate(self: &Arc<Self>, id: &str) -> Result<()> {
        let fix = self.fix(id)?;
        let settings = self.settings();
        let pat = prr::auth::get_pat()?;
        let org = AdoClient::new(settings.organization.trim(), "", &pat)?;
        let url = org.with_project(&fix.pr.project).clone_url(&fix.pr.repo);
        let (clone, work) = (clone_dir(&fix.pr)?, fix_dir(id)?);
        git::ensure_tool("git")?;
        git::ensure_tool("claude")?;

        self.fix_log(id, "cmd", "Getting the latest code for the branch");
        let base = {
            let (clone, work, pat) = (clone.clone(), work.clone(), pat.clone());
            let (source, target) = (fix.pr.source.clone(), fix.pr.target.clone());
            blocking(move || {
                let auth = GitAuth::from_pat(&pat);
                if clone.exists() {
                    // Only refs are updated here; the clone's files may be in use by a review.
                    git::fetch(&clone, &source, &target, Some(&auth))?;
                } else {
                    git::sync_managed_clone(&url, &clone, &source, &target, Some(&auth))?;
                }
                git::worktree_remove(&clone, &work);
                git::worktree_add(&clone, &work, &format!("origin/{source}"))?;
                git::head(&work)
            })
            .await?
        };
        if fix.pr.commit.as_deref().is_some_and(|reviewed| reviewed != base) {
            anyhow::bail!(
                "New commits have been pushed to this pull request since the review. Warden will review them; start the fix from that new review."
            );
        }
        self.update_fix(id, |f| f.base_commit = Some(base.clone()));

        self.fix_log(id, "cmd", format!("Claude is fixing {} issue(s)", fix.issues.len()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (me, fix_id) = (self.clone(), id.to_string());
        let forward = tokio::spawn(async move {
            while let Some(progress) = rx.recv().await {
                if let Progress::Activity(activity) = progress {
                    me.fix_log(&fix_id, "info", activity);
                }
            }
        });
        let result = claude::run_with(&work, &fix_prompt(&fix.pr, &fix.issues), FIX_TIMEOUT, Some(&tx), FIX_TOOLS, &[]).await;
        drop(tx);
        let _ = forward.await;
        let report = result?;

        let (mut diff, files) = {
            let work = work.clone();
            blocking(move || git::pending_changes(&work)).await?
        };
        let nothing = files.is_empty();
        if nothing {
            git::worktree_remove(&clone, &work);
        }
        let truncated = diff.len() > MAX_DIFF_BYTES;
        if truncated {
            let mut cut = MAX_DIFF_BYTES;
            while !diff.is_char_boundary(cut) {
                cut -= 1;
            }
            diff.truncate(cut);
        }
        let summary = format!(
            "{} file(s) changed, +{} −{}",
            files.len(),
            files.iter().map(|f| f.additions).sum::<u32>(),
            files.iter().map(|f| f.deletions).sum::<u32>()
        );
        self.fix_log(id, if nothing { "warn" } else { "ok" }, if nothing { "Claude made no changes".to_string() } else { summary.clone() });
        self.update_fix(id, |f| {
            f.status = if nothing { FixStatus::NoChanges } else { FixStatus::Ready };
            f.report = report.trim().to_string();
            f.diff = diff;
            f.diff_truncated = truncated;
            f.files = files.into_iter().map(|c| FixFile { path: c.path, additions: c.additions, deletions: c.deletions }).collect();
            f.finished_at = Some(Utc::now());
        });
        if nothing {
            self.notify("Fix: no changes made", format!("PR {}: {}\nClaude did not change anything.", fix.pr.id, fix.pr.title));
        } else {
            self.notify("Fix ready for your review", format!("PR {}: {}\n{summary}. Nothing is pushed until you approve.", fix.pr.id, fix.pr.title));
        }
        Ok(())
    }

    /// Commits an approved fix to the pull request's branch and pushes it.
    pub async fn push_fix(&self, id: &str) -> Result<()> {
        let fix = self.fix(id)?;
        if fix.status != FixStatus::Ready {
            anyhow::bail!("This fix is not waiting for approval.");
        }
        let settings = self.settings();
        if settings.dry_run {
            anyhow::bail!("Dry run is on, so nothing can be pushed. Turn it off in Settings first.");
        }
        self.update_fix(id, |f| {
            f.status = FixStatus::Pushing;
            f.error = None;
        });
        match self.push_ready_fix(&fix, &settings).await {
            Ok(commit) => {
                self.update_fix(id, |f| {
                    f.status = FixStatus::Pushed;
                    f.commit = Some(commit);
                    f.finished_at = Some(Utc::now());
                });
                // The new commit is picked up and reviewed like any other push.
                self.check_now();
                Ok(())
            }
            Err(e) => {
                let message = format!("{e:#}");
                self.update_fix(id, |f| {
                    f.status = FixStatus::Ready;
                    f.error = Some(message);
                });
                Err(e)
            }
        }
    }

    async fn push_ready_fix(&self, fix: &Fix, settings: &Settings) -> Result<String> {
        let pat = prr::auth::get_pat()?;
        let org = AdoClient::new(settings.organization.trim(), "", &pat)?;
        let me = self.me(&org).await?;
        let (clone, work) = (clone_dir(&fix.pr)?, fix_dir(&fix.id)?);
        let base = fix.base_commit.clone().context("This fix has no recorded starting point; discard it and start again")?;
        let name = if me.display_name.is_empty() { "Warden user".to_string() } else { me.display_name.clone() };
        let email = Some(me.unique_name.clone())
            .filter(|e| e.contains('@'))
            .or_else(git::configured_email)
            .unwrap_or_else(|| format!("{}@users.noreply.dev.azure.com", me.id));
        let message = commit_message(&fix.issues, &name);
        let source = fix.pr.source.clone();

        blocking(move || {
            let auth = GitAuth::from_pat(&pat);
            if !work.is_dir() {
                anyhow::bail!("The prepared changes are no longer on disk. Discard this fix and start again.");
            }
            if git::remote_head(&clone, &source, Some(&auth))?.as_deref() != Some(base.as_str()) {
                anyhow::bail!(
                    "The branch has new commits since this fix was prepared, so it was not pushed. Discard it and fix from the new review."
                );
            }
            // A retry after a failed push finds the commit already made.
            let commit = if git::head(&work)? == base { git::commit(&work, &name, &email, &message)? } else { git::head(&work)? };
            git::push_head(&work, &source, Some(&auth))?;
            git::worktree_remove(&clone, &work);
            Ok(commit)
        })
        .await
    }

    /// Throws a fix away, with its working directory. Nothing was ever pushed.
    pub fn discard_fix(&self, id: &str) -> Result<()> {
        let fix = self.fix(id)?;
        if matches!(fix.status, FixStatus::Generating | FixStatus::Pushing) {
            anyhow::bail!("This fix is still in progress.");
        }
        if let (Ok(clone), Ok(work)) = (clone_dir(&fix.pr), fix_dir(id)) {
            git::worktree_remove(&clone, &work);
        }
        {
            let mut shared = self.shared.lock().unwrap();
            shared.fixes.retain(|f| f.id != id);
            self.store.save_fixes(&shared.fixes)?;
        }
        self.emit_fixes();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(severity: &str, title: &str) -> FixIssue {
        FixIssue { severity: severity.into(), title: title.into(), location: "src/a.cs:42".into(), body: "Explain.".into() }
    }

    fn pr_info() -> PrInfo {
        PrInfo {
            id: 7,
            title: "Add cache".into(),
            project: "Payments".into(),
            repo: "api".into(),
            repo_id: "rid".into(),
            author: "Ann".into(),
            source: "feature/x".into(),
            target: "main".into(),
            url: String::new(),
            commit: Some("c1".into()),
        }
    }

    #[test]
    fn prompt_lists_the_issues_and_forbids_git_changes() {
        let p = fix_prompt(&pr_info(), &[issue("major", "Null deref"), issue("minor", "Typo")]);
        assert!(p.contains("source branch `feature/x`"));
        assert!(p.contains("1. [major] Null deref (src/a.cs:42)\nExplain."));
        assert!(p.contains("2. [minor] Typo"));
        assert!(p.contains("Do not commit, push"));
        assert!(p.contains("Fix ONLY the issues listed"));
    }

    #[test]
    fn commit_message_names_the_issues_and_the_approver() {
        let m = commit_message(&[issue("major", "Null deref")], "Shaun Sheppard");
        assert!(m.starts_with("Fix issues from Warden review\n\n- [major] Null deref (src/a.cs:42)\n"));
        assert!(m.contains("reviewed and approved by Shaun Sheppard before pushing"));
    }

    #[test]
    fn interrupted_fixes_are_marked_failed_on_start_and_can_be_discarded() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let fix = |id: &str, status| Fix {
            id: id.into(),
            record_id: "r".into(),
            pr: pr_info(),
            status,
            issues: vec![issue("major", "x")],
            base_commit: None,
            diff: String::new(),
            diff_truncated: false,
            files: vec![],
            report: String::new(),
            error: None,
            commit: None,
            started_at: Utc::now(),
            finished_at: None,
            lines: vec![],
        };
        store.save_fixes(&[fix("a", FixStatus::Generating), fix("b", FixStatus::Ready), fix("c", FixStatus::Pushing)]).unwrap();

        let engine = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {}));
        let statuses: Vec<_> = engine.fixes().iter().map(|f| f.status).collect();
        assert_eq!(statuses, [FixStatus::Failed, FixStatus::Ready, FixStatus::Failed]);
        assert!(engine.fixes()[0].error.as_deref().unwrap().contains("closed before this finished"));

        engine.discard_fix("a").unwrap();
        assert_eq!(engine.fixes().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["b", "c"]);
        assert!(engine.discard_fix("nope").is_err());
    }

    #[tokio::test]
    async fn pushing_needs_a_ready_fix_and_dry_run_off() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::new(Store::new(tmp.path().to_path_buf()), Box::new(|_| {}));
        assert!(engine.push_fix("missing").await.is_err());
    }
}
