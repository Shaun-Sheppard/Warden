//! Full-screen UI: PR list on the left, details / review progress / results
//! on the right. Reviews only read from Azure DevOps; posting comments hands
//! over to the same confirmed, line-based flow as `prr review`.

use anyhow::Result;
use chrono::Utc;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::ado::{author_matches, vote_label, AdoClient, Identity, PrFilter, PullRequest};
use crate::cache::{self, SavedReview};
use crate::config::{self, Config};
use crate::flow;
use crate::pipeline::{self, Progress, ReviewRun};
use crate::review::{Severity, Verdict};
use crate::ui::{self, format_age, TerminalPrompter};

const MAX_ACTIVITY: usize = 500;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub enum AppEvent {
    Loaded(Result<(Identity, Vec<PullRequest>)>),
    Progress(u64, Progress),
    Finished(u64, Result<ReviewRun>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Mine,
    Reviewing,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Mine,
            Filter::Mine => Filter::Reviewing,
            Filter::Reviewing => Filter::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Mine => "mine",
            Filter::Reviewing => "reviewing",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    /// The user confirmed a review of this PR.
    Start(u64),
    /// Leave the full-screen UI to choose and post comments for this PR.
    Post(u64),
}

struct Running {
    pr_id: u64,
    started: Instant,
    stages: Vec<String>,
    activity: VecDeque<String>,
}

pub struct App {
    prs: Vec<PullRequest>,
    me_id: String,
    loading: bool,
    load_error: Option<String>,
    filter: Filter,
    list: ListState,
    reviews_dir: PathBuf,
    /// The list is limited to configured authors.
    by_author: bool,
    reviews: HashMap<u64, SavedReview>,
    failures: HashMap<u64, String>,
    running: Option<Running>,
    /// PR awaiting the "start a review?" answer.
    confirm: Option<u64>,
    scroll: u16,
    status: Option<String>,
    quit_armed: bool,
}

impl App {
    pub fn new(reviews_dir: PathBuf) -> Self {
        Self {
            prs: Vec::new(),
            me_id: String::new(),
            loading: true,
            load_error: None,
            filter: Filter::All,
            list: ListState::default(),
            reviews_dir,
            by_author: false,
            reviews: HashMap::new(),
            failures: HashMap::new(),
            running: None,
            confirm: None,
            scroll: 0,
            status: None,
            quit_armed: false,
        }
    }

    fn visible(&self) -> Vec<&PullRequest> {
        self.prs
            .iter()
            .filter(|pr| match self.filter {
                Filter::All => true,
                Filter::Mine => pr.created_by.id.eq_ignore_ascii_case(&self.me_id),
                Filter::Reviewing => pr.vote_of(&self.me_id).is_some(),
            })
            .collect()
    }

    fn selected(&self) -> Option<&PullRequest> {
        self.list.selected().and_then(|i| self.visible().get(i).copied())
    }

    pub fn selected_id(&self) -> Option<u64> {
        self.selected().map(|pr| pr.pull_request_id)
    }

    pub fn pr(&self, id: u64) -> Option<&PullRequest> {
        self.prs.iter().find(|pr| pr.pull_request_id == id)
    }

    pub fn review(&self, id: u64) -> Option<&SavedReview> {
        self.reviews.get(&id)
    }

    fn select_id(&mut self, id: Option<u64>) {
        let visible = self.visible();
        let index = id
            .and_then(|id| visible.iter().position(|pr| pr.pull_request_id == id))
            .or((!visible.is_empty()).then_some(0));
        self.list.select(index);
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let current = self.list.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        if self.list.selected() != Some(next) {
            self.list.select(Some(next));
            self.scroll = 0;
        }
    }

    pub fn set_loading(&mut self) {
        self.loading = true;
        self.load_error = None;
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = Some(status.into());
    }

    pub fn start(&mut self, pr_id: u64) {
        self.failures.remove(&pr_id);
        self.scroll = 0;
        self.running = Some(Running {
            pr_id,
            started: Instant::now(),
            stages: Vec::new(),
            activity: VecDeque::new(),
        });
    }

    pub fn apply(&mut self, event: AppEvent) {
        match event {
            AppEvent::Loaded(Ok((me, mut prs))) => {
                let keep = self.selected_id();
                prs.sort_by(|a, b| b.creation_date.cmp(&a.creation_date));
                // Show earlier reviews without re-running them.
                for pr in &prs {
                    let id = pr.pull_request_id;
                    if !self.reviews.contains_key(&id) {
                        if let Ok(Some((_, saved))) = cache::latest(&self.reviews_dir, id) {
                            self.reviews.insert(id, saved);
                        }
                    }
                }
                self.me_id = me.id;
                self.prs = prs;
                self.loading = false;
                self.load_error = None;
                self.select_id(keep);
            }
            AppEvent::Loaded(Err(e)) => {
                self.loading = false;
                self.load_error = Some(format!("{e:#}"));
            }
            AppEvent::Progress(id, progress) => {
                let Some(run) = self.running.as_mut().filter(|r| r.pr_id == id) else { return };
                match progress {
                    Progress::Stage(stage) => run.stages.push(stage),
                    Progress::Activity(activity) => {
                        if run.activity.len() == MAX_ACTIVITY {
                            run.activity.pop_front();
                        }
                        run.activity.push_back(activity);
                    }
                }
            }
            AppEvent::Finished(id, result) => {
                if self.running.as_ref().is_some_and(|r| r.pr_id == id) {
                    self.running = None;
                }
                match result {
                    Ok(run) => {
                        self.reviews.insert(id, run.saved);
                        self.status = Some(format!("Review of #{id} finished."));
                    }
                    Err(e) => {
                        self.failures.insert(id, format!("{e:#}"));
                        self.status = Some(format!("Review of #{id} failed."));
                    }
                }
                if self.selected_id() == Some(id) {
                    self.scroll = 0;
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        self.status = None;

        if let Some(id) = self.confirm {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.confirm = None;
                    Action::Start(id)
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.confirm = None;
                    Action::None
                }
                _ => Action::None,
            };
        }

        let quit_was_armed = std::mem::take(&mut self.quit_armed);
        match key.code {
            KeyCode::Char('q') => {
                if self.running.is_some() && !quit_was_armed {
                    self.quit_armed = true;
                    self.status =
                        Some("A review is running. Press q again to cancel it and quit.".into());
                    return Action::None;
                }
                return Action::Quit;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Home => self.move_selection(isize::MIN / 2),
            KeyCode::End => self.move_selection(isize::MAX / 2),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Enter => match (self.selected_id(), &self.running) {
                (None, _) => {}
                (Some(_), Some(run)) => {
                    self.status = Some(format!(
                        "A review of #{} is already running; wait for it to finish.",
                        run.pr_id
                    ));
                }
                (Some(id), None) => self.confirm = Some(id),
            },
            KeyCode::Char('p') => {
                if let Some(id) = self.selected_id() {
                    let busy = self.running.as_ref().is_some_and(|r| r.pr_id == id);
                    if self.reviews.contains_key(&id) && !busy {
                        return Action::Post(id);
                    }
                    self.status = Some("No finished review to post for this PR yet.".into());
                }
            }
            KeyCode::Char('f') => {
                let keep = self.selected_id();
                self.filter = self.filter.next();
                self.select_id(keep);
                self.scroll = 0;
            }
            KeyCode::Char('r') => return Action::Refresh,
            _ => {}
        }
        Action::None
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn heading(text: &str) -> Line<'static> {
    Line::from(text.to_string().bold())
}

fn review_lines(saved: &SavedReview) -> Vec<Line<'static>> {
    let review = &saved.review;
    let decision_color = match review.verdict {
        Verdict::Approve => Color::Green,
        Verdict::ApproveWithSuggestions => Color::Yellow,
        Verdict::ChangesRequested => Color::Red,
    };
    let mut lines = vec![
        Line::from(vec![
            "Decision: ".bold(),
            Span::styled(
                review.verdict.to_string(),
                Style::new().fg(decision_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(
            format!(
                "Reviewed {} ago · {} → {}",
                format_age(saved.reviewed_at, Utc::now()),
                saved.source_branch,
                saved.target_branch
            )
            .dim(),
        ),
        Line::default(),
    ];
    lines.extend(review.summary.lines().map(|l| Line::from(l.to_string())));

    if review.comments.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from("No issues found.".green()));
        return lines;
    }
    for (severity, comments) in review.grouped() {
        let color = match severity {
            Severity::Critical => Color::Red,
            Severity::Major => Color::Yellow,
            Severity::Minor => Color::Cyan,
        };
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("{} ({})", severity.heading(), comments.len()),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        )));
        for (number, comment) in comments {
            lines.push(Line::default());
            lines.push(Line::from(vec![format!("{number}. ").into(), comment.location().bold()]));
            lines.extend(comment.body.lines().map(|l| Line::from(l.to_string())));
        }
    }
    lines
}

fn details_lines(pr: &PullRequest, me_id: &str) -> Vec<Line<'static>> {
    let field = |name: &str, value: String| Line::from(vec![format!("{name:<9}").dim(), value.into()]);
    let mut lines = vec![
        field("Author", pr.created_by.display_name.clone()),
        field("Repo", pr.repository.name.clone()),
        field("Branches", format!("{} → {}", pr.source_branch(), pr.target_branch())),
        field("Age", format_age(pr.creation_date, Utc::now())),
        field("My vote", vote_label(pr.vote_of(me_id)).to_string()),
    ];
    if let Some(description) = pr.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        lines.push(Line::default());
        lines.push(heading("Description"));
        lines.extend(description.lines().map(|l| Line::from(l.to_string())));
    }
    lines
}

