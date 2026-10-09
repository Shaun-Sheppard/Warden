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

/// The reviewer has no git access, so the change is handed over as files.
#[test]
fn the_change_is_written_out_as_files_for_the_reviewer() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    commit(&origin, "base.txt", "base\n");
    git(&origin, &["checkout", "-q", "-b", "feature/x"]);
    commit(&origin, "new.txt", "one\ntwo\n");
    let clone = tmp.path().join("repos/r");
    sync_managed_clone(origin.to_str().unwrap(), &clone, "feature/x", "main", None).unwrap();

    let input = tmp.path().join("review-input/change");
    prr::git::write_review_input(&clone, "main", "feature/x", &input).unwrap();

    let read = |name: &str| std::fs::read_to_string(input.join(name)).unwrap();
    assert!(read("files.txt").contains("new.txt"));
    assert!(read("diff.patch").contains("+one\n+two"));
    assert!(!read("diff.patch").contains("base.txt"), "only the PR's own change, not the target's history");
    assert!(read("commits.txt").contains("new.txt"), "{}", read("commits.txt"));
    assert!(!read("commits.txt").contains("base.txt"));
}

/// A user's own clone is never checked out; the PR's files are copied out of it.
#[test]
fn snapshot_copies_the_pr_files_without_touching_the_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("mine");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    commit(&repo, "base.txt", "base\n");
    git(&repo, &["checkout", "-q", "-b", "feature/x"]);
    std::fs::create_dir(repo.join("src")).unwrap();
    commit(&repo, "src/new.txt", "pr version\n");
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("base.txt"), "uncommitted work\n").unwrap();
    let before = git(&repo, &["status", "--porcelain"]);

    let files = tmp.path().join("scratch/files");
    prr::git::snapshot(&repo, "feature/x", &files).unwrap();

    assert_eq!(std::fs::read_to_string(files.join("src/new.txt")).unwrap(), "pr version\n");
    assert_eq!(std::fs::read_to_string(files.join("base.txt")).unwrap(), "base\n");
    assert!(!files.join(".git").exists());
    // The user's branch, working tree and uncommitted change are exactly as they were.
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(git(&repo, &["status", "--porcelain"]), before);
    assert_eq!(std::fs::read_to_string(repo.join("base.txt")).unwrap(), "uncommitted work\n");
    assert!(!tmp.path().join("scratch/files.tar").exists());
}
