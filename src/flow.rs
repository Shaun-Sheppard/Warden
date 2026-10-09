//! The post-review flow: per-comment choice, final confirmation, posting.
//! Nothing here writes to Azure DevOps unless a comment was individually
//! approved AND the final confirmation was answered yes.

use anyhow::Result;

use crate::ado::{thread_body, AdoClient, PullRequest};
use crate::review::{Comment, CriterionStatus, Review};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Post,
    Edit,
    Skip,
    Quit,
}

/// User interaction, abstracted so the flow can be tested without a terminal.
pub trait Prompter {
    /// Show a candidate comment (heading + body) before asking about it.
    fn show(&mut self, heading: &str, body: &str);
    /// `[p]ost / [e]dit / [s]kip / [q]uit`
    fn choose(&mut self) -> Result<Choice>;
    /// Open the text in an editor. None means the edit was abandoned.
    fn edit(&mut self, text: &str) -> Result<Option<String>>;
    /// Yes/no question defaulting to No.
    fn confirm(&mut self, question: &str) -> Result<bool>;
    fn info(&mut self, message: &str);
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    pub label: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Final content, including the marker prefix.
    pub content: String,
    /// Post the thread already closed. An open thread can hold up a merge
    /// where branch policy requires every comment to be resolved.
    pub closed: bool,
}

#[derive(Debug)]
pub struct PostResult {
    pub label: String,
    pub result: Result<u64>,
}

#[derive(Debug)]
pub enum Outcome {
    /// `--dry-run`: no prompts, no writes.
    DryRun,
    /// Every comment was skipped.
    NothingSelected,
    /// The user quit part-way through; nothing is posted.
    Quit,
    /// The final confirmation was declined.
    Declined,
    Posted(Vec<PostResult>),
}

/// What goes in front of every posted comment: the marker, then the
/// author's mention if they are being tagged.
#[derive(Debug, Clone, Default)]
pub struct Lead {
    pub prefix: String,
    pub mention: Option<String>,
}

impl Lead {
    pub fn for_pr(prefix: &str, mention_author: bool, pr: &PullRequest) -> Self {
        Self {
            prefix: prefix.to_string(),
            mention: mention_author.then(|| pr.author_mention()).flatten(),
        }
    }

