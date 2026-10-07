use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

use prr::ado::{author_matches, AdoClient, PrFilter, PullRequest};
use prr::auto::{self, AutoOptions};
use prr::flow::Prompter;
use prr::config::{self, Config};
use prr::pipeline::{self, Progress, ProgressTx};
use prr::ui::{self, TerminalPrompter};
use prr::{auth, cache, flow, git, init, tui};

#[derive(Parser)]
#[command(name = "prr", version, about = "AI-assisted PR review for Azure DevOps")]
struct Cli {
    /// Without a command, prr opens the full-screen PR browser.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create ~/.config/prr/config.toml interactively
    Init {
        /// Overwrite an existing config file
        #[arg(long)]
        force: bool,
    },
    /// Manage the Azure DevOps Personal Access Token
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// List active pull requests
    List {
        /// Only this repository (defaults to `default_repo` from the config)
        #[arg(long)]
        repo: Option<String>,
        /// All repositories in the project, ignoring `default_repo`
        #[arg(long, conflicts_with = "repo")]
        all: bool,
        /// Only PRs I created
        #[arg(long)]
        mine: bool,
        /// Only PRs where I am a reviewer
        #[arg(long)]
        reviewing: bool,
        /// Only PRs raised by these people (comma-separated names or emails;
        /// overrides `authors` from the config)
        #[arg(long, value_delimiter = ',')]
        author: Vec<String>,
    },
    /// Watch for new pull requests, review them and post the comments unattended
    Auto {
        /// Minutes between checks
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
        /// Check once and exit (for cron or launchd)
        #[arg(long)]
        once: bool,
        /// Review but never post
        #[arg(long)]
        dry_run: bool,
        /// On the very first run, also review PRs that are already open
        #[arg(long)]
        include_existing: bool,
        /// Approve PRs whose review finds no critical or major issues
        #[arg(long)]
        approve: bool,
        /// Also set those PRs to auto-complete
        #[arg(long)]
        autocomplete: bool,
        /// Skip the start-up confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Review a pull request with Claude and optionally post comments
    Review {
        id: u64,
        /// Run the review and display it; never prompt to post
        #[arg(long)]
        dry_run: bool,
        /// Delete prr's clone of the repo and clone it again from scratch
        #[arg(long)]
        fresh: bool,
    },
    /// Show the last saved review for a pull request
    Show { id: u64 },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Store a PAT in the macOS Keychain
    Login,
    /// Remove the PAT from the macOS Keychain
    Logout,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        None => browse().await,
        Some(Command::Init { force }) => init_config(force),
        Some(Command::Auth { command: AuthCommand::Login }) => auth_login().await,
        Some(Command::Auth { command: AuthCommand::Logout }) => auth_logout(),
        Some(Command::List { repo, all, mine, reviewing, author }) => {
            list(repo, all, mine, reviewing, author).await
        }
        Some(Command::Auto { interval, once, dry_run, include_existing, approve, autocomplete, yes }) => {
            let opts = AutoOptions {
                interval: Duration::from_secs(interval * 60),
                once,
                dry_run,
                include_existing,
                approve,
                autocomplete,
            };
            auto_mode(opts, yes).await
        }
        Some(Command::Review { id, dry_run, fresh }) => review_pr(id, dry_run, fresh).await,
        Some(Command::Show { id }) => show(id),
    };
    if let Err(e) = result {
        eprintln!("{} {e:#}", style("error:").red().bold());
        std::process::exit(1);
    }
}

fn client(config: &Config) -> Result<AdoClient> {
    AdoClient::new(&config.organization, &config.project, &auth::get_pat()?)
}

async fn browse() -> Result<()> {
    // Fail with a normal error message before taking over the screen.
    let config = Config::load()?;
    let pat = auth::get_pat()?;
    tui::run(config, pat).await
}

fn ask(prompt: &str, default: Option<&str>, allow_empty: bool) -> Result<String> {
    let mut input = dialoguer::Input::<String>::new()
        .with_prompt(prompt)
        .allow_empty(allow_empty);
    if let Some(default) = default {
        input = input.default(default.to_string());
    }
    let answer = input
        .interact_text()
        .map_err(|e| anyhow!("Could not read input: {e}"))?;
    Ok(answer.trim().to_string())
}

