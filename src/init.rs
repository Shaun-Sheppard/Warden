//! Support for `prr init`: detecting Azure DevOps details from a git remote
//! and rendering the config file.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub organization: String,
    pub project: String,
    pub repo: String,
}

fn decode(s: &str) -> String {
    urlencoding::decode(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// Parses an Azure DevOps git remote URL (HTTPS or SSH, including the legacy
/// visualstudio.com hosts). Returns None for anything else.
pub fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim().trim_end_matches('/');

    // SSH: git@ssh.dev.azure.com:v3/org/project/repo
    if let Some((head, tail)) = url.split_once(":v3/").or_else(|| url.split_once("/v3/")) {
        if !(head.contains("ssh.dev.azure.com") || head.contains("vs-ssh.visualstudio.com")) {
            return None;
        }
        return match tail.split('/').collect::<Vec<_>>().as_slice() {
            [org, project, repo] => Some(Remote {
                organization: decode(org),
                project: decode(project),
                repo: decode(repo),
            }),
            _ => None,
        };
    }

    let rest = url.split_once("://").map(|(_, r)| r)?;
    let (authority, path) = rest.split_once('/')?;
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    let organization = if host.eq_ignore_ascii_case("dev.azure.com") {
        if parts.is_empty() {
            return None;
        }
        decode(parts.remove(0))
    } else if let Some(org) = host.strip_suffix(".visualstudio.com") {
        if parts.first() == Some(&"DefaultCollection") {
            parts.remove(0);
        }
        org.to_string()
    } else {
        return None;
    };

    let (project, repo) = match parts.as_slice() {
        [project, "_git", repo] => (project, repo),
        // The project segment is omitted when the repo shares its name.
        ["_git", repo] => (repo, repo),
        _ => return None,
    };
    Some(Remote {
        organization,
        project: decode(project),
        repo: decode(repo),
    })
}

/// Accepts either a bare organization name or a pasted Azure DevOps URL
/// (`https://dev.azure.com/org[/project/...]`, `https://org.visualstudio.com/...`).
/// Returns the organization and, if the URL included one, the project.
pub fn parse_organization(input: &str) -> (String, Option<String>) {
    let input = input.trim().trim_matches('/');
    let rest = input.split_once("://").map(|(_, r)| r).unwrap_or(input);
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let mut parts = path.split('/').filter(|s| !s.is_empty());

    let organization = if host.eq_ignore_ascii_case("dev.azure.com") {
        match parts.next() {
            Some(org) => decode(org),
            None => return (String::new(), None),
        }
    } else if let Some(org) = host.strip_suffix(".visualstudio.com") {
        org.to_string()
    } else {
        // Not a URL we recognise: treat the whole input as the name.
        return (input.to_string(), None);
    };
    let project = parts
        .next()
        .filter(|p| !p.starts_with('_') && *p != "DefaultCollection")
        .map(decode);
    (organization, project)
}

fn quoted(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn key(s: &str) -> String {
    let bare = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        s.to_string()
    } else {
        quoted(s)
    }
}

/// Splits a comma-separated list of names, dropping blanks.
pub fn parse_authors(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

pub fn render_config(
    organization: &str,
    project: &str,
    default_repo: Option<&str>,
    authors: &[String],
    repos: &[(String, String)],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("organization = {}\n", quoted(organization)));
    out.push_str(&format!("project = {}\n", quoted(project)));
    match default_repo {
        Some(repo) => out.push_str(&format!("default_repo = {}\n", quoted(repo))),
        None => out.push_str("# default_repo = \"my-repo\"   # limit `prr list` to one repo by default\n"),
    }

    if authors.is_empty() {
        out.push_str("# authors = [\"Jane Doe\"]       # only show PRs raised by these people\n");
    } else {
        let list: Vec<String> = authors.iter().map(|a| quoted(a)).collect();
        out.push_str(&format!("authors = [{}]\n", list.join(", ")));
    }
    out.push_str(
        "\n# Optional. prr keeps its own clone of each repo under ~/.cache/prr/repos.\n\
# Map a repo here to review in an existing local clone instead.\n[repos]\n",
    );
    if repos.is_empty() {
        out.push_str("# my-repo = \"~/code/my-repo\"\n");
    }
    for (name, path) in repos {
        out.push_str(&format!("{} = {}\n", key(name), quoted(path)));
    }

    out.push_str(
        "\n[review]\n\
# prompt_file = \"~/.config/prr/review-prompt.md\"   # replaces the default review guidance\n\
timeout_seconds = 600\n\
# comment_prefix = \"🤖 AI-assisted review:\"\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn remote(org: &str, project: &str, repo: &str) -> Option<Remote> {
        Some(Remote {
            organization: org.into(),
            project: project.into(),
            repo: repo.into(),
        })
    }

    #[test]
    fn parses_https_remotes() {
        assert_eq!(
            parse_remote("https://my-org@dev.azure.com/my-org/My%20Project/_git/my-repo"),
            remote("my-org", "My Project", "my-repo")
        );
        assert_eq!(
            parse_remote("https://dev.azure.com/my-org/proj/_git/repo/\n"),
            remote("my-org", "proj", "repo")
        );
        assert_eq!(
            parse_remote("https://dev.azure.com/my-org/_git/same"),
            remote("my-org", "same", "same")
        );
    }

    #[test]
    fn parses_ssh_remotes() {
        assert_eq!(
            parse_remote("git@ssh.dev.azure.com:v3/my-org/My%20Project/my-repo"),
            remote("my-org", "My Project", "my-repo")
        );
        assert_eq!(
            parse_remote("ssh://git@ssh.dev.azure.com/v3/my-org/proj/repo"),
            remote("my-org", "proj", "repo")
        );
    }

    #[test]
    fn parses_legacy_visualstudio_remotes() {
        assert_eq!(
            parse_remote("https://my-org.visualstudio.com/DefaultCollection/proj/_git/repo"),
            remote("my-org", "proj", "repo")
        );
        assert_eq!(
            parse_remote("https://my-org.visualstudio.com/proj/_git/repo"),
            remote("my-org", "proj", "repo")
        );
        assert_eq!(
            parse_remote("my-org@vs-ssh.visualstudio.com:v3/my-org/proj/repo"),
            remote("my-org", "proj", "repo")
        );
    }

    #[test]
    fn ignores_other_hosts_and_shapes() {
        assert_eq!(parse_remote("https://github.com/owner/repo.git"), None);
        assert_eq!(parse_remote("git@github.com:owner/repo.git"), None);
        assert_eq!(parse_remote("https://dev.azure.com/my-org"), None);
        assert_eq!(parse_remote("https://example.com/v3/a/b/c"), None);
        assert_eq!(parse_remote(""), None);
    }

    #[test]
    fn organization_accepts_names_and_pasted_urls() {
        let org = |s: &str| parse_organization(s);
        assert_eq!(org(" my-org "), ("my-org".into(), None));
        assert_eq!(org("https://dev.azure.com/my-org"), ("my-org".into(), None));
        assert_eq!(org("https://dev.azure.com/my-org/"), ("my-org".into(), None));
        assert_eq!(org("dev.azure.com/my-org"), ("my-org".into(), None));
        assert_eq!(
            org("https://dev.azure.com/my-org/My%20Project/_git/repo"),
            ("my-org".into(), Some("My Project".into()))
        );
        assert_eq!(org("https://dev.azure.com/my-org/_git/repo"), ("my-org".into(), None));
        assert_eq!(org("https://my-org.visualstudio.com/proj"), ("my-org".into(), Some("proj".into())));
        assert_eq!(org("https://dev.azure.com/"), (String::new(), None));
    }

    #[test]
    fn rendered_config_round_trips() {
        let repos = vec![
            ("my-repo".to_string(), "~/code/my-repo".to_string()),
            ("Odd.Name \"x\"".to_string(), "/abs/with space".to_string()),
        ];
        let authors = parse_authors(" Shaun Sheppard, ,ann@example.com,");
        assert_eq!(authors, ["Shaun Sheppard", "ann@example.com"]);
        let text = render_config("my-org", "My \"Project\"", Some("my-repo"), &authors, &repos);
        let c = Config::parse(&text).unwrap();
        assert_eq!(c.organization, "my-org");
        assert_eq!(c.project, "My \"Project\"");
        assert_eq!(c.default_repo.as_deref(), Some("my-repo"));
        assert_eq!(c.authors, authors);
        assert_eq!(c.repos["my-repo"], "~/code/my-repo");
        assert_eq!(c.repos["Odd.Name \"x\""], "/abs/with space");
        assert_eq!(c.review.timeout_seconds, 600);
    }

    #[test]
    fn rendered_config_without_repos_is_valid() {
        let c = Config::parse(&render_config("o", "p", None, &[], &[])).unwrap();
        assert!(c.default_repo.is_none());
        assert!(c.authors.is_empty());
        assert!(c.repos.is_empty());
    }
}
