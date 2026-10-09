use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::pipeline::{Progress, ProgressTx};
use crate::review::extract_result;

/// Which built-in tools a headless run has. Anything not named here does
/// not exist for that run.
#[derive(Debug, Clone, Copy)]
pub struct Tools {
    pub tools: &'static str,
    /// Let file edits inside the working directory go ahead without asking.
    pub accept_edits: bool,
}

/// Reviews only read. There is deliberately no shell access: even
/// "read-only" git commands can write files (`--output`) or read ones
/// outside the repository (`--no-index`).
pub const REVIEW_TOOLS: Tools = Tools { tools: "Read,Grep,Glob", accept_edits: false };
/// Fixing issues also edits files, confined to the directory it runs in.
/// It cannot commit, push or run commands.
pub const FIX_TOOLS: Tools = Tools { tools: "Read,Grep,Glob,Edit,Write", accept_edits: true };

/// Denied outright in every run, whatever else is configured.
pub const DENIED_TOOLS: &str = "Bash,PowerShell,NotebookEdit,WebFetch,WebSearch,Task";

/// The code under review is untrusted, and so is everything in its
/// repository. These options keep it from reaching the rest of the machine:
/// - `--restricted`: no command-running tools, file tools confined to the
///   working directory, and no settings files loaded. The last matters most:
///   a repository's own `.claude/settings.json` can define hooks, which
///   would otherwise run as commands on this machine.
/// - `--safe-mode`: the repository's CLAUDE.md, skills, plugins and hooks are not loaded.
/// - `--strict-mcp-config`: no MCP servers, and so none of their tools.
const CONFINEMENT: [&str; 3] = ["--restricted", "--safe-mode", "--strict-mcp-config"];

/// Arguments for a headless review run.
pub fn build_args(stream: bool) -> Vec<String> {
    build_args_for(stream, REVIEW_TOOLS, &[])
}

