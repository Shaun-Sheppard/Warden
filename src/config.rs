use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const DEFAULT_COMMENT_PREFIX: &str = "🤖 AI-assisted review:";
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub organization: String,
    pub project: String,
    #[serde(default)]
    pub default_repo: Option<String>,
    /// Only show (and auto-review) PRs raised by these people. Empty = everyone.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Optional: Azure DevOps repo name -> existing local clone to review in,
    /// instead of prr's own managed clone.
    #[serde(default)]
    pub repos: BTreeMap<String, String>,
    #[serde(default)]
    pub review: ReviewConfig,
    #[serde(default)]
    pub auto: AutoConfig,
}

pub const MERGE_STRATEGIES: [&str; 4] = ["noFastForward", "squash", "rebase", "rebaseMerge"];

/// Extra actions `prr auto` may take on PRs with no critical or major issues.
/// Both are off unless enabled here or on the command line.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AutoConfig {
    /// Vote to approve.
    pub approve: bool,
    /// Set the PR to auto-complete.
    pub autocomplete: bool,
    /// Merge strategy for auto-complete; the repository default if unset.
    pub merge_strategy: Option<String>,
    pub delete_source_branch: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Review guidance given inline; takes precedence over `prompt_file`.
    pub prompt: Option<String>,
    pub prompt_file: Option<String>,
    pub timeout_seconds: u64,
    pub comment_prefix: String,
    /// Tag the PR's author in every posted comment.
    pub mention_author: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            prompt: None,
            prompt_file: None,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            comment_prefix: DEFAULT_COMMENT_PREFIX.to_string(),
            mention_author: true,
        }
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set; cannot locate config and cache directories"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(home()?.join(".config/prr/config.toml"))
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(home()?.join(".cache/prr"))
}

pub fn expand_tilde(path: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (path, home) {
        ("~", Some(home)) => home,
        (p, Some(home)) if p.starts_with("~/") => home.join(&p[2..]),
        _ => PathBuf::from(path),
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        if config.organization.trim().is_empty() {
            bail!("`organization` must not be empty");
        }
        if config.organization.contains('/') {
            bail!(
                "`organization` must be the organization name only (e.g. \"my-org\"), not a URL: {}",
                config.organization
            );
        }
        if config.project.trim().is_empty() {
            bail!("`project` must not be empty");
        }
        if config.review.timeout_seconds == 0 {
            bail!("`review.timeout_seconds` must be greater than 0");
        }
        if let Some(strategy) = &config.auto.merge_strategy {
            if !MERGE_STRATEGIES.contains(&strategy.as_str()) {
                bail!(
                    "`auto.merge_strategy` must be one of {}, not \"{strategy}\"",
                    MERGE_STRATEGIES.join(", ")
                );
            }
        }
        Ok(config)
    }

    pub fn load() -> Result<Self> {
        let path = config_path()?;
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!(
                    "Config file not found at {}. Run `prr init` to create it.",
                    path.display()
                )
            } else {
                anyhow!("Could not read config file {}: {e}", path.display())
            }
        })?;
        Self::parse(&text).with_context(|| format!("Invalid config file {}", path.display()))
    }

    /// User-provided local clone for an Azure DevOps repo, if one is mapped.
    /// Repo names are case-insensitive in Azure DevOps, so the lookup is too.
    pub fn repo_path(&self, repo: &str) -> Option<PathBuf> {
        self.repos
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(repo))
            .map(|(_, path)| expand_tilde(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
organization = "my-org"
project = "my-project"
default_repo = "my-repo"
authors = ["Shaun Sheppard", "ann@example.com"]

[repos]
my-repo = "~/code/my-repo"
Other = "/abs/other"

[review]
prompt_file = "~/.config/prr/review-prompt.md"
timeout_seconds = 120
comment_prefix = "[bot]"
"#;

    #[test]
    fn parses_full_config() {
        let c = Config::parse(FULL).unwrap();
        assert_eq!(c.organization, "my-org");
        assert_eq!(c.project, "my-project");
        assert_eq!(c.default_repo.as_deref(), Some("my-repo"));
        assert_eq!(c.authors, ["Shaun Sheppard", "ann@example.com"]);
        assert_eq!(c.repos.len(), 2);
        assert_eq!(c.review.timeout_seconds, 120);
        assert_eq!(c.review.comment_prefix, "[bot]");
        assert_eq!(
            c.review.prompt_file.as_deref(),
            Some("~/.config/prr/review-prompt.md")
        );
    }

    #[test]
    fn applies_defaults() {
        let c = Config::parse("organization = \"o\"\nproject = \"p\"\n").unwrap();
        assert!(c.default_repo.is_none());
        assert!(c.authors.is_empty());
        assert!(c.repos.is_empty());
        assert_eq!(c.review.timeout_seconds, 600);
        assert_eq!(c.review.comment_prefix, DEFAULT_COMMENT_PREFIX);
        assert!(c.review.mention_author);
        assert!(c.review.prompt_file.is_none());
    }

    #[test]
    fn auto_actions_are_off_by_default_and_validated() {
        let base = "organization = \"o\"\nproject = \"p\"\n";
        let c = Config::parse(base).unwrap();
        assert!(!c.auto.approve && !c.auto.autocomplete && !c.auto.delete_source_branch);
        assert!(c.auto.merge_strategy.is_none());

        let c = Config::parse(&format!(
            "{base}[auto]\napprove = true\nautocomplete = true\nmerge_strategy = \"squash\"\n"
        ))
        .unwrap();
        assert!(c.auto.approve && c.auto.autocomplete);
        assert_eq!(c.auto.merge_strategy.as_deref(), Some("squash"));

        assert!(Config::parse(&format!("{base}[auto]\nmerge_strategy = \"yolo\"\n")).is_err());
    }

    #[test]
    fn partial_review_section_keeps_other_defaults() {
        let c = Config::parse("organization = \"o\"\nproject = \"p\"\n[review]\ntimeout_seconds = 30\n")
            .unwrap();
        assert_eq!(c.review.timeout_seconds, 30);
        assert_eq!(c.review.comment_prefix, DEFAULT_COMMENT_PREFIX);
    }

    #[test]
    fn rejects_missing_or_empty_required_fields() {
        assert!(Config::parse("project = \"p\"").is_err());
        assert!(Config::parse("organization = \"\"\nproject = \"p\"").is_err());
        assert!(Config::parse("organization = \"https://dev.azure.com/o\"\nproject = \"p\"").is_err());
        assert!(Config::parse("organization = \"o\"\nproject = \"p\"\n[review]\ntimeout_seconds = 0").is_err());
    }

    #[test]
    fn repo_path_expands_tilde_and_ignores_case() {
        let c = Config::parse(FULL).unwrap();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(c.repo_path("MY-REPO").unwrap(), home.join("code/my-repo"));
        assert_eq!(c.repo_path("other").unwrap(), PathBuf::from("/abs/other"));
    }

    #[test]
    fn unmapped_repo_has_no_path() {
        let c = Config::parse(FULL).unwrap();
        assert!(c.repo_path("nope").is_none());
    }
}
