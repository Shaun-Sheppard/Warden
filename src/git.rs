use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use std::path::{Path, PathBuf};
use std::process::Command;

/// PAT-based HTTPS auth for git. The credential is handed to git through
/// environment-scoped config, so it never appears on a command line, in the
/// clone's config file, or anywhere else on disk.
pub struct GitAuth {
    header: String,
}

impl GitAuth {
    pub fn from_pat(pat: &str) -> Self {
        let token = base64::engine::general_purpose::STANDARD.encode(format!(":{pat}"));
        Self { header: format!("Authorization: Basic {token}") }
    }

    fn apply(&self, cmd: &mut Command) {
        // Scoped to Azure DevOps, so the token is not sent to any other host
        // git might contact (a redirect, an LFS server, a submodule).
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.https://dev.azure.com/.extraHeader")
            .env("GIT_CONFIG_VALUE_0", &self.header)
            .env("GIT_TERMINAL_PROMPT", "0");
    }
}

/// File names a command can have on this platform, most preferred first.
/// On Windows a real `.exe` is preferred over a `.cmd` shim, which cannot
/// be handed every kind of argument safely.
fn executable_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"].iter().map(|ext| format!("{name}.{ext}")).collect()
    } else {
        vec![name.to_string()]
    }
}

/// Full path of an external tool, searched for on PATH.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH")?).collect();
    executable_names(name)
        .iter()
        .find_map(|file| dirs.iter().map(|dir| dir.join(file)).find(|p| p.is_file()))
}

/// Checks that an external tool is on PATH before we depend on it.
pub fn ensure_tool(name: &str) -> Result<()> {
    if find_tool(name).is_some() {
        return Ok(());
    }
    let hint = match name {
        "claude" => "Install the Claude Code CLI and make sure `claude` is on your PATH.",
        "git" => "Install git (e.g. `xcode-select --install`) and make sure it is on your PATH.",
        _ => "Make sure it is installed and on your PATH.",
    };
    bail!("`{name}` was not found on PATH. {hint}")
}

