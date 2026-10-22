// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod awake;
mod engine;
mod model;
mod store;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use engine::{Change, Engine};
use model::{Fix, Live, Record, Settings, TriageState};
use prr::ado::{AdoClient, PrFilter};
use store::Store;

type Shared<'a> = State<'a, Arc<Engine>>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Apps launched from the Dock or Start menu do not get the shell's PATH,
/// so `git` and `claude` would not be found. Borrow it from a login shell.
fn inherit_shell_path() {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let output = Command::new(shell)
            .args(["-ilc", "printf '__WARDEN__%s__WARDEN__' \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(path) = text.split("__WARDEN__").nth(1) {
                dirs.extend(std::env::split_paths(path));
            }
        }
    }
    if let Some(current) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&current));
    }
    if let Some(home) = prr::config::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".claude/local"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    if let Ok(joined) = std::env::join_paths(dirs) {
        std::env::set_var("PATH", joined);
    }
}

/// Puts a user-chosen `claude` ahead of anything else on PATH.
fn prefer_cli(cli_path: &str) {
    let path = Path::new(cli_path.trim());
    if cli_path.trim().is_empty() || !path.is_file() {
        return;
    }
    let Some(dir) = path.parent() else { return };
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(current) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&current).filter(|d| d != dir));
    }
    if let Ok(joined) = std::env::join_paths(dirs) {
        std::env::set_var("PATH", joined);
    }
}

#[tauri::command]
fn get_settings(engine: Shared) -> Settings {
    // Lets a packaged build be checked from a terminal: this line proves the
    // window's scripts loaded and can reach the app under its security policy.
    if std::env::var_os("WARDEN_TRACE").is_some() {
        eprintln!("warden: the window requested settings");
    }
    engine.settings()
}

#[tauri::command(async)]
fn save_settings(engine: Shared, settings: Settings) -> Result<Settings, String> {
    prefer_cli(&settings.cli_path);
    awake::keep_awake(settings.keep_awake);
    engine.update_settings(settings).map_err(err)
}

/// The built-in review guidance, shown as the starting point for a custom prompt.
#[tauri::command]
fn default_prompt() -> &'static str {
    prr::review::DEFAULT_GUIDANCE
}

/// The built-in triage guidance, shown as the starting point for a custom triage prompt.
#[tauri::command]
fn default_triage_prompt() -> &'static str {
    engine::triage::DEFAULT_TRIAGE_GUIDANCE
}

#[tauri::command]
fn get_live(engine: Shared) -> Live {
    engine.live()
}

#[tauri::command]
fn get_history(engine: Shared) -> Vec<Record> {
    engine.history()
}

#[tauri::command]
fn check_now(engine: Shared) {
    engine.check_now();
}

#[tauri::command(async)]
fn retry_review(engine: Shared, pr_id: u64) -> Result<(), String> {
    engine.retry(pr_id).map_err(err)
}

#[tauri::command]
async fn apply_vote(engine: Shared<'_>, record_id: String) -> Result<(), String> {
    engine.apply_vote(&record_id).await.map_err(err)
}

/// Called before installing an update: holds new work back and says whether
/// a review or fix is still in flight.
#[tauri::command]
fn begin_update(engine: Shared) -> bool {
    engine.begin_update()
}

#[tauri::command]
fn is_busy(engine: Shared) -> bool {
    engine.busy()
}

#[tauri::command]
fn end_update(engine: Shared) {
    engine.end_update();
}

#[tauri::command]
fn get_triage(engine: Shared) -> TriageState {
    engine.triage().without_diffs()
}

/// The diff of an item's latest fix, left out of the triage state the window gets.
#[tauri::command]
fn fix_diff(engine: Shared, id: u64) -> String {
    engine.triage().records.into_iter().find(|r| r.id == id).and_then(|r| r.fix).map(|f| f.diff).unwrap_or_default()
}

/// Sends the latest value of an event to the window at most every 250ms, and not at
/// all while the window is hidden (it asks for everything again when shown). Claude's
/// activity changes the live and triage state many times a second.
struct Throttled {
    latest: std::sync::Mutex<Option<serde_json::Value>>,
    scheduled: std::sync::atomic::AtomicBool,
}

impl Throttled {
    const fn new() -> Self {
        Self { latest: std::sync::Mutex::new(None), scheduled: std::sync::atomic::AtomicBool::new(false) }
    }

    fn send(&'static self, handle: &AppHandle, event: &'static str, value: impl Serialize) {
        use std::sync::atomic::Ordering;
        let Ok(value) = serde_json::to_value(value) else { return };
        *self.latest.lock().unwrap() = Some(value);
        if self.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let handle = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            self.scheduled.store(false, Ordering::SeqCst);
            let latest = self.latest.lock().unwrap().take();
            if let Some(value) = latest {
                if window_visible(&handle) {
                    let _ = handle.emit(event, value);
                }
            }
        });
    }
}

