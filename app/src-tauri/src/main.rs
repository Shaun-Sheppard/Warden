// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
use model::{Live, Record, Settings};
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
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
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

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[tauri::command]
fn get_settings(engine: Shared) -> Settings {
    engine.settings()
}

#[tauri::command(async)]
fn save_settings(engine: Shared, settings: Settings) -> Result<Settings, String> {
    prefer_cli(&settings.cli_path);
    engine.update_settings(settings).map_err(err)
}

/// The built-in review guidance, shown as the starting point for a custom prompt.
#[tauri::command]
fn default_prompt() -> &'static str {
    prr::review::DEFAULT_GUIDANCE
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
    git_found: bool,
}

#[tauri::command(async)]
fn claude_status() -> ClaudeStatus {
    let mut status = ClaudeStatus { git_found: find_on_path("git").is_some(), ..Default::default() };
    let Some(path) = find_on_path("claude") else { return status };
    status.found = true;
    status.path = path.display().to_string();
    let run = |args: &[&str]| {
        Command::new(&path)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    status.version = run(&["--version"]).unwrap_or_default();
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
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let handle = app.handle().clone();
            let engine = Arc::new(Engine::new(
                Store::new(dir),
                Box::new(move |change| {
                    let _ = match change {
                        Change::Live(live) => handle.emit("live", live),
                        Change::History(history) => handle.emit("history", history),
                    };
                }),
            ));
            prefer_cli(&engine.settings().cli_path);
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
            get_live,
            get_history,
            check_now,
            retry_review,
            apply_vote,
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
