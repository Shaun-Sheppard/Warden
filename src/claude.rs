use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::pipeline::{Progress, ProgressTx};
use crate::review::extract_result;

/// Read-only tool allowlist for the headless review.
pub const ALLOWED_TOOLS: &str =
    "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)";
/// Denied explicitly so a permissive user/project settings file cannot re-enable them.
pub const DISALLOWED_TOOLS: &str = "Edit,Write,NotebookEdit";

/// Arguments for a headless run. The prompt itself is sent on standard
/// input: it is long and multi-line, which command lines (Windows `.cmd`
/// shims especially) cannot carry reliably.
/// `stream` asks for one JSON event per line, so tool activity can be shown live.
pub fn build_args(stream: bool) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        if stream { "stream-json" } else { "json" }.to_string(),
    ];
    if stream {
        // Required by the CLI for stream-json in print mode.
        args.push("--verbose".to_string());
    }
    args.extend([
        "--allowedTools".to_string(),
        ALLOWED_TOOLS.to_string(),
        "--disallowedTools".to_string(),
        DISALLOWED_TOOLS.to_string(),
    ]);
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

/// Runs Claude headless in `repo` and returns the model's reply text.
/// With `progress`, Claude's activity is streamed to it as it happens.
pub async fn run(
    repo: &Path,
    prompt: &str,
    timeout: Duration,
    progress: Option<&ProgressTx>,
) -> Result<String> {
    let stream = progress.is_some();
    let program = crate::git::find_tool("claude").unwrap_or_else(|| "claude".into());
    let mut child = tokio::process::Command::new(program)
        .args(build_args(stream))
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

    #[test]
    fn args_are_headless_json_and_read_only() {
        let args = build_args(false);
        assert_eq!(&args[..3], ["-p", "--output-format", "json"]);
        assert!(!args.contains(&"--verbose".to_string()));
        let allowed = &args[args.iter().position(|a| a == "--allowedTools").unwrap() + 1];
        assert_eq!(
            allowed,
            "Read,Grep,Glob,Bash(git diff:*),Bash(git log:*),Bash(git show:*)"
        );
        for tool in ["Edit", "Write"] {
            assert!(!allowed.split(',').any(|t| t == tool));
        }
    }

    #[test]
    fn streaming_args_keep_the_same_tool_limits() {
        let args = build_args(true);
        assert_eq!(&args[1..4], ["--output-format", "stream-json", "--verbose"]);
        assert_eq!(args[4..], build_args(false)[3..]);
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