fn init_config(force: bool) -> Result<()> {
    let path = config::config_path()?;
    if path.exists() && !force {
        bail!(
            "Config file already exists at {}. Edit it directly, or run `prr init --force` to replace it.",
            path.display()
        );
    }

    // Running inside an Azure DevOps clone lets us pre-fill the answers.
    let cwd = std::env::current_dir().context("Could not determine the current directory")?;
    let detected = git::origin_url(&cwd).and_then(|url| init::parse_remote(&url));
    if let Some(d) = &detected {
        println!("Detected {}/{} from this clone's origin.", d.organization, d.project);
    }

    let answer = ask(
        "Organization (name, or paste its https://dev.azure.com/... URL)",
        detected.as_ref().map(|d| d.organization.as_str()),
        false,
    )?;
    let (organization, url_project) = init::parse_organization(&answer);
    if organization.is_empty() {
        bail!("Could not work out the organization from `{answer}`.");
    }
    if organization != answer {
        println!("  Using organization `{organization}`.");
    }
    let project_default = url_project.or_else(|| detected.as_ref().map(|d| d.project.clone()));
    let project = ask("Project", project_default.as_deref(), false)?;

    let default_repo = ask("Default repo for `prr list` (blank = all repos)", None, true)?;
    let default_repo = (!default_repo.is_empty()).then_some(default_repo);
    let authors = init::parse_authors(&ask(
        "Only show PRs raised by (comma-separated names, blank = everyone)",
        None,
        true,
    )?);

    let text = init::render_config(&organization, &project, default_repo.as_deref(), &authors, &[]);
    Config::parse(&text).context("Generated config is invalid")?;
    let dir = path.parent().expect("config path has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    std::fs::write(&path, text).with_context(|| format!("Could not write {}", path.display()))?;

    println!("\n{} Wrote {}", style("✓").green(), path.display());
    println!("Next: run `prr auth login` to store your Personal Access Token.");
    Ok(())
}

async fn auth_login() -> Result<()> {
    let config = Config::load()?;
    let pat = dialoguer::Password::new()
        .with_prompt("Azure DevOps Personal Access Token (Code: Read & Write)")
        .interact()
        .map_err(|e| anyhow!("Could not read the token: {e}"))?;
    let pat = pat.trim();
    if pat.is_empty() {
        bail!("No token entered.");
    }

    let client = AdoClient::new(&config.organization, &config.project, pat)?;
    let me = client.current_user().await?;
    // Also proves the token can read Code in the configured project.
    client
        .list_prs(&PrFilter { top: Some(1), ..Default::default() })
        .await
        .with_context(|| {
            format!(
                "The token works, but could not list pull requests in {}/{}",
                config.organization, config.project
            )
        })?;

    auth::store_pat(pat)?;
    let who = if me.display_name.is_empty() { me.id } else { me.display_name };
    println!("{} Authenticated as {who}. Token stored in the macOS Keychain.", style("✓").green());
    Ok(())
}

fn auth_logout() -> Result<()> {
    if auth::delete_pat()? {
        println!("Token removed from the macOS Keychain.");
    } else {
        println!("No stored token to remove.");
    }
    Ok(())
}

async fn auto_mode(mut opts: AutoOptions, yes: bool) -> Result<()> {
    let config = Config::load()?;
    opts.approve |= config.auto.approve;
    opts.autocomplete |= config.auto.autocomplete;
    let pat = auth::get_pat()?;
    let client = AdoClient::new(&config.organization, &config.project, &pat)?;
    git::ensure_tool("git")?;
    git::ensure_tool("claude")?;

    let scope = match &config.default_repo {
        Some(repo) => format!("{}/{} (repo {repo})", config.organization, config.project),
        None => format!("{}/{} (all repos)", config.organization, config.project),
    };
    let who = if config.authors.is_empty() {
        "anyone".to_string()
    } else {
        config.authors.join(", ")
    };
    println!("{}", style("prr auto mode").bold());
    println!("  Watching:  {scope}");
    println!("  Raised by: {who}");
    println!(
        "  Checking:  {}",
        if opts.once { "once".to_string() } else { format!("every {} min", opts.interval.as_secs() / 60) }
    );
    if opts.dry_run {
        println!("  Posting:   never (dry run)");
    } else {
        println!(
            "  Posting:   {}",
            style("one review comment per PR, automatically, with no confirmation").yellow()
        );
        if config.review.mention_author {
            println!("             the comment tags the PR's author");
        }
        let clean = "PRs with no critical or major issues";
        if opts.approve {
            println!("  Approving: {}", style(format!("{clean}, as your vote, with no human review")).yellow());
        }
        if opts.autocomplete {
            println!("  Completing: {}", style(format!("{clean} are set to auto-complete (merge when policies pass)")).yellow());
        }
        let question = if opts.approve || opts.autocomplete {
            "Start posting, approving and completing automatically?"
        } else {
            "Start posting reviews automatically?"
        };
        if !yes && !TerminalPrompter.confirm(question)? {
            println!("Not started.");
            return Ok(());
        }
    }
    println!("Press Ctrl-C to stop.\n");

    let state_path = config::cache_dir()?.join("auto-state.json");
    auto::run(&config, &client, &pat, state_path, &opts).await
}

async fn list(
    repo: Option<String>,
    all: bool,
    mine: bool,
    reviewing: bool,
    author: Vec<String>,
) -> Result<()> {
    let config = Config::load()?;
    let client = client(&config)?;
    let repo = if all { None } else { repo.or_else(|| config.default_repo.clone()) };
    let me = client.current_user().await?;

    let base = PrFilter { repo, ..Default::default() };
    let mut filters = Vec::new();
    if mine {
        filters.push(PrFilter { creator_id: Some(me.id.clone()), ..base.clone() });
    }
    if reviewing {
        filters.push(PrFilter { reviewer_id: Some(me.id.clone()), ..base.clone() });
    }
    if filters.is_empty() {
        filters.push(base);
    }

    // --mine --reviewing means either, so the result sets are merged.
    let mut prs: Vec<PullRequest> = Vec::new();
    for filter in &filters {
        for pr in client.list_prs(filter).await? {
            if !prs.iter().any(|p| p.pull_request_id == pr.pull_request_id) {
                prs.push(pr);
            }
        }
    }
    let authors = if author.is_empty() { &config.authors } else { &author };
    prs.retain(|pr| author_matches(authors, &pr.created_by));
    prs.sort_by(|a, b| b.creation_date.cmp(&a.creation_date));

    if prs.is_empty() {
        println!("No active pull requests.");
        return Ok(());
    }
    println!("{}", ui::pr_table(&prs, &me.id, Utc::now()));
    Ok(())
}

/// Shows pipeline stages as a spinner with elapsed time, ticking each off as it completes.
fn cli_progress() -> (ProgressTx, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let mut current: Option<(ProgressBar, String)> = None;
        while let Some(progress) = rx.recv().await {
            let Progress::Stage(stage) = progress else { continue };
            if let Some((spinner, done)) = current.take() {
                spinner.finish_and_clear();
                println!("  {} {done}", style("✓").green());
            }
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::with_template("  {spinner} {msg} [{elapsed}]").expect("valid template"),
            );
            spinner.set_message(stage.clone());
            spinner.enable_steady_tick(Duration::from_millis(100));
            current = Some((spinner, stage));
        }
        if let Some((spinner, _)) = current {
            spinner.finish_and_clear();
        }
    });
    (tx, handle)
}