fn running_lines(run: &Running, height: usize) -> Vec<Line<'static>> {
    let elapsed = run.started.elapsed();
    let frame = SPINNER[(elapsed.as_millis() / 100) as usize % SPINNER.len()];
    let mut lines = vec![
        Line::from(vec![
            "Review in progress".bold(),
            format!("  {}", format_elapsed(elapsed)).dim(),
        ]),
        Line::default(),
    ];
    let last = run.stages.len().saturating_sub(1);
    for (i, stage) in run.stages.iter().enumerate() {
        lines.push(if i < last {
            Line::from(vec!["✓ ".green(), stage.clone().dim()])
        } else {
            Line::from(vec![format!("{frame} ").cyan(), stage.clone().bold()])
        });
    }
    if !run.activity.is_empty() {
        lines.push(Line::default());
        lines.push(heading("Activity"));
        // Keep the newest activity in view.
        let room = height.saturating_sub(lines.len()).max(1);
        let skip = run.activity.len().saturating_sub(room);
        lines.extend(run.activity.iter().skip(skip).map(|a| Line::from(format!("  {a}").dim())));
    }
    lines
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &mut App) {
    let scope = if app.by_author { " · chosen authors" } else { "" };
    let title = format!(" Pull requests · {}{scope} ", app.filter.label());
    let block = Block::bordered().title(title);
    let width = area.width.saturating_sub(4) as usize;

    let message = if app.loading && app.prs.is_empty() {
        Some("Loading…".to_string())
    } else if let Some(error) = app.load_error.as_ref().filter(|_| app.prs.is_empty()) {
        Some(error.clone())
    } else if app.visible().is_empty() {
        Some("No active pull requests.".to_string())
    } else {
        None
    };
    if let Some(message) = message {
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }).block(block), area);
        return;
    }

    let items: Vec<ListItem> = app
        .visible()
        .iter()
        .map(|pr| {
            let id = pr.pull_request_id;
            let marker = if app.running.as_ref().is_some_and(|r| r.pr_id == id) {
                "● ".cyan()
            } else if app.failures.contains_key(&id) {
                "✗ ".red()
            } else if app.reviews.contains_key(&id) {
                "✓ ".green()
            } else {
                "  ".into()
            };
            let id_text = format!("#{id} ");
            let title = truncate(&pr.title, width.saturating_sub(id_text.chars().count() + 2));
            ListItem::new(vec![
                Line::from(vec![marker, id_text.bold(), title.into()]),
                Line::from(format!("  {}", truncate(&pr.created_by.display_name, width.saturating_sub(2))).dim()),
            ])
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut app.list);
}

