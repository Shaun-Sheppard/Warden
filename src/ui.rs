use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use comfy_table::{presets::UTF8_FULL_CONDENSED, ContentArrangement, Table};
use console::style;
use std::io::{BufRead, Write};

use crate::ado::{vote_label, PullRequest};
use crate::flow::{Choice, Outcome, Prompter};
use crate::review::{CriterionStatus, Review, Severity, Verdict};

pub fn format_age(created: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let d = now.signed_duration_since(created);
    if d.num_days() >= 1 {
        format!("{}d", d.num_days())
    } else if d.num_hours() >= 1 {
        format!("{}h", d.num_hours())
    } else {
        format!("{}m", d.num_minutes().max(0))
    }
}

pub fn pr_table(prs: &[PullRequest], my_id: &str, now: DateTime<Utc>) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["ID", "Repo", "Title", "Author", "Branches", "Age", "My vote"]);
    for pr in prs {
        table.add_row(vec![
            pr.pull_request_id.to_string(),
            pr.repository.name.clone(),
            pr.title.clone(),
            pr.created_by.display_name.clone(),
            format!("{} → {}", pr.source_branch(), pr.target_branch()),
            format_age(pr.creation_date, now),
            vote_label(pr.vote_of(my_id)).to_string(),
        ]);
    }
    table
}

fn severity_tag(severity: Severity) -> String {
    let text = severity.heading();
    match severity {
        Severity::Critical => style(text).red().bold().to_string(),
        Severity::Major => style(text).yellow().bold().to_string(),
        Severity::Minor => style(text).cyan().to_string(),
    }
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

pub fn print_review(review: &Review) {
    let decision = match review.verdict {
        Verdict::Approve => style(review.verdict.to_string()).green().bold(),
        Verdict::ApproveWithSuggestions => style(review.verdict.to_string()).yellow().bold(),
        Verdict::ChangesRequested => style(review.verdict.to_string()).red().bold(),
    };
    println!("\n{} {}", style("Decision:").bold(), decision);
    println!("{}", indent(&review.summary));

    if !review.criteria.is_empty() {
        println!("\n{}", style("Acceptance criteria").bold());
        for c in &review.criteria {
            let mark = match c.status {
                CriterionStatus::Met => style("met    ").green(),
                CriterionStatus::NotMet => style("NOT MET").red().bold(),
                CriterionStatus::Unclear => style("unclear").yellow(),
            };
            let item = c.work_item.map(|id| format!("#{id} ")).unwrap_or_default();
            println!("  {mark} {item}{}", c.criterion);
            if let Some(note) = &c.note {
                println!("          {}", style(note).dim());
            }
        }
    }
    if review.comments.is_empty() {
        println!("\n{}\n", style("No issues found.").green());
        return;
    }
    for (severity, comments) in review.grouped() {
        println!("\n{} ({})", severity_tag(severity), comments.len());
        for (number, c) in comments {
            println!("\n  {number}. {}", style(c.location()).bold());
            println!("{}", indent(&c.body));
        }
    }
    println!();
}

/// Reports what the posting flow did. Returns how many comments failed to post.
pub fn print_outcome(outcome: &Outcome, pr_url: &str) -> usize {
    match outcome {
        Outcome::DryRun => println!("Dry run: nothing was posted."),
        Outcome::NothingSelected => println!("\nNo comments selected; nothing was posted."),
        Outcome::Quit => println!("\nQuit; nothing was posted."),
        Outcome::Declined => println!("\nNot confirmed; nothing was posted."),
        Outcome::Posted(results) => {
            println!();
            let mut failed = 0;
            for r in results {
                match &r.result {
                    Ok(_) => println!("  {} {}", style("✓").green(), r.label),
                    Err(e) => {
                        failed += 1;
                        println!("  {} {} — {e:#}", style("✗").red(), r.label);
                    }
                }
            }
            println!("\n{pr_url}");
            return failed;
        }
    }
    0
}

/// Interactive prompts on the real terminal.
pub struct TerminalPrompter;

fn read_line() -> Result<Option<String>> {
    let mut line = String::new();
    let n = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| anyhow!("Could not read from the terminal: {e}"))?;
    // EOF (e.g. stdin is not a terminal) must never be read as consent.
    Ok((n > 0).then(|| line.trim().to_lowercase()))
}

impl Prompter for TerminalPrompter {
    fn show(&mut self, heading: &str, body: &str) {
        println!("\n{}\n{}", style(heading).bold(), indent(body));
    }

    fn choose(&mut self) -> Result<Choice> {
        loop {
            print!("  [p]ost / [e]dit / [s]kip / [q]uit: ");
            std::io::stdout().flush().ok();
            let Some(answer) = read_line()? else {
                println!();
                return Ok(Choice::Quit);
            };
            match answer.as_str() {
                "p" | "post" => return Ok(Choice::Post),
                "e" | "edit" => return Ok(Choice::Edit),
                "s" | "skip" => return Ok(Choice::Skip),
                "q" | "quit" => return Ok(Choice::Quit),
                _ => println!("  Please answer p, e, s or q."),
            }
        }
    }

    fn edit(&mut self, text: &str) -> Result<Option<String>> {
        dialoguer::Editor::new()
            .extension(".md")
            .edit(text)
            .map_err(|e| anyhow!("Could not open $EDITOR: {e}"))
    }

    fn confirm(&mut self, question: &str) -> Result<bool> {
        print!("{} [y/N] ", style(question).bold());
        std::io::stdout().flush().ok();
        Ok(matches!(read_line()?.as_deref(), Some("y" | "yes")))
    }

    fn info(&mut self, message: &str) {
        println!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn formats_age() {
        let now = Utc::now();
        assert_eq!(format_age(now - Duration::days(3), now), "3d");
        assert_eq!(format_age(now - Duration::hours(5), now), "5h");
        assert_eq!(format_age(now - Duration::minutes(12), now), "12m");
        assert_eq!(format_age(now + Duration::minutes(1), now), "0m");
    }
}