/// Arguments for a headless run. The prompt itself is sent on standard
/// input: it is long and multi-line, which command lines (Windows `.cmd`
/// shims especially) cannot carry reliably.
/// `stream` asks for one JSON event per line, so tool activity can be shown live.
/// `extra_dirs` are readable in addition to the working directory.
pub fn build_args_for(stream: bool, tools: Tools, extra_dirs: &[&Path]) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        if stream { "stream-json" } else { "json" }.to_string(),
    ];
    if stream {
        // Required by the CLI for stream-json in print mode.
        args.push("--verbose".to_string());
    }
    args.extend(CONFINEMENT.map(str::to_string));
    let denied = if tools.accept_edits {
        DENIED_TOOLS.to_string()
    } else {
        format!("{DENIED_TOOLS},Edit,Write,MultiEdit")
    };
    args.extend(["--tools".to_string(), tools.tools.to_string(), "--disallowedTools".to_string(), denied]);
    if tools.accept_edits {
        args.extend(["--permission-mode".to_string(), "acceptEdits".to_string()]);
    }
    for dir in extra_dirs {
        args.extend(["--add-dir".to_string(), dir.display().to_string()]);
    }
    args
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 160 {
        format!("{}…", line.chars().take(159).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Human-readable activity (tool calls and narration) in one stream-json event.
pub fn describe_event(event: &Value, repo: &Path) -> Vec<String> {
    if event["type"] != "assistant" {
        return Vec::new();
    }
    let Some(items) = event["message"]["content"].as_array() else {
        return Vec::new();
    };
    let repo_prefix = format!("{}/", repo.display());
    items
        .iter()
        .filter_map(|item| match item["type"].as_str()? {
            "tool_use" => {
                let name = item["name"].as_str()?;
                let detail = ["file_path", "command", "pattern", "path"]
                    .iter()
                    .find_map(|key| item["input"][*key].as_str())
                    .unwrap_or("")
                    .replace(&repo_prefix, "");
                Some(one_line(&format!("{name} {detail}")))
            }
            "text" => {
                let text = item["text"].as_str()?.trim();
                // The final answer is the review JSON itself; that is shown as the result.
                let is_answer = text.starts_with('{') || text.starts_with("```");
                (!text.is_empty() && !is_answer).then(|| one_line(text))
            }
            _ => None,
        })
        .collect()
}

/// Runs a read-only review in `repo` and returns the model's reply text.
/// With `progress`, Claude's activity is streamed to it as it happens.
/// `input_dir`, if given, is a second directory it may read.
pub async fn run(
    repo: &Path,
    prompt: &str,
    timeout: Duration,
    progress: Option<&ProgressTx>,
    input_dir: Option<&Path>,
) -> Result<String> {
    let extra: Vec<&Path> = input_dir.into_iter().collect();
    run_with(repo, prompt, timeout, progress, REVIEW_TOOLS, &extra).await
}

/// Runs Claude headless in `repo` with the given tools.
pub async fn run_with(
    repo: &Path,
    prompt: &str,
    timeout: Duration,
    progress: Option<&ProgressTx>,
    tools: Tools,
    extra_dirs: &[&Path],
) -> Result<String> {
    let stream = progress.is_some();
    let program = crate::git::find_tool("claude").unwrap_or_else(|| "claude".into());
    let mut child = tokio::process::Command::new(program)
        .args(build_args_for(stream, tools, extra_dirs))
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Dropping the future on timeout or cancellation must not leave Claude running.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!("`claude` was not found on PATH. Install the Claude Code CLI first.")
            } else {
                anyhow!("Could not start `claude`: {e}")
            }
        })?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let prompt_text = prompt.to_string();
    // Written from its own task so a full pipe can never stall reading the output.
    tokio::spawn(async move {
        let _ = stdin.write_all(prompt_text.as_bytes()).await;
        let _ = stdin.shutdown().await;
    });
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_task = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });

    let read = async {
        let mut lines = BufReader::new(stdout).lines();
        // The result envelope: the whole output in json mode, the final
        // `result` event in stream mode.
        let mut envelope = String::new();
        while let Some(line) = lines.next_line().await? {
            match progress {
                None => {
                    envelope.push_str(&line);
                    envelope.push('\n');
                }
                Some(tx) => {
                    let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
                    for activity in describe_event(&event, repo) {
                        let _ = tx.send(Progress::Activity(activity));
                    }
                    if event["type"] == "result" {
                        envelope = line;
                    }
                }
            }
        }
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, envelope))
    };

    let (status, envelope) = match tokio::time::timeout(timeout, read).await {
        Err(_) => bail!(
            "Claude timed out after {}s. Raise `review.timeout_seconds` in ~/.config/prr/config.toml for large PRs.",
            timeout.as_secs()
        ),
        Ok(result) => result.map_err(|e| anyhow!("Failed while reading from `claude`: {e}"))?,
    };
    let stderr = stderr_task.await.unwrap_or_default();

    if !status.success() || envelope.trim().is_empty() {
        // The reason is usually in the envelope.
        if !envelope.trim().is_empty() {
            extract_result(&envelope)?;
        }
        if stderr.contains("unknown option") {
            // Without these options the run would not be confined, so it does not run at all.
            bail!(
                "Your Claude Code CLI is too old for Warden's safety settings. Update it (run `claude update`) and try again. ({})",
                stderr.trim()
            );
        }
        bail!(
            "`claude` exited with {} and no usable result: {}",
            status,
            if stderr.trim().is_empty() { envelope.trim() } else { stderr.trim() }
        );
    }
    extract_result(&envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn value_after<'a>(args: &'a [String], flag: &str) -> &'a str {
        &args[args.iter().position(|a| a == flag).unwrap() + 1]
    }

    #[test]
    fn review_runs_are_confined_and_read_only() {
        let args = build_args(false);
        assert_eq!(&args[..3], ["-p", "--output-format", "json"]);
        assert!(!args.contains(&"--verbose".to_string()));
        for flag in ["--restricted", "--safe-mode", "--strict-mcp-config"] {
            assert!(args.contains(&flag.to_string()), "{flag} is what keeps untrusted repositories contained");
        }
        // No shell at all, and no way to change files.
        assert_eq!(value_after(&args, "--tools"), "Read,Grep,Glob");
        let denied: Vec<&str> = value_after(&args, "--disallowedTools").split(',').collect();
        for tool in ["Bash", "Edit", "Write", "WebFetch", "Task"] {
            assert!(denied.contains(&tool), "{tool}");
        }
        assert!(!args.iter().any(|a| a.contains("Bash(")), "no git or other commands, however read-only they look");
        assert!(!args.contains(&"--permission-mode".to_string()));
        assert!(!args.contains(&"--allowedTools".to_string()));
    }

    #[test]
    fn fix_runs_can_edit_but_are_just_as_confined() {
        let args = build_args_for(false, FIX_TOOLS, &[]);
        for flag in ["--restricted", "--safe-mode", "--strict-mcp-config"] {
            assert!(args.contains(&flag.to_string()));
        }
        assert_eq!(value_after(&args, "--tools"), "Read,Grep,Glob,Edit,Write");
        assert_eq!(value_after(&args, "--permission-mode"), "acceptEdits");
        let denied: Vec<&str> = value_after(&args, "--disallowedTools").split(',').collect();
        assert!(denied.contains(&"Bash") && denied.contains(&"WebFetch"));
        assert!(!denied.contains(&"Edit"));
    }

    #[test]
    fn extra_directories_come_last_and_streaming_keeps_the_same_limits() {
        let dir = Path::new("/cache/review-input/7");
        let args = build_args_for(true, REVIEW_TOOLS, &[dir]);
        assert_eq!(&args[1..4], ["--output-format", "stream-json", "--verbose"]);
        assert_eq!(&args[args.len() - 2..], ["--add-dir", "/cache/review-input/7"]);
        assert_eq!(args[4..args.len() - 2], build_args(false)[3..]);
    }

    #[test]
    fn describes_tool_use_and_narration() {
        let repo = Path::new("/cache/repo");
        let event = json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "text", "text": "Looking at the diff first.\nMore." },
                { "type": "tool_use", "name": "Read", "input": { "file_path": "/cache/repo/src/a.cs" } },
                { "type": "tool_use", "name": "Bash", "input": { "command": "git diff origin/main...origin/x" } },
                { "type": "tool_use", "name": "Grep", "input": { "pattern": "Foo\\(", "path": "/cache/repo/src" } },
                { "type": "text", "text": "{\"verdict\":\"approve\"}" }
            ]}
        });
        assert_eq!(
            describe_event(&event, repo),
            [
                "Looking at the diff first.",
                "Read src/a.cs",
                "Bash git diff origin/main...origin/x",
                "Grep Foo\\(",
            ]
        );
    }

    #[test]
    fn other_events_have_no_activity() {
        let repo = Path::new("/r");
        assert!(describe_event(&json!({"type": "system", "subtype": "init"}), repo).is_empty());
        assert!(describe_event(&json!({"type": "result", "result": "x"}), repo).is_empty());
        assert!(describe_event(&json!({"type": "assistant"}), repo).is_empty());
    }
}
