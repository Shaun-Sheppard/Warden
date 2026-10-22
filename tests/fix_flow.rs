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

#[test]
fn local_files_and_setup_output_stay_out_of_the_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (_origin, clone) = setup(tmp.path());

    // The developer's own checkout, with local-only files git does not track.
    let local = tmp.path().join("local");
    sh(tmp.path(), &["clone", "-q", clone.to_str().unwrap(), local.to_str().unwrap()]);
    std::fs::write(local.join(".gitignore"), "appsettings.Development.json\nnode_modules/\n").unwrap();
    std::fs::write(local.join("appsettings.Development.json"), "{\"db\":\"local\"}").unwrap();
    std::fs::create_dir_all(local.join("node_modules/pkg")).unwrap();
    std::fs::write(local.join("node_modules/pkg/index.js"), "x").unwrap();
    std::fs::write(local.join("notes.txt"), "mine").unwrap();
    let found = git::local_only_files(&local).unwrap();
    assert!(found.contains(&"appsettings.Development.json".to_string()), "{found:?}");
    assert!(found.contains(&"notes.txt".to_string()), "{found:?}");
    assert!(!found.iter().any(|p| p.contains("node_modules")), "{found:?}");

    // A fix's worktree: a local copy of a tracked file, a local-only file, and setup output.
    let work = tmp.path().join("work");
    git::worktree_add(&clone, &work, "origin/feature/x").unwrap();
    std::fs::write(work.join("base.txt"), "local override\n").unwrap();
    std::fs::write(work.join("appsettings.Development.json"), "{}").unwrap();
    std::fs::create_dir_all(work.join("node_modules")).unwrap();
    std::fs::write(work.join("node_modules/x.js"), "x").unwrap();
    let protected = git::protect_local_changes(&work).unwrap();
    assert!(protected.contains(&"base.txt".to_string()), "{protected:?}");

    // Claude's change, then the tests write something and are cleaned up after.
    std::fs::write(work.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
    let (_, files) = git::pending_changes(&work).unwrap();
    assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["calc.py"]);
    std::fs::write(work.join("test-output.log"), "x").unwrap();
    git::discard_unstaged(&work).unwrap();
    assert_eq!(std::fs::read_to_string(work.join("base.txt")).unwrap(), "local override\n");
    assert!(work.join("appsettings.Development.json").exists());
    assert!(work.join("node_modules/x.js").exists());
    assert!(!work.join("test-output.log").exists());

    let commit = git::commit(&work, "t", "t@example.com", "fix").unwrap();
    let changed = sh(&work, &["show", "--name-only", "--format=", &commit]);
    assert_eq!(changed, "calc.py");
}

#[test]
fn a_session_worktree_comes_from_the_base_branch_and_is_never_copied() {
    let tmp = tempfile::tempdir().unwrap();
    let (_origin, clone) = setup(tmp.path());
    let local = tmp.path().join("mine");
    sh(tmp.path(), &["clone", "-q", clone.to_str().unwrap(), local.to_str().unwrap()]);

    // A worktree for an item, on a new branch from the base branch, kept out of `git status`.
    git::exclude_locally(&local, "/.claude/worktrees/").unwrap();
    git::exclude_locally(&local, "/.claude/worktrees/").unwrap();
    let work = local.join(".claude/worktrees/4102-page-heading");
    git::worktree_add_branch(&local, &work, "4102-page-heading", "origin/main").unwrap();
    assert_eq!(sh(&work, &["rev-parse", "--abbrev-ref", "HEAD"]), "4102-page-heading");
    assert_eq!(sh(&local, &["status", "--porcelain"]), "");
    let exclude = std::fs::read_to_string(local.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/.claude/worktrees/").count(), 1);

    // Asking again for an existing branch reuses it rather than failing.
    std::fs::remove_dir_all(&work).unwrap();
    git::worktree_add_branch(&local, &work, "4102-page-heading", "origin/main").unwrap();

    // Local-only files never include Claude's worktrees, even when `.claude/` is untracked.
    std::fs::write(local.join(".claude/settings.local.json"), "{}").unwrap();
    let found = git::local_only_files(&local).unwrap();
    assert!(found.contains(&".claude/settings.local.json".to_string()), "{found:?}");
    assert!(!found.iter().any(|p| p.contains("worktrees")), "{found:?}");
}
