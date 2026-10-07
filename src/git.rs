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
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.extraHeader")
            .env("GIT_CONFIG_VALUE_0", &self.header)
            .env("GIT_TERMINAL_PROMPT", "0");
    }
}

/// Checks that an external tool is on PATH before we depend on it.
pub fn ensure_tool(name: &str) -> Result<()> {
    let found = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false);
    if found {
        return Ok(());
    }
    let hint = match name {
        "claude" => "Install the Claude Code CLI and make sure `claude` is on your PATH.",
        "git" => "Install git (e.g. `xcode-select --install`) and make sure it is on your PATH.",
        _ => "Make sure it is installed and on your PATH.",
    };
    bail!("`{name}` was not found on PATH. {hint}")
}

fn git(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
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

    let mut cmd = Command::new("git");
    cmd.args(["clone", "--no-checkout", "--quiet", url]).arg(&partial);
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