fn draw_main(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(pr) = app.selected() else {
        let text = if app.loading { "Loading pull requests…" } else { "Select a pull request." };
        frame.render_widget(Paragraph::new(text).block(Block::bordered()), area);
        return;
    };
    let id = pr.pull_request_id;
    let block = Block::bordered().title(format!(
        " #{id} {} ",
        truncate(&pr.title, area.width.saturating_sub(12) as usize)
    ));
    let inner_height = area.height.saturating_sub(2) as usize;

    let lines = if let Some(run) = app.running.as_ref().filter(|r| r.pr_id == id) {
        running_lines(run, inner_height)
    } else {
        let mut lines = Vec::new();
        if let Some(error) = app.failures.get(&id) {
            lines.push(Line::from("Review failed".red().bold()));
            lines.extend(error.lines().map(|l| Line::from(l.to_string())));
            lines.push(Line::default());
        }
        match app.reviews.get(&id) {
            Some(saved) => {
                lines.extend(review_lines(saved));
                lines.push(Line::default());
                lines.push(Line::from("p: choose comments to post · Enter: review again".dim()));
            }
            None => {
                lines.extend(details_lines(pr, &app.me_id));
                lines.push(Line::default());
                lines.push(Line::from("Enter: review this pull request".dim()));
            }
        }
        lines
    };

    app.scroll = app.scroll.min(lines.len().saturating_sub(1) as u16);
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0))
        .block(block);
    frame.render_widget(paragraph, area);
}