    fn apply(&self, text: &str) -> String {
        [self.prefix.trim(), self.mention.as_deref().unwrap_or(""), text]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn comment_content(lead: &Lead, comment: &Comment, body: &str) -> String {
    lead.apply(&format!("**[{}]** {}", comment.severity, body))
}

pub fn summary_body(review: &Review) -> String {
    format!("**Decision: {}**\n\n{}", review.verdict, review.summary)
}

/// Ask about one candidate. Ok(None) = skipped, Err(Quit) is modelled as Ok(Err(())).
fn ask(prompter: &mut dyn Prompter, heading: &str, initial: &str) -> Result<Result<Option<String>, ()>> {
    let mut body = initial.to_string();
    prompter.show(heading, &body);
    loop {
        match prompter.choose()? {
            Choice::Post => return Ok(Ok(Some(body))),
            Choice::Skip => return Ok(Ok(None)),
            Choice::Quit => return Ok(Err(())),
            Choice::Edit => {
                match prompter.edit(&body)? {
                    Some(edited) if !edited.trim().is_empty() => {
                        body = edited.trim().to_string();
                        prompter.show(&format!("{heading} (edited)"), &body);
                    }
                    Some(_) => prompter.info("Edited text is empty; keeping the previous text."),
                    None => prompter.info("Edit cancelled; keeping the previous text."),
                }
            }
        }
    }
}

/// Walks the user through each comment (and the optional summary comment).
/// Returns None if the user quit.
pub fn select_comments(
    review: &Review,
    lead: &Lead,
    prompter: &mut dyn Prompter,
) -> Result<Option<Vec<Outgoing>>> {
    let mut approved = Vec::new();
    let total = review.comments.len();
    for (i, comment) in review.comments.iter().enumerate() {
        let label = format!("[{}/{}] {} · {}", i + 1, total, comment.severity, comment.location());
        match ask(prompter, &label, &comment.body)? {
            Err(()) => return Ok(None),
            Ok(None) => {}
            Ok(Some(body)) => approved.push(Outgoing {
                label,
                file: comment.file.clone(),
                line: comment.line,
                content: comment_content(lead, comment, &body),
                closed: false,
            }),
        }
    }

    let label = "Overall summary comment".to_string();
    match ask(prompter, &label, &summary_body(review))? {
        Err(()) => return Ok(None),
        Ok(None) => {}
        Ok(Some(body)) => approved.push(Outgoing {
            label,
            file: None,
            line: None,
            content: lead.apply(&body),
            closed: false,
        }),
    }
    Ok(Some(approved))
}

pub async fn post_flow(
    client: &AdoClient,
    pr: &PullRequest,
    review: &Review,
    prefix: &str,
    mention_author: bool,
    dry_run: bool,
    prompter: &mut dyn Prompter,
) -> Result<Outcome> {
    if dry_run {
        return Ok(Outcome::DryRun);
    }

    let lead = Lead::for_pr(prefix, mention_author, pr);
    if lead.mention.is_some() {
        prompter.info(&format!(
            "\nPosted comments will tag the PR author, {}.",
            pr.created_by.display_name
        ));
    }
    let Some(approved) = select_comments(review, &lead, prompter)? else {
        return Ok(Outcome::Quit);
    };
    if approved.is_empty() {
        return Ok(Outcome::NothingSelected);
    }

    prompter.info("\nAbout to post:");
    for item in &approved {
        prompter.info(&format!("  • {}", item.label));
    }
    let n = approved.len();
    let question = format!(
        "Post {n} comment{} to PR #{}?",
        if n == 1 { "" } else { "s" },
        pr.pull_request_id
    );
    if !prompter.confirm(&question)? {
        return Ok(Outcome::Declined);
    }

    Ok(Outcome::Posted(post_all(client, pr, approved).await))
}

/// The whole review as one PR-level comment: decision, summary, then the
/// issues grouped by severity with their locations. Used by auto mode,
/// which posts without asking.
pub fn single_comment(review: &Review, lead: &Lead) -> Outgoing {
    let mut text = summary_body(review);
    if !review.criteria.is_empty() {
        text.push_str(&format!(
            "\n\n**Acceptance criteria ({} of {} met)**\n",
            review.criteria.iter().filter(|c| c.status == CriterionStatus::Met).count(),
            review.criteria.len()
        ));
        for c in &review.criteria {
            let mark = match c.status {
                CriterionStatus::Met => "✅",
                CriterionStatus::NotMet => "❌",
                CriterionStatus::Unclear => "❓",
            };
            let item = c.work_item.map(|id| format!("#{id} ")).unwrap_or_default();
            let note = c.note.as_deref().map(|n| format!(" — {n}")).unwrap_or_default();
            text.push_str(&format!("\n- {mark} {item}{}{note}", c.criterion));
        }
    }
    for (severity, comments) in review.grouped() {
        text.push_str(&format!("\n\n**{} ({})**\n", severity.heading(), comments.len()));
        for (number, comment) in comments {
            // Keep multi-line bodies inside their list item.
            let body = comment.body.replace('\n', "\n   ");
            let body = match &comment.title {
                Some(title) => format!("**{title}** {body}"),
                None => body,
            };
            match &comment.file {
                Some(_) => text.push_str(&format!("\n{number}. `{}` — {body}", comment.location())),
                None => text.push_str(&format!("\n{number}. {body}")),
            }
        }
    }
    Outgoing {
        label: "Review comment".to_string(),
        file: None,
        line: None,
        content: lead.apply(&text),
        closed: false,
    }
}

/// Posts each item as its own thread. Callers are responsible for consent.
pub async fn post_all(client: &AdoClient, pr: &PullRequest, items: Vec<Outgoing>) -> Vec<PostResult> {
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let mut body = thread_body(&item.content, item.file.as_deref(), item.line);
        if item.closed {
            body["status"] = serde_json::json!("closed");
        }
        let result = client
            .post_thread(&pr.repository.id, pr.pull_request_id, &body)
            .await;
        results.push(PostResult { label: item.label, result });
    }
    results
}