static LIVE_OUT: Throttled = Throttled::new();
static TRIAGE_OUT: Throttled = Throttled::new();

fn window_visible(handle: &AppHandle) -> bool {
    handle.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(true)
}

/// Queues Claude to fix a triaged work item: branch, fix, tests, pull request.
#[tauri::command]
async fn fix_work_item(engine: Shared<'_>, id: u64, notes: Option<String>) -> Result<(), String> {
    engine.fix_work_item(id, notes.as_deref().unwrap_or("")).map_err(err)
}

/// Queues Claude to address the open comments on the pull request a fix opened.
#[tauri::command]
async fn address_pr_feedback(engine: Shared<'_>, id: u64) -> Result<(), String> {
    engine.address_pr_feedback(id).map_err(err)
}

/// Queues every failed fix again.
#[tauri::command]
async fn retry_failed_fixes(engine: Shared<'_>) -> Result<usize, String> {
    engine.retry_failed_fixes().map_err(err)
}

/// Moves a triaged item to "AI can fix" (`ai`) or "For a person".
#[tauri::command]
async fn set_recommendation(engine: Shared<'_>, id: u64, ai: bool) -> Result<(), String> {
    engine.set_recommendation(id, ai).map_err(err)
}

/// Starts a Claude Code session for an item: a worktree in the user's checkout from the
/// latest base branch, then the Claude desktop app opened there with `/fix <id>` ready.
#[tauri::command]
async fn new_session(app: AppHandle, engine: Shared<'_>, id: u64) -> Result<String, String> {
    let work = engine.session_worktree(id).await.map_err(err)?;
    let url = format!(
        "claude://code/new?folder={}&q={}",
        urlencoding::encode(&work.to_string_lossy()),
        urlencoding::encode(&format!("/fix {id}"))
    );
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>).map_err(|e| format!("Could not open the Claude app: {e}"))?;
    Ok(work.display().to_string())
}

/// Approves a pull request that failed review, with the user's reason posted on it.
#[tauri::command]
async fn approve_anyway(engine: Shared<'_>, pr_id: u64, project: String, reason: String) -> Result<(), String> {
    engine.approve_anyway(pr_id, &project, &reason).await.map_err(err)
}

/// Checks the local model answers, and how quickly.
#[tauri::command]
async fn test_local_model(url: String, model: String) -> Result<String, String> {
    let started = std::time::Instant::now();
    let reply = engine::local::ask(&url, &model, "Reply with only this JSON: {\"ok\": true}", std::time::Duration::from_secs(120))
        .await
        .map_err(err)?;
    if !reply.contains("ok") {
        return Err(format!("The model answered, but not as asked: {}", reply.chars().take(120).collect::<String>()));
    }
    Ok(format!("{model} answered in {:.1}s", started.elapsed().as_secs_f64()))
}

/// Sets whether an item's fix is simple or complex, which picks its model.
#[tauri::command(async)]
fn set_complexity(engine: Shared, id: u64, complex: bool) -> Result<(), String> {
    engine.set_complexity(id, complex).map_err(err)
}