fn draw_confirm(frame: &mut Frame, area: Rect, pr: &PullRequest) {
    let width = area.width.saturating_sub(4).min(64);
    let [_, row, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(7), Constraint::Fill(1)])
        .areas(area);
    let [_, popup, _] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width), Constraint::Fill(1)])
            .areas(row);
    let lines = vec![
        Line::default(),
        Line::from(format!("Run an AI review of PR #{}?", pr.pull_request_id).bold()),
        Line::from(truncate(&pr.title, width.saturating_sub(4) as usize)),
        Line::default(),
        Line::from(vec!["y".bold(), " start review    ".into(), "n".bold(), " cancel".into()]),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).centered().block(Block::bordered().title(" Confirm ")),
        popup,
    );
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    let sidebar_width = (body.width * 35 / 100).clamp(28, 52).min(body.width);
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(0)]).areas(body);

    draw_sidebar(frame, sidebar, app);
    draw_main(frame, main, app);

    let help = match &app.status {
        Some(status) => Line::from(format!(" {status}").yellow()),
        None => Line::from(
            " ↑/↓ select · Enter review · p post comments · f filter · r refresh · PgUp/PgDn scroll · q quit"
                .dim(),
        ),
    };
    frame.render_widget(Paragraph::new(help), footer);

    if let Some(pr) = app.confirm.and_then(|id| app.pr(id)) {
        draw_confirm(frame, frame.area(), pr);
    }
}

fn spawn_load(
    client: &Arc<AdoClient>,
    config: &Arc<Config>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let (client, config, tx) = (client.clone(), config.clone(), tx.clone());
    tokio::spawn(async move {
        let filter = PrFilter { repo: config.default_repo.clone(), ..Default::default() };
        let result = tokio::try_join!(client.current_user(), client.list_prs(&filter)).map(
            |(me, mut prs)| {
                prs.retain(|pr| author_matches(&config.authors, &pr.created_by));
                (me, prs)
            },
        );
        let _ = tx.send(AppEvent::Loaded(result));
    });
}

fn spawn_review(
    id: u64,
    client: &Arc<AdoClient>,
    config: &Arc<Config>,
    pat: &Arc<String>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let (client, config, pat, tx) = (client.clone(), config.clone(), pat.clone(), tx.clone());
    tokio::spawn(async move {
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let forward = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                let _ = forward.send(AppEvent::Progress(id, progress));
            }
        });
        let result =
            pipeline::run_review(&config, &client, &pat, id, false, true, &progress_tx).await;
        drop(progress_tx);
        let _ = forwarder.await;
        let _ = tx.send(AppEvent::Finished(id, result));
    });
}

