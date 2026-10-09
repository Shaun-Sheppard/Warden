#![cfg(unix)]
//! Runs `claude::run` against a stand-in `claude` executable to check both
//! output modes. One test only: it changes PATH for the whole process.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use prr::claude;
use prr::pipeline::Progress;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
# The prompt arrives on standard input, the options as arguments.
prompt=$(cat)
# Every run must carry the options that confine an untrusted repository.
case "$*" in
  *--restricted*--safe-mode*--strict-mcp-config*) ;;
  *) echo '{"type":"result","is_error":true,"result":"run was not confined"}'; exit 1 ;;
esac
case "$prompt $*" in
  *FAIL*)
    echo '{"type":"result","is_error":true,"result":"Not logged in"}'
    exit 1 ;;
  *SLOW*)
    sleep 30 ;;
  *stream-json*)
    echo '{"type":"system","subtype":"init"}'
    echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"git diff origin/main...origin/x"}}]}}'
    echo 'not json'
    echo '{"type":"result","is_error":false,"result":"streamed answer"}' ;;
  *)
    printf '{"type":"result",\n "is_error":false,"result":"plain answer"}\n' ;;
esac
"#;

#[tokio::test]
async fn json_and_stream_modes_timeouts_and_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("claude");
    std::fs::write(&fake, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", tmp.path().display(), std::env::var("PATH").unwrap());
    std::env::set_var("PATH", path);
    let repo = tmp.path();
    let long = Duration::from_secs(20);

    let plain = claude::run(repo, "review", long, None, None).await.unwrap();
    assert_eq!(plain, "plain answer");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let streamed = claude::run(repo, "review", long, Some(&tx), Some(repo)).await.unwrap();
    assert_eq!(streamed, "streamed answer");
    assert_eq!(
        rx.try_recv().unwrap(),
        Progress::Activity("Bash git diff origin/main...origin/x".into())
    );
    assert!(rx.try_recv().is_err());

    let err = claude::run(repo, "FAIL", long, None, None).await.unwrap_err().to_string();
    assert!(err.contains("Not logged in"), "{err}");
    let err = claude::run(repo, "FAIL", long, Some(&tx), None).await.unwrap_err().to_string();
    assert!(err.contains("Not logged in"), "{err}");

    let started = std::time::Instant::now();
    let err = claude::run(repo, "SLOW", Duration::from_millis(300), None, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("timed out"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
