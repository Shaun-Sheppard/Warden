//! The git side of "Fix with Claude": prepare a change in a separate
//! working directory, show it, commit it, and push it without forcing.

use std::path::Path;
use std::process::Command;

use prr::git;

fn sh(dir: &Path, args: &[&str]) -> String {
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

fn commit_file(dir: &Path, file: &str, content: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", file]);
}

/// An origin with `main` and `feature/x`, and prr's clone of it.
fn setup(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let origin = tmp.join("origin");
    std::fs::create_dir(&origin).unwrap();
    sh(&origin, &["init", "-q", "-b", "main"]);
    commit_file(&origin, "base.txt", "base\n");
    sh(&origin, &["checkout", "-q", "-b", "feature/x"]);
    commit_file(&origin, "calc.py", "def add(a, b):\n    return a - b\n");
    // A branch that is checked out cannot be pushed to.
    sh(&origin, &["checkout", "-q", "main"]);
    let clone = tmp.join("cache/repos/repo");
    git::sync_managed_clone(origin.to_str().unwrap(), &clone, "feature/x", "main", None).unwrap();
    (origin, clone)
}

#[test]
fn a_fix_is_prepared_shown_committed_and_pushed() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, clone) = setup(tmp.path());
    let work = tmp.path().join("cache/fixes/fix-1");

    git::worktree_add(&clone, &work, "origin/feature/x").unwrap();
    let base = git::head(&work).unwrap();
    assert_eq!(base, sh(&origin, &["rev-parse", "feature/x"]));
    assert_eq!(git::remote_head(&clone, "feature/x", None).unwrap(), Some(base.clone()));
    assert_eq!(git::remote_head(&clone, "no-such-branch", None).unwrap(), None);

    // Nothing changed yet.
    assert_eq!(git::pending_changes(&work).unwrap(), (String::new(), vec![]));

    std::fs::write(work.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
    std::fs::write(work.join("test_calc.py"), "assert add(1, 2) == 3\n").unwrap();
    let (diff, files) = git::pending_changes(&work).unwrap();
    assert!(diff.contains("-    return a - b") && diff.contains("+    return a + b"), "{diff}");
    assert!(diff.contains("test_calc.py"), "new files must be in the diff");
    assert_eq!(
        files,
        vec![
            git::FileChange { path: "calc.py".into(), additions: 1, deletions: 1 },
            git::FileChange { path: "test_calc.py".into(), additions: 1, deletions: 0 },
        ]
    );
    // Preparing a fix leaves the review clone and the remote untouched.
    assert_eq!(std::fs::read_to_string(clone.join("calc.py")).unwrap(), "def add(a, b):\n    return a - b\n");
    assert_eq!(sh(&origin, &["rev-parse", "feature/x"]), base);

    let sha = git::commit(&work, "Ann Author", "ann@example.com", "Fix review issues").unwrap();
    assert_ne!(sha, base);
    git::push_head(&work, "feature/x", None).unwrap();
    assert_eq!(sh(&origin, &["rev-parse", "feature/x"]), sha);
    assert_eq!(sh(&origin, &["log", "-1", "--format=%an <%ae>|%s", "feature/x"]), "Ann Author <ann@example.com>|Fix review issues");
    assert_eq!(sh(&origin, &["rev-parse", "feature/x~1"]), base);

    git::worktree_remove(&clone, &work);
    assert!(!work.exists());
    assert!(!sh(&clone, &["worktree", "list"]).contains("fix-1"));
}

#[test]
fn a_fix_is_refused_if_the_branch_has_moved_on() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, clone) = setup(tmp.path());
    let work = tmp.path().join("cache/fixes/fix-2");
    git::worktree_add(&clone, &work, "origin/feature/x").unwrap();
    let base = git::head(&work).unwrap();

    // Someone pushes to the PR while the fix waits for approval.
    sh(&origin, &["checkout", "-q", "feature/x"]);
    commit_file(&origin, "other.txt", "theirs\n");
    sh(&origin, &["checkout", "-q", "main"]);
    let theirs = sh(&origin, &["rev-parse", "feature/x"]);
    assert_ne!(git::remote_head(&clone, "feature/x", None).unwrap(), Some(base));

    std::fs::write(work.join("calc.py"), "fixed\n").unwrap();
    git::pending_changes(&work).unwrap();
    git::commit(&work, "A", "a@example.com", "fix").unwrap();
    let err = git::push_head(&work, "feature/x", None).unwrap_err().to_string();
    assert!(err.contains("push failed"), "{err}");
    // Their commit is still the tip: nothing was overwritten.
    assert_eq!(sh(&origin, &["rev-parse", "feature/x"]), theirs);
}

#[test]
fn discarding_removes_the_working_directory_and_its_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, clone) = setup(tmp.path());
    let work = tmp.path().join("cache/fixes/fix-3");
    git::worktree_add(&clone, &work, "origin/feature/x").unwrap();
    let base = git::head(&work).unwrap();
    std::fs::write(work.join("calc.py"), "half-finished\n").unwrap();

    git::worktree_remove(&clone, &work);

    assert!(!work.exists());
    assert_eq!(sh(&origin, &["rev-parse", "feature/x"]), base);
    // The same location can be used again.
    git::worktree_add(&clone, &work, "origin/feature/x").unwrap();
    assert_eq!(std::fs::read_to_string(work.join("calc.py")).unwrap(), "def add(a, b):\n    return a - b\n");
}