/// Leaves the full-screen UI for the per-comment, confirmed posting flow.
async fn post_comments(
    terminal: &mut DefaultTerminal,
    client: &AdoClient,
    config: &Config,
    pr: &PullRequest,
    saved: &SavedReview,
) {
    ratatui::restore();
    println!("PR #{} {}", pr.pull_request_id, pr.title);
    ui::print_review(&saved.review);
    let outcome = flow::post_flow(
        client,
        pr,
        &saved.review,
        &config.review.comment_prefix,
        config.review.mention_author,
        false,
        &mut TerminalPrompter,
    )
    .await;
    match outcome {
        Ok(outcome) => {
            ui::print_outcome(&outcome, &client.pr_web_url(pr));
        }
        Err(e) => eprintln!("error: {e:#}"),
    }
    println!("\nPress Enter to return to the PR list…");
    let _ = std::io::stdin().read_line(&mut String::new());
    *terminal = ratatui::init();
}

pub async fn run(config: Config, pat: String) -> Result<()> {
    let client = Arc::new(AdoClient::new(&config.organization, &config.project, &pat)?);
    let config = Arc::new(config);
    let pat = Arc::new(pat);
    let mut app = App::new(cache::reviews_dir(&config::cache_dir()?));
    app.by_author = !config.authors.is_empty();
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_load(&client, &config, &tx);

    let mut terminal = ratatui::init();
    let result = async {
        loop {
            while let Ok(event) = rx.try_recv() {
                app.apply(event);
            }
            terminal.draw(|frame| draw(frame, &mut app))?;

            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let Event::Key(key) = event::read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.on_key(key) {
                Action::None => {}
                Action::Quit => break,
                Action::Refresh => {
                    app.set_loading();
                    app.set_status("Refreshing…");
                    spawn_load(&client, &config, &tx);
                }
                Action::Start(id) => {
                    app.start(id);
                    spawn_review(id, &client, &config, &pat, &tx);
                }
                Action::Post(id) => {
                    if let (Some(pr), Some(saved)) = (app.pr(id).cloned(), app.review(id).cloned()) {
                        post_comments(&mut terminal, &client, &config, &pr, &saved).await;
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::parse_review;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;

    fn pr(id: u64, title: &str, author: (&str, &str), reviewers: &[&str]) -> PullRequest {
        serde_json::from_value(json!({
            "pullRequestId": id,
            "title": title,
            "description": "Makes it faster.",
            "sourceRefName": "refs/heads/feature/x",
            "targetRefName": "refs/heads/main",
            "creationDate": format!("2026-10-0{}T10:00:00Z", id % 9 + 1),
            "createdBy": { "id": author.0, "displayName": author.1 },
            "repository": { "id": "rid", "name": "repo" },
            "reviewers": reviewers.iter().map(|r| json!({ "id": r, "vote": 0 })).collect::<Vec<_>>()
        }))
        .unwrap()
    }

    fn loaded_app() -> App {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(tmp.path().join("reviews"));
        let me = Identity { id: "me".into(), display_name: "Me".into(), ..Default::default() };
        let prs = vec![
            pr(1, "Add cache layer to the patient lookup service endpoint", ("u1", "Ann Author"), &["me"]),
            pr(2, "Fix login", ("me", "Me Myself"), &[]),
            pr(3, "Tidy docs", ("u3", "Cat Writer"), &[]),
        ];
        app.apply(AppEvent::Loaded(Ok((me, prs))));
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn saved(id: u64) -> SavedReview {
        SavedReview {
            pr_id: id,
            repo: "repo".into(),
            title: "t".into(),
            source_branch: "feature/x".into(),
            target_branch: "main".into(),
            source_commit: None,
            reviewed_at: Utc::now(),
            review: parse_review(
                r#"{"verdict":"changes_requested","summary":"Risky change to lookup.",
                    "comments":[{"file":"src/a.cs","line":42,"severity":"critical","body":"Null deref here."}]}"#,
            )
            .unwrap(),
        }
    }

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(110)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn sidebar_shows_id_truncated_title_and_author() {
        let mut app = loaded_app();
        let screen = render(&mut app);
        // Newest first, first row selected.
        assert_eq!(app.selected_id(), Some(3));
        assert!(screen.contains("#3 Tidy docs"), "{screen}");
        assert!(screen.contains("Cat Writer"));
        assert!(screen.contains("#1 Add cache layer to the"), "{screen}");
        assert!(screen.contains('…'));
        assert!(screen.contains("Ann Author"));
        // Details of the selected PR on the right.
        assert!(screen.contains("feature/x → main"));
        assert!(screen.contains("Enter: review this pull request"));
    }

    #[test]
    fn arrows_move_selection_within_bounds() {
        let mut app = loaded_app();
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.selected_id(), Some(3));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected_id(), Some(2));
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected_id(), Some(1));
    }

    #[test]
    fn enter_asks_for_confirmation_before_starting() {
        let mut app = loaded_app();
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(render(&mut app).contains("Run an AI review of PR #3?"));

        assert_eq!(app.on_key(key(KeyCode::Char('n'))), Action::None);
        assert!(!render(&mut app).contains("Run an AI review"));

        app.on_key(key(KeyCode::Enter));
        // Navigation keys do not answer the question.
        assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
        assert_eq!(app.selected_id(), Some(3));
        assert_eq!(app.on_key(key(KeyCode::Char('y'))), Action::Start(3));
    }

    #[test]
    fn progress_then_results_appear_in_the_right_column() {
        let mut app = loaded_app();
        app.start(3);
        app.apply(AppEvent::Progress(3, Progress::Stage("Fetching PR #3 from Azure DevOps".into())));
        app.apply(AppEvent::Progress(3, Progress::Stage("Claude is reviewing".into())));
        app.apply(AppEvent::Progress(3, Progress::Activity("Read src/a.cs".into())));
        let screen = render(&mut app);
        assert!(screen.contains("Review in progress"), "{screen}");
        assert!(screen.contains("✓ Fetching PR #3 from Azure DevOps"));
        assert!(screen.contains("Claude is reviewing"));
        assert!(screen.contains("Read src/a.cs"));

        // Only one review at a time.
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(render(&mut app).contains("already running"));
        app.on_key(key(KeyCode::Up));

        let run = ReviewRun { pr: app.pr(3).unwrap().clone(), saved: saved(3), path: PathBuf::new(), stats: None, work_items: vec![], work_items_note: None };
        app.apply(AppEvent::Finished(3, Ok(run)));
        let screen = render(&mut app);
        assert!(screen.contains("Decision: Reject"), "{screen}");
        assert!(screen.contains("Risky change to lookup."));
        assert!(screen.contains("Critical (1)"));
        assert!(screen.contains("1. src/a.cs:42"));
        assert!(screen.contains("Null deref here."));
        assert_eq!(app.on_key(key(KeyCode::Char('p'))), Action::Post(3));
    }

    #[test]
    fn failure_is_shown_and_posting_needs_a_review() {
        let mut app = loaded_app();
        app.start(3);
        app.apply(AppEvent::Finished(3, Err(anyhow::anyhow!("Claude timed out after 600s."))));
        let screen = render(&mut app);
        assert!(screen.contains("Review failed"));
        assert!(screen.contains("Claude timed out after 600s."));
        assert_eq!(app.on_key(key(KeyCode::Char('p'))), Action::None);
        assert!(render(&mut app).contains("No finished review to post"));
    }

    #[test]
    fn filter_cycles_all_mine_reviewing() {
        let mut app = loaded_app();
        app.on_key(key(KeyCode::Char('f')));
        assert_eq!(app.visible().iter().map(|p| p.pull_request_id).collect::<Vec<_>>(), [2]);
        assert_eq!(app.selected_id(), Some(2));
        app.on_key(key(KeyCode::Char('f')));
        assert_eq!(app.visible().iter().map(|p| p.pull_request_id).collect::<Vec<_>>(), [1]);
        app.on_key(key(KeyCode::Char('f')));
        assert_eq!(app.visible().len(), 3);
    }

    #[test]
    fn quitting_during_a_review_needs_a_second_press() {
        let mut app = loaded_app();
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::Quit);
        app.start(3);
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::None);
        assert_eq!(app.on_key(key(KeyCode::Char('q'))), Action::Quit);
    }

    #[test]
    fn renders_loading_error_and_tiny_terminals_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::new(tmp.path().to_path_buf());
        assert!(render(&mut app).contains("Loading…"));
        app.apply(AppEvent::Loaded(Err(anyhow::anyhow!("Azure DevOps API error (HTTP 500): boom"))));
        assert!(render(&mut app).contains("HTTP 500"));

        let mut app = loaded_app();
        app.on_key(key(KeyCode::Enter));
        for (w, h) in [(1, 1), (10, 3), (30, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        }
    }
}