/// Forgets a triage item; the next check adds it back if it still qualifies.
#[tauri::command(async)]
fn forget_item(engine: Shared, id: u64) -> Result<(), String> {
    engine.forget_item(id).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemCommentView {
    author: String,
    date: String,
    text: String,
}

/// The latest comments on a work item in the triage list, newest first.
#[tauri::command]
async fn item_comments(engine: Shared<'_>, id: u64) -> Result<Vec<ItemCommentView>, String> {
    let project = engine
        .triage()
        .records
        .iter()
        .find(|r| r.id == id)
        .map(|r| r.project.clone())
        .ok_or_else(|| "That work item is not in the triage list.".to_string())?;
    let token = prr::auth::get_pat().map_err(err)?;
    let client = AdoClient::new(engine.settings().organization.trim(), &project, &token).map_err(err)?;
    let mut comments = client.work_item_comments(id, 30).await.map_err(err)?;
    comments.reverse();
    Ok(comments.into_iter().map(|c| ItemCommentView { author: c.author, date: c.date, text: c.text }).collect())
}

/// The projects in the organization, for adding one in Settings.
#[tauri::command]
async fn list_projects(engine: Shared<'_>) -> Result<Vec<String>, String> {
    let token = prr::auth::get_pat().map_err(err)?;
    let client = AdoClient::new(engine.settings().organization.trim(), "", &token).map_err(err)?;
    client.list_projects().await.map_err(err)
}

#[tauri::command(async)]
fn triage_again(engine: Shared, id: u64) -> Result<(), String> {
    engine.triage_again(id).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectStructure {
    areas: Vec<String>,
    repos: Vec<RepoChoice>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoChoice {
    name: String,
    default_branch: String,
}

/// Area paths and repositories in a project, for the mapping editor in Settings.
#[tauri::command]
async fn project_structure(engine: Shared<'_>, project: String) -> Result<ProjectStructure, String> {
    let token = prr::auth::get_pat().map_err(err)?;
    let client = AdoClient::new(engine.settings().organization.trim(), &project, &token).map_err(err)?;
    let areas = client.area_paths().await.map_err(err)?;
    let repos = client
        .repositories()
        .await
        .map_err(err)?
        .into_iter()
        .map(|r| RepoChoice {
            default_branch: r.default_branch.unwrap_or_default().trim_start_matches("refs/heads/").to_string(),
            name: r.name,
        })
        .collect();
    Ok(ProjectStructure { areas, repos })
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct TriageChoices {
    types: Vec<String>,
    states: Vec<String>,
}

/// The work item types and states used across the given projects, for the triage settings.
#[tauri::command]
async fn triage_choices(engine: Shared<'_>, projects: Vec<String>) -> Result<TriageChoices, String> {
    let token = prr::auth::get_pat().map_err(err)?;
    let organization = engine.settings().organization.trim().to_string();
    let mut choices = TriageChoices::default();
    for project in projects.iter().filter(|p| !p.trim().is_empty()) {
        let client = AdoClient::new(&organization, project, &token).map_err(err)?;
        for kind in client.work_item_types().await.map_err(err)? {
            if !choices.types.iter().any(|t| t.eq_ignore_ascii_case(&kind.name)) {
                choices.types.push(kind.name);
            }
            for state in kind.states {
                if !choices.states.iter().any(|s| s.eq_ignore_ascii_case(&state)) {
                    choices.states.push(state);
                }
            }
        }
    }
    choices.types.sort_by_key(|t| t.to_lowercase());
    Ok(choices)
}

#[tauri::command]
fn get_fixes(engine: Shared) -> Vec<Fix> {
    engine.fixes()
}

/// Has Claude prepare a fix for chosen issues. Nothing is pushed by this.
#[tauri::command]
async fn start_fix(engine: Shared<'_>, record_id: String, issues: Vec<usize>) -> Result<String, String> {
    engine.start_fix(&record_id, &issues).map_err(err)
}

/// Commits and pushes a fix the user has read and approved.
#[tauri::command]
async fn push_fix(engine: Shared<'_>, fix_id: String) -> Result<(), String> {
    engine.push_fix(&fix_id).await.map_err(err)
}

#[tauri::command(async)]
fn discard_fix(engine: Shared, fix_id: String) -> Result<(), String> {
    engine.discard_fix(&fix_id).map_err(err)
}

#[tauri::command(async)]
fn has_pat() -> bool {
    prr::auth::get_pat().is_ok()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    user: String,
    projects: Vec<String>,
    /// Set when the token works but cannot list projects.
    projects_note: Option<String>,
}

/// Checks a token against an organization. A new token is stored in the
/// system keychain only once it has been shown to work.
#[tauri::command]
async fn test_connection(organization: String, pat: Option<String>) -> Result<Connection, String> {
    let organization = organization.trim().to_string();
    if organization.is_empty() {
        return Err("Enter your organization first.".to_string());
    }
    let new_pat = pat.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let token = match &new_pat {
        Some(p) => p.clone(),
        None => prr::auth::get_pat().map_err(|_| "Enter a personal access token.".to_string())?,
    };
    let client = AdoClient::new(&organization, "", &token).map_err(err)?;
    let me = client.current_user().await.map_err(err)?;
    let (projects, projects_note) = match client.list_projects().await {
        Ok(projects) => (projects, None),
        Err(e) => (Vec::new(), Some(err(e))),
    };
    if let Some(p) = new_pat {
        prr::auth::store_pat(&p).map_err(err)?;
    }
    Ok(Connection {
        user: if me.display_name.is_empty() { me.id } else { me.display_name },
        projects,
        projects_note,
    })
}

/// People who have raised pull requests recently, to pick reviewers' targets from.
#[tauri::command]
async fn list_people(organization: String, projects: Vec<String>) -> Result<Vec<String>, String> {
    let token = prr::auth::get_pat().map_err(err)?;
    let client = AdoClient::new(organization.trim(), "", &token).map_err(err)?;
    let projects = if projects.is_empty() { client.list_projects().await.map_err(err)? } else { projects };
    let mut names = std::collections::BTreeSet::new();
    for project in projects.iter().take(20) {
        let filter = PrFilter { status: Some("all".to_string()), top: Some(200), ..Default::default() };
        if let Ok(prs) = client.with_project(project).list_prs(&filter).await {
            names.extend(prs.into_iter().map(|pr| pr.created_by.display_name).filter(|n| !n.is_empty()));
        }
    }
    Ok(names.into_iter().collect())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ClaudeStatus {
    found: bool,
    path: String,
    version: String,
    signed_in: bool,
    /// Recent enough to run reviews confined (has `--restricted`).
    supported: bool,
    git_found: bool,
}

#[tauri::command(async)]
fn claude_status() -> ClaudeStatus {
    let mut status = ClaudeStatus { git_found: prr::git::find_tool("git").is_some(), ..Default::default() };
    let Some(path) = prr::git::find_tool("claude") else { return status };
    status.found = true;
    status.path = path.display().to_string();
    let run = |args: &[&str]| {
        prr::git::background(&path)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    status.version = run(&["--version"]).unwrap_or_default();
    status.supported = run(&["--help"]).is_some_and(|help| help.contains("--restricted"));
    status.signed_in = run(&["auth", "status"])
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["loggedIn"].as_bool())
        .unwrap_or(false);
    status
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Legacy {
    organization: String,
    projects: Vec<String>,
    people: Vec<String>,
    approve_and_complete: bool,
}

/// Settings from the `prr` command-line tool, to pre-fill first-run setup.
#[tauri::command(async)]
fn import_legacy() -> Option<Legacy> {
    let config = prr::config::Config::load().ok()?;
    Some(Legacy {
        organization: config.organization,
        projects: vec![config.project],
        people: config.authors,
        approve_and_complete: config.auto.approve && config.auto.autocomplete,
    })
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Warden", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "Check for PRs now", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause / resume monitoring", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Warden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &check, &pause, &quit])?;
    let mut tray = TrayIconBuilder::new().menu(&menu).tooltip("Warden");
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, event| {
        let engine = app.state::<Arc<Engine>>();
        match event.id.as_ref() {
            "open" => show_main(app),
            "check" => engine.check_now(),
            "pause" => {
                let mut settings = engine.settings();
                settings.paused = !settings.paused;
                let _ = engine.update_settings(settings.clone());
                let _ = app.emit("settings", settings);
            }
            "quit" => app.exit(0),
            _ => {}
        }
    })
    .build(app)?;
    Ok(())
}

fn main() {
    inherit_shell_path();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        // Updates are fetched from GitHub Releases and must be signed with
        // the key whose public half is in tauri.conf.json.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let handle = app.handle().clone();
            let engine = Arc::new(Engine::new(
                Store::new(dir),
                Box::new(move |change| {
                    match change {
                        Change::Live(live) => LIVE_OUT.send(&handle, "live", live),
                        Change::History(history) => {
                            if window_visible(&handle) {
                                let _ = handle.emit("history", history);
                            }
                        }
                        Change::Fixes(fixes) => {
                            let _ = handle.emit("fixes", fixes);
                        }
                        Change::Triage(triage) => TRIAGE_OUT.send(&handle, "triage", triage.without_diffs()),
                        Change::Notify { title, body } => {
                            use tauri_plugin_notification::NotificationExt;
                            let _ = handle.notification().builder().title(title).body(body).show();
                        }
                    }
                }),
            ));
            prefer_cli(&engine.settings().cli_path);
            awake::keep_awake(engine.settings().keep_awake);
            app.manage(engine.clone());
            tauri::async_runtime::spawn(engine.run());
            build_tray(app)?;
            Ok(())
        })
        // Closing the window keeps monitoring going; Quit is in the tray menu.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            default_prompt,
            default_triage_prompt,
            get_live,
            fix_diff,
            get_history,
            check_now,
            retry_review,
            apply_vote,
            begin_update,
            is_busy,
            end_update,
            get_fixes,
            get_triage,
            triage_again,
            fix_work_item,
            address_pr_feedback,
            retry_failed_fixes,
            set_recommendation,
            forget_item,
            set_complexity,
            test_local_model,
            approve_anyway,
            new_session,
            list_projects,
            item_comments,
            project_structure,
            triage_choices,
            start_fix,
            push_fix,
            discard_fix,
            has_pat,
            test_connection,
            list_people,
            claude_status,
            import_legacy,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Warden");

    app.run(|app, event| {
        // Clicking the Dock icon brings the window back.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}