/// Windows flag that stops a console program from opening its own window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command that runs without a window of its own. Warden is a windowed
/// app, so on Windows every git or claude it starts would otherwise flash
/// up a console window and take focus.
#[allow(unused_mut)]
pub fn background(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn git(repo: &Path) -> Command {
    let mut cmd = background("git");
    cmd.arg("-C").arg(repo);
    cmd
}

fn is_repo(dir: &Path) -> bool {
    dir.is_dir()
        && git(dir)
            .args(["rev-parse", "--git-dir"])
            .output()
            .is_ok_and(|out| out.status.success())
}

pub fn ensure_repo(repo: &Path) -> Result<()> {
    if !repo.is_dir() {
        bail!(
            "Local clone path {} does not exist. Check the [repos] mapping in ~/.config/prr/config.toml.",
            repo.display()
        );
    }
    if !is_repo(repo) {
        bail!("{} is not a git repository.", repo.display());
    }
    Ok(())
}

fn stdout_of(repo: &Path, args: &[&str]) -> Option<String> {
    let out = git(repo).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// URL of `origin` for the repo containing `dir`, if any.
pub fn origin_url(dir: &Path) -> Option<String> {
    stdout_of(dir, &["remote", "get-url", "origin"])
}

/// Root of the working tree containing `dir`, if any.
pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    stdout_of(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Refspecs that update only the remote-tracking refs for the given branches.
pub fn fetch_refspecs(branches: &[&str]) -> Vec<String> {
    branches
        .iter()
        .map(|b| format!("+refs/heads/{b}:refs/remotes/origin/{b}"))
        .collect()
}

/// Fetches the PR branches into `origin/*`. Never touches the working tree
/// or local branches. Without `auth`, git's own credentials are used.
pub fn fetch(repo: &Path, source: &str, target: &str, auth: Option<&GitAuth>) -> Result<()> {
    let mut cmd = git(repo);
    cmd.arg("fetch").arg("origin").args(fetch_refspecs(&[source, target]));
    if let Some(auth) = auth {
        auth.apply(&mut cmd);
    }
    let out = cmd.output().map_err(|e| anyhow!("Could not run git fetch: {e}"))?;
    if !out.status.success() {
        bail!(
            "`git fetch origin {source} {target}` failed in {}:\n{}\nCheck that `origin` points at the Azure DevOps repo and that your credentials work.",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Windows limits paths to 260 characters unless git is told otherwise, and
/// deep project trees inside the cache directory exceed that ("Filename too
/// long" on checkout). The setting has no effect on other systems.
const LONG_PATHS: (&str, &str) = ("core.longpaths", "true");

/// Brings prr's own clone at `dir` up to date and checks out the PR's source
/// branch (detached), cloning from `url` first if needed. `dir` is owned by
/// prr, so unlike a user's clone its working tree is ours to change.
pub fn sync_managed_clone(
    url: &str,
    dir: &Path,
    source: &str,
    target: &str,
    auth: Option<&GitAuth>,
) -> Result<()> {
    if dir.exists() && !is_repo(dir) {
        // Left over from an interrupted run.
        std::fs::remove_dir_all(dir)
            .with_context(|| format!("Could not remove broken clone {}", dir.display()))?;
    }
    if !dir.exists() {
        clone(url, dir, auth)?;
    }
    // Clones made before long paths were enabled need it switched on too.
    // Only ever done to prr's own clone, never to a user's.
    let _ = git(dir).args(["config", LONG_PATHS.0, LONG_PATHS.1]).output();
    fetch(dir, source, target, auth)?;

    let out = git(dir)
        .args(["checkout", "--quiet", "--detach", "--force"])
        .arg(format!("origin/{source}"))
        .output()
        .map_err(|e| anyhow!("Could not run git checkout: {e}"))?;
    if !out.status.success() {
        bail!(
            "Could not check out origin/{source} in {}:\n{}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn clone(url: &str, dir: &Path, auth: Option<&GitAuth>) -> Result<()> {
    let parent = dir.parent().context("clone directory has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Could not create {}", parent.display()))?;
    // Clone beside the final location and rename, so an interrupted clone
    // is never mistaken for a complete one.
    let mut partial = dir.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    if partial.exists() {
        std::fs::remove_dir_all(&partial)
            .with_context(|| format!("Could not remove {}", partial.display()))?;
    }

    let mut cmd = background("git");
    cmd.args(["clone", "--no-checkout", "--quiet", "-c"])
        .arg(format!("{}={}", LONG_PATHS.0, LONG_PATHS.1))
        .arg(url)
        .arg(&partial);
    if let Some(auth) = auth {
        auth.apply(&mut cmd);
    }
    let out = cmd.output().map_err(|e| anyhow!("Could not run git clone: {e}"))?;
    if !out.status.success() {
        std::fs::remove_dir_all(&partial).ok();
        bail!(
            "`git clone {url}` failed:\n{}\nCheck that your Personal Access Token has Code (Read) scope for this repository.",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    std::fs::rename(&partial, dir)
        .with_context(|| format!("Could not move the clone into {}", dir.display()))
}

fn run(mut cmd: Command, what: &str) -> Result<String> {
    let out = cmd.output().map_err(|e| anyhow!("Could not run git {what}: {e}"))?;
    if !out.status.success() {
        bail!("git {what} failed:\n{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Adds a separate working directory at `rev`, sharing `repo`'s objects.
/// Used so a fix can be prepared without disturbing the clone reviews run in.
pub fn worktree_add(repo: &Path, dir: &Path, rev: &str) -> Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("Could not create {}", parent.display()))?;
    }
    let _ = git(repo).args(["worktree", "prune"]).output();
    let mut cmd = git(repo);
    cmd.args(["worktree", "add", "--detach", "--force"]).arg(dir).arg(rev);
    run(cmd, "worktree add").map(|_| ())
}

/// Removes a working directory added with `worktree_add`, with anything in it.
pub fn worktree_remove(repo: &Path, dir: &Path) {
    let mut cmd = git(repo);
    cmd.args(["worktree", "remove", "--force"]).arg(dir);
    let _ = cmd.output();
    let _ = std::fs::remove_dir_all(dir);
    let _ = git(repo).args(["worktree", "prune"]).output();
}

/// The commit a working directory is at.
pub fn head(dir: &Path) -> Result<String> {
    let mut cmd = git(dir);
    cmd.args(["rev-parse", "HEAD"]);
    Ok(run(cmd, "rev-parse")?.trim().to_string())
}

/// One changed file in a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

/// Everything changed in a working directory since its commit, new files
/// included, as a unified diff plus per-file counts. Stages the changes.
pub fn pending_changes(dir: &Path) -> Result<(String, Vec<FileChange>)> {
    let mut add = git(dir);
    add.args(["add", "--all"]);
    run(add, "add")?;
    let mut diff = git(dir);
    diff.args(["diff", "--cached", "--no-color", "--no-ext-diff"]);
    let text = run(diff, "diff")?;
    let mut stat = git(dir);
    stat.args(["diff", "--cached", "--numstat"]);
    let files = run(stat, "diff --numstat")?
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            // Binary files report "-" for both counts.
            let additions = parts.next()?.parse().unwrap_or(0);
            let deletions = parts.next()?.parse().unwrap_or(0);
            Some(FileChange { path: parts.next()?.to_string(), additions, deletions })
        })
        .collect();
    Ok((text, files))
}

/// Throws away everything not staged: edits to tracked files and new files
/// that are not ignored. What is staged is kept. Used after running a
/// repository's tests, so files they write never end up in a commit.
pub fn discard_unstaged(dir: &Path) -> Result<()> {
    let mut checkout = git(dir);
    checkout.args(["checkout", "--", "."]);
    run(checkout, "checkout")?;
    let mut clean = git(dir);
    clean.args(["clean", "-fdq"]);
    run(clean, "clean").map(|_| ())
}

/// Folders never copied as local-only files: dependencies and build output,
/// which a fix's setup command recreates, and editor state.
pub const NOT_COPIED: [&str; 16] = [
    ".git", "node_modules", "bin", "obj", "dist", "out", "build", ".angular", ".vs", ".vscode", ".idea", "packages", "coverage",
    "TestResults", ".nx", ".cache",
];

/// Files in `dir` that git does not track: untracked ones and ignored ones
/// (local settings, `.env` files), with ignored folders listed once, and
/// dependencies and build output left out. Paths use `/`.
pub fn local_only_files(dir: &Path) -> Result<Vec<String>> {
    let list = |ignored: bool| -> Result<Vec<String>> {
        let mut cmd = git(dir);
        cmd.args(["ls-files", "--others", "--exclude-standard", "--directory", "-z"]);
        if ignored {
            cmd.arg("--ignored");
        }
        Ok(run(cmd, "ls-files")?.split('\0').filter(|p| !p.is_empty()).map(String::from).collect())
    };
    let mut paths = list(false)?;
    paths.extend(list(true)?);
    paths.retain(|p| !p.trim_end_matches('/').split('/').any(|part| NOT_COPIED.iter().any(|n| n.eq_ignore_ascii_case(part))));
    // Claude's worktrees live inside the checkout: never copy them into another one.
    // A wholly untracked `.claude/` is listed as one folder, so list what is in it instead.
    if let Some(at) = paths.iter().position(|p| p == ".claude/") {
        paths.remove(at);
        if let Ok(entries) = std::fs::read_dir(dir.join(".claude")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let slash = if entry.path().is_dir() { "/" } else { "" };
                paths.push(format!(".claude/{name}{slash}"));
            }
        }
    }
    paths.retain(|p| !p.starts_with(".claude/worktrees"));
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Adds new, not-ignored dependency and build-output folders in `dir`
/// (`bin`, `obj`, `TestResults`, `dist`...) to the clone's exclude list, so
/// what a build or test run left behind is never committed.
pub fn exclude_build_output(dir: &Path) -> Result<()> {
    let mut others = git(dir);
    others.args(["ls-files", "--others", "--exclude-standard", "-z"]);
    let mut folders: Vec<String> = Vec::new();
    for path in run(others, "ls-files")?.split('\0').filter(|p| !p.is_empty()) {
        let parts: Vec<&str> = path.split('/').collect();
        // Only folders count: a file that happens to be called `build` is real work.
        if let Some(at) = parts[..parts.len() - 1].iter().position(|part| NOT_COPIED.iter().any(|n| n.eq_ignore_ascii_case(part))) {
            let folder = format!("/{}/", parts[..=at].join("/"));
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
    }
    if folders.is_empty() {
        return Ok(());
    }
    let mut common = git(dir);
    common.args(["rev-parse", "--git-common-dir"]);
    let common = PathBuf::from(run(common, "rev-parse")?.trim());
    let common = if common.is_absolute() { common } else { dir.join(common) };
    let exclude = common.join("info").join("exclude");
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for folder in folders {
        if !text.lines().any(|l| l == folder) {
            text.push_str(&folder);
            text.push('\n');
        }
    }
    std::fs::create_dir_all(exclude.parent().unwrap()).with_context(|| format!("Could not create {}", common.display()))?;
    std::fs::write(&exclude, text).with_context(|| format!("Could not write {}", exclude.display()))
}

/// Adds a line to the repository's own exclude list (`.git/info/exclude`), if not there.
pub fn exclude_locally(repo: &Path, pattern: &str) -> Result<()> {
    let mut common = git(repo);
    common.args(["rev-parse", "--git-common-dir"]);
    let common = PathBuf::from(run(common, "rev-parse")?.trim());
    let common = if common.is_absolute() { common } else { repo.join(common) };
    let exclude = common.join("info").join("exclude");
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if text.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(pattern);
    text.push('\n');
    std::fs::create_dir_all(exclude.parent().unwrap()).with_context(|| format!("Could not create {}", common.display()))?;
    std::fs::write(&exclude, text).with_context(|| format!("Could not write {}", exclude.display()))
}

/// Adds a worktree at `dir` on `branch`: a new branch from `start` if it does not
/// exist yet, otherwise the existing branch.
pub fn worktree_add_branch(repo: &Path, dir: &Path, branch: &str, start: &str) -> Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("Could not create {}", parent.display()))?;
    }
    let _ = git(repo).args(["worktree", "prune"]).output();
    let mut exists = git(repo);
    exists.args(["rev-parse", "--verify", "--quiet"]).arg(format!("refs/heads/{branch}"));
    let has_branch = exists.output().map(|o| o.status.success()).unwrap_or(false);
    let mut cmd = git(repo);
    cmd.args(["worktree", "add"]);
    if has_branch {
        cmd.arg(dir).arg(branch);
    } else {
        cmd.arg("-b").arg(branch).arg(dir).arg(start);
    }
    run(cmd, "worktree add").map(|_| ())
}

/// Keeps everything currently changed or new in `dir` out of commits, for
/// good: changed tracked files are marked skip-worktree, and new files (or
/// folders) are added to the clone's exclude list. Used on local files copied
/// into a fix's worktree and on what its setup command writes, such as
/// `node_modules` or an updated lock file. Returns the paths it protected.
pub fn protect_local_changes(dir: &Path) -> Result<Vec<String>> {
    let mut changed = git(dir);
    changed.args(["diff", "--name-only", "-z"]);
    let tracked: Vec<String> = run(changed, "diff --name-only")?.split('\0').filter(|p| !p.is_empty()).map(String::from).collect();
    if !tracked.is_empty() {
        let mut skip = git(dir);
        skip.args(["update-index", "--skip-worktree", "--"]).args(&tracked);
        run(skip, "update-index")?;
    }

    let mut others = git(dir);
    others.args(["ls-files", "--others", "--exclude-standard", "--directory", "-z"]);
    let untracked: Vec<String> = run(others, "ls-files")?.split('\0').filter(|p| !p.is_empty()).map(String::from).collect();
    if !untracked.is_empty() {
        let mut common = git(dir);
        common.args(["rev-parse", "--git-common-dir"]);
        let common = PathBuf::from(run(common, "rev-parse")?.trim());
        let common = if common.is_absolute() { common } else { dir.join(common) };
        let exclude = common.join("info").join("exclude");
        let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
        let mut text = existing.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for path in &untracked {
            let pattern = format!("/{path}");
            if !existing.lines().any(|l| l == pattern) {
                text.push_str(&pattern);
                text.push('\n');
            }
        }
        if text != existing {
            std::fs::create_dir_all(exclude.parent().unwrap()).with_context(|| format!("Could not create {}", common.display()))?;
            std::fs::write(&exclude, text).with_context(|| format!("Could not write {}", exclude.display()))?;
        }
    }
    Ok(tracked.into_iter().chain(untracked).collect())
}

/// Commits the staged changes and returns the new commit id.
pub fn commit(dir: &Path, name: &str, email: &str, message: &str) -> Result<String> {
    let mut cmd = git(dir);
    cmd.arg("-c")
        .arg(format!("user.name={name}"))
        .arg("-c")
        .arg(format!("user.email={email}"))
        .args(["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "-m", message]);
    run(cmd, "commit")?;
    head(dir)
}

/// The commit `branch` is at on the remote right now, if it exists.
pub fn remote_head(repo: &Path, branch: &str, auth: Option<&GitAuth>) -> Result<Option<String>> {
    let mut cmd = git(repo);
    cmd.args(["ls-remote", "origin"]).arg(format!("refs/heads/{branch}"));
    if let Some(auth) = auth {
        auth.apply(&mut cmd);
    }
    Ok(run(cmd, "ls-remote")?.split_whitespace().next().map(str::to_string))
}

/// Pushes the working directory's commit to `branch`. Never forces: if the
/// branch has moved on, the push is refused.
pub fn push_head(dir: &Path, branch: &str, auth: Option<&GitAuth>) -> Result<()> {
    let mut cmd = git(dir);
    // --no-verify: no hook in the clone gets a say in what is pushed.
    cmd.args(["push", "--quiet", "--no-verify", "origin"]).arg(format!("HEAD:refs/heads/{branch}"));
    if let Some(auth) = auth {
        auth.apply(&mut cmd);
    }
    run(cmd, "push").map(|_| ())
}

/// Writes the pull request's change into `dir` as files, for a reviewer
/// that has no git access: the changed files, the full diff, the commits.
pub fn write_review_input(repo: &Path, target: &str, source: &str, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let changed = format!("origin/{target}...origin/{source}");
    let commits = format!("origin/{target}..origin/{source}");
    let outputs: [(&str, Vec<&str>); 3] = [
        ("files.txt", vec!["diff", "--no-color", "--no-ext-diff", "--stat=200", "--stat-count=5000", &changed]),
        ("diff.patch", vec!["diff", "--no-color", "--no-ext-diff", &changed]),
        ("commits.txt", vec!["log", "--no-color", "--date=short", "--format=%h  %ad  %an%n    %s%n%w(0,4,4)%b", &commits]),
    ];
    for (name, args) in outputs {
        let mut cmd = git(repo);
        cmd.args(&args);
        let text = run(cmd, args[0])?;
        std::fs::write(dir.join(name), text).with_context(|| format!("Could not write {name}"))?;
    }
    Ok(())
}

/// Copies the files of `rev` into `dir` without touching the repository's
/// working tree, index or branches.
pub fn snapshot(repo: &Path, rev: &str, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let archive = dir.with_extension("tar");
    let mut cmd = git(repo);
    cmd.args(["archive", "--format=tar", "-o"]).arg(&archive).arg(rev);
    run(cmd, "archive")?;
    let unpacked = background("tar").arg("-xf").arg(&archive).arg("-C").arg(dir).output();
    let _ = std::fs::remove_file(&archive);
    let out = unpacked.map_err(|e| anyhow!("Could not run tar: {e}"))?;
    if !out.status.success() {
        bail!("Could not unpack the pull request's files:\n{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// The user's own git email, if they have one configured.
pub fn configured_email() -> Option<String> {
    let out = background("git").args(["config", "--get", "user.email"]).output().ok()?;
    let email = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !email.is_empty()).then_some(email)
}

/// Size of the change a PR makes: (files changed, lines added, lines deleted).
pub fn diff_stat(repo: &Path, target: &str, source: &str) -> Option<(u32, u32, u32)> {
    let range = format!("origin/{target}...origin/{source}");
    parse_shortstat(&stdout_of(repo, &["diff", "--shortstat", &range])?)
}

fn parse_shortstat(text: &str) -> Option<(u32, u32, u32)> {
    let mut stats = (0, 0, 0);
    for part in text.split(',') {
        let mut words = part.split_whitespace();
        let n: u32 = words.next()?.parse().ok()?;
        match words.next()? {
            w if w.starts_with("file") => stats.0 = n,
            w if w.starts_with("insertion") => stats.1 = n,
            w if w.starts_with("deletion") => stats.2 = n,
            _ => {}
        }
    }
    Some(stats)
}

/// Contents of `file` at `rev`, or None if it does not exist there.
pub fn show_file(repo: &Path, rev: &str, file: &str) -> Option<String> {
    let out = git(repo)
        .arg("show")
        .arg(format!("{rev}:{file}"))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refspecs_only_touch_remote_tracking_refs() {
        assert_eq!(
            fetch_refspecs(&["feature/x", "main"]),
            vec![
                "+refs/heads/feature/x:refs/remotes/origin/feature/x",
                "+refs/heads/main:refs/remotes/origin/main",
            ]
        );
    }

    #[test]
    fn the_token_is_only_offered_to_azure_devops() {
        let mut cmd = Command::new("git");
        GitAuth::from_pat("secret").apply(&mut cmd);
        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();
        assert_eq!(envs["GIT_CONFIG_KEY_0"], "http.https://dev.azure.com/.extraHeader");
        assert!(envs["GIT_CONFIG_VALUE_0"].starts_with("Authorization: Basic "));
        assert!(!envs["GIT_CONFIG_VALUE_0"].contains("secret"), "sent encoded, never in the clear");
        assert_eq!(envs["GIT_CONFIG_COUNT"], "1");
    }

    #[test]
    fn parses_shortstat() {
        assert_eq!(parse_shortstat(" 9 files changed, 412 insertions(+), 58 deletions(-)"), Some((9, 412, 58)));
        assert_eq!(parse_shortstat(" 1 file changed, 1 insertion(+)"), Some((1, 1, 0)));
        assert_eq!(parse_shortstat(" 2 files changed, 3 deletions(-)"), Some((2, 0, 3)));
        assert_eq!(parse_shortstat(""), None);
    }

    #[test]
    fn missing_tool_is_a_clear_error() {
        let err = ensure_tool("definitely-not-a-real-tool-xyz").unwrap_err().to_string();
        assert!(err.contains("not found on PATH"));
    }
}
