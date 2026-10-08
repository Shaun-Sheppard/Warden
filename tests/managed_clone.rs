//! prr's own clone: created on first use, updated afterwards, PR checked out.

use std::path::Path;
use std::process::Command;

use prr::git::sync_managed_clone;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, content: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

#[test]
fn clones_then_updates_and_checks_out_the_pr_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    commit(&origin, "base.txt", "base");
    git(&origin, &["checkout", "-q", "-b", "feature/x"]);
    commit(&origin, "new.txt", "v1");

    let clone = tmp.path().join("cache/repos/repo-id");
    let url = origin.to_str().unwrap();

    sync_managed_clone(url, &clone, "feature/x", "main", None).unwrap();
    assert_eq!(std::fs::read_to_string(clone.join("new.txt")).unwrap(), "v1");
    assert_eq!(git(&clone, &["diff", "--name-only", "origin/main...origin/feature/x"]), "new.txt");
    assert!(!tmp.path().join("cache/repos/repo-id.partial").exists());
    // Needed on Windows, where deep trees exceed the default path limit.
    assert_eq!(git(&clone, &["config", "core.longpaths"]), "true");

    // A clone made by an older version gets the setting on its next use.
    git(&clone, &["config", "--unset", "core.longpaths"]);

    // A later review picks up new commits on the PR branch.
    commit(&origin, "new.txt", "v2");
    sync_managed_clone(url, &clone, "feature/x", "main", None).unwrap();
    assert_eq!(std::fs::read_to_string(clone.join("new.txt")).unwrap(), "v2");
    assert_eq!(git(&clone, &["config", "core.longpaths"]), "true");
    assert_eq!(
        git(&clone, &["rev-parse", "HEAD"]),
        git(&origin, &["rev-parse", "feature/x"])
    );

    // A broken directory left by an interrupted run is replaced.
    std::fs::remove_dir_all(clone.join(".git")).unwrap();
    sync_managed_clone(url, &clone, "feature/x", "main", None).unwrap();
    assert_eq!(std::fs::read_to_string(clone.join("new.txt")).unwrap(), "v2");
}

#[test]
fn clone_failure_is_reported_and_leaves_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("repos/x");
    let missing = tmp.path().join("does-not-exist");

    let err = sync_managed_clone(missing.to_str().unwrap(), &clone, "a", "b", None).unwrap_err();

    assert!(err.to_string().contains("git clone"));
    assert!(!clone.exists());
    assert!(!tmp.path().join("repos/x.partial").exists());
}