async fn review_pr(id: u64, dry_run: bool, fresh: bool) -> Result<()> {
    let config = Config::load()?;
    let pat = auth::get_pat()?;
    let client = AdoClient::new(&config.organization, &config.project, &pat)?;

    let (tx, progress) = cli_progress();
    let result = pipeline::run_review(&config, &client, &pat, id, fresh, false, &tx).await;
    drop(tx);
    let _ = progress.await;
    let run = result?;

    println!(
        "\n{} #{} {} ({} → {})",
        style("Review of").bold(),
        run.pr.pull_request_id,
        run.pr.title,
        run.saved.source_branch,
        run.saved.target_branch
    );
    ui::print_review(&run.saved.review);
    println!("Saved to {}", run.path.display());

    let outcome = flow::post_flow(
        &client,
        &run.pr,
        &run.saved.review,
        &config.review.comment_prefix,
        config.review.mention_author,
        dry_run,
        &mut TerminalPrompter,
    )
    .await?;
    let failed = ui::print_outcome(&outcome, &client.pr_web_url(&run.pr));
    if failed > 0 {
        bail!("{failed} comment(s) failed to post.");
    }
    Ok(())
}

fn show(id: u64) -> Result<()> {
    let dir = cache::reviews_dir(&config::cache_dir()?);
    let Some((path, saved)) = cache::latest(&dir, id)? else {
        bail!("No saved review for PR #{id}. Run `prr review {id}` first.");
    };
    println!(
        "{} #{} {} [{}] ({} → {})",
        style("Review of").bold(),
        saved.pr_id,
        saved.title,
        saved.repo,
        saved.source_branch,
        saved.target_branch
    );
    println!(
        "Reviewed {} ago ({})",
        ui::format_age(saved.reviewed_at, Utc::now()),
        saved.reviewed_at.format("%Y-%m-%d %H:%M UTC")
    );
    ui::print_review(&saved.review);
    println!("{}", path.display());
    Ok(())
}
