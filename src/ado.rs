use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::RequestBuilder;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

pub const API_VERSION: &str = "7.1";
// connectionData is only published as a preview resource.
const CONNECTION_API_VERSION: &str = "7.1-preview";
// Pull request properties are only published as a preview resource.
const PROPERTIES_API_VERSION: &str = "7.1-preview.1";
const DEFAULT_BASE_URL: &str = "https://dev.azure.com";
const ANONYMOUS_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Identity {
    pub id: String,
    pub display_name: String,
    /// Usually the sign-in email address.
    pub unique_name: String,
}

/// Whether `who` is one of `authors` (display name or sign-in name,
/// case-insensitive). An empty list matches everyone.
pub fn author_matches(authors: &[String], who: &Identity) -> bool {
    authors.is_empty()
        || authors.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).any(|a| {
            a.eq_ignore_ascii_case(who.display_name.trim())
                || (!who.unique_name.is_empty() && a.eq_ignore_ascii_case(who.unique_name.trim()))
        })
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Reviewer {
    pub id: String,
    pub vote: i32,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ProjectRef {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub project: ProjectRef,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRef {
    pub commit_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub pull_request_id: u64,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub source_ref_name: String,
    pub target_ref_name: String,
    pub creation_date: DateTime<Utc>,
    #[serde(default)]
    pub created_by: Identity,
    pub repository: Repository,
    #[serde(default)]
    pub reviewers: Vec<Reviewer>,
    #[serde(default)]
    pub last_merge_source_commit: Option<CommitRef>,
    #[serde(default)]
    pub is_draft: bool,
    /// `active`, `completed` or `abandoned`.
    #[serde(default)]
    pub status: String,
}

impl PullRequest {
    pub fn source_branch(&self) -> &str {
        short_branch(&self.source_ref_name)
    }

    pub fn target_branch(&self) -> &str {
        short_branch(&self.target_ref_name)
    }

    /// Markup that tags the PR's author in a comment (Azure DevOps resolves
    /// `@<identity id>` to a mention and notifies them).
    pub fn author_mention(&self) -> Option<String> {
        let id = self.created_by.id.trim();
        (!id.is_empty()).then(|| format!("@<{id}>"))
    }

    pub fn vote_of(&self, user_id: &str) -> Option<i32> {
        self.reviewers
            .iter()
            .find(|r| r.id.eq_ignore_ascii_case(user_id))
            .map(|r| r.vote)
    }
}

pub fn short_branch(ref_name: &str) -> &str {
    ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name)
}

pub fn vote_label(vote: Option<i32>) -> &'static str {
    match vote {
        None => "-",
        Some(10) => "approved",
        Some(5) => "approved w/ suggestions",
        Some(0) => "no vote",
        Some(-5) => "waiting for author",
        Some(-10) => "rejected",
        Some(_) => "?",
    }
}

#[derive(Debug, Clone, Default)]
pub struct PrFilter {
    pub repo: Option<String>,
    pub creator_id: Option<String>,
    pub reviewer_id: Option<String>,
    pub top: Option<u32>,
    /// `active` unless set (e.g. `all`).
    pub status: Option<String>,
}

#[derive(Deserialize)]
struct ListResponse<T> {
    value: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionData {
    authenticated_user: ConnectionUser,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionUser {
    id: String,
    #[serde(default)]
    provider_display_name: Option<String>,
}

/// A work item linked to a pull request, with its text fields as plain text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkItem {
    pub id: u64,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub description: String,
    pub acceptance_criteria: String,
    pub repro_steps: String,
}

/// Azure DevOps stores rich-text fields as HTML; reduce them to readable text.
pub fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = &rest[start..];
            break;
        };
        let tag = rest[start + 1..start + end].trim().to_lowercase();
        let name = tag.trim_start_matches('/').split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("");
        match (name, tag.starts_with('/')) {
            ("li", false) => text.push_str("\n- "),
            ("br", _) | ("p", true) | ("div", true) | ("tr", true) | ("ul", true) | ("ol", true) => text.push('\n'),
            ("h1" | "h2" | "h3" | "h4", true) => text.push('\n'),
            _ => {}
        }
        rest = &rest[start + end + 1..];
    }
    text.push_str(rest);
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines.join("\n")
}

/// JSON body for a new comment thread. `file`/`line` make it an inline
/// comment on the right-hand (source branch) side of the diff.
pub fn thread_body(content: &str, file: Option<&str>, line: Option<u32>) -> Value {
    let mut body = json!({
        "comments": [{ "parentCommentId": 0, "content": content, "commentType": 1 }],
        "status": "active",
    });
    if let Some(file) = file {
        let path = format!("/{}", file.trim_start_matches('/'));
        let mut ctx = json!({ "filePath": path });
        if let Some(line) = line {
            ctx["rightFileStart"] = json!({ "line": line, "offset": 1 });
            ctx["rightFileEnd"] = json!({ "line": line, "offset": 1 });
        }
        body["threadContext"] = ctx;
    }
    body
}

/// JSON body that sets a PR to auto-complete on behalf of `user_id`.
pub fn auto_complete_body(user_id: &str, merge_strategy: Option<&str>, delete_source_branch: bool) -> Value {
    let mut options = json!({ "deleteSourceBranch": delete_source_branch });
    if let Some(strategy) = merge_strategy {
        options["mergeStrategy"] = json!(strategy);
    }
    json!({ "autoCompleteSetBy": { "id": user_id }, "completionOptions": options })
}

fn seg(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

pub struct AdoClient {
    http: reqwest::Client,
    base_url: String,
    organization: String,
    project: String,
    pat: String,
}

impl AdoClient {
    pub fn new(organization: &str, project: &str, pat: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            // An invalid PAT can be answered with a redirect to the sign-in
            // page; don't follow it, report it as an auth failure instead.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("prr/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Could not initialise the HTTP client")?;
        Ok(Self {
            http,
            base_url: DEFAULT_BASE_URL.to_string(),
            organization: organization.to_string(),
            project: project.to_string(),
            pat: pat.to_string(),
        })
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    /// The same connection, pointed at another project in the organization.
    pub fn with_project(&self, project: &str) -> Self {
        Self {
            http: self.http.clone(),
            base_url: self.base_url.clone(),
            organization: self.organization.clone(),
            project: project.to_string(),
            pat: self.pat.clone(),
        }
    }

    pub fn organization(&self) -> &str {
        &self.organization
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    /// Names of every project in the organization the token can see.
    pub async fn list_projects(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Project {
            name: String,
        }
        let url = format!("{}/{}/_apis/projects", self.base_url, seg(&self.organization));
        let resp: ListResponse<Project> = self
            .send(self.http.get(url).query(&[("$top", "500"), ("api-version", API_VERSION)]))
            .await?;
        let mut names: Vec<String> = resp.value.into_iter().map(|p| p.name).collect();
        names.sort_by_key(|n| n.to_lowercase());
        Ok(names)
    }

    fn git_api(&self, path: &str) -> String {
        format!(
            "{}/{}/{}/_apis/git/{}",
            self.base_url,
            seg(&self.organization),
            seg(&self.project),
            path
        )
    }

    pub fn connection_data_url(&self) -> String {
        format!("{}/{}/_apis/connectionData", self.base_url, seg(&self.organization))
    }

    pub fn list_prs_request(&self, filter: &PrFilter) -> (String, Vec<(&'static str, String)>) {
        let url = match &filter.repo {
            Some(repo) => self.git_api(&format!("repositories/{}/pullrequests", seg(repo))),
            None => self.git_api("pullrequests"),
        };
        let status = filter.status.clone().unwrap_or_else(|| "active".to_string());
        let mut query = vec![("searchCriteria.status", status)];
        if let Some(id) = &filter.creator_id {
            query.push(("searchCriteria.creatorId", id.clone()));
        }
        if let Some(id) = &filter.reviewer_id {
            query.push(("searchCriteria.reviewerId", id.clone()));
        }
        query.push(("$top", filter.top.unwrap_or(200).to_string()));
        query.push(("api-version", API_VERSION.to_string()));
        (url, query)
    }

    /// Project-level lookup, so the repository does not need to be known up front.
    pub fn get_pr_url(&self, id: u64) -> String {
        self.git_api(&format!("pullrequests/{id}"))
    }

    pub fn threads_url(&self, repo_id: &str, pr_id: u64) -> String {
        self.git_api(&format!("repositories/{}/pullRequests/{pr_id}/threads", seg(repo_id)))
    }

    /// HTTPS clone URL for a repository in the configured project.
    pub fn clone_url(&self, repo_name: &str) -> String {
        format!(
            "{}/{}/{}/_git/{}",
            self.base_url,
            seg(&self.organization),
            seg(&self.project),
            seg(repo_name)
        )
    }

    pub fn pr_web_url(&self, pr: &PullRequest) -> String {
        format!(
            "{}/{}/{}/_git/{}/pullrequest/{}",
            self.base_url,
            seg(&self.organization),
            seg(&self.project),
            seg(&pr.repository.name),
            pr.pull_request_id
        )
    }

    async fn send<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T> {
        let resp = req
            .basic_auth("", Some(&self.pat))
            .send()
            .await
            .map_err(|e| anyhow!("Could not reach Azure DevOps: {}", e.without_url()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| anyhow!("Could not read the Azure DevOps response: {}", e.without_url()))?;

        // 203/302 are how Azure DevOps answers a bad PAT with its sign-in page.
        if matches!(status.as_u16(), 401 | 203 | 302) {
            bail!(
                "Azure DevOps rejected the Personal Access Token (HTTP {}). It is invalid or expired; run `prr auth login` with a token that has Code (Read & Write) scope.",
                status.as_u16()
            );
        }
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| body.chars().take(300).collect());
            bail!("Azure DevOps API error (HTTP {}): {}", status.as_u16(), message.trim());
        }
        serde_json::from_str(&body).context("Unexpected response format from Azure DevOps")
    }

    pub async fn current_user(&self) -> Result<Identity> {
        let data: ConnectionData = self
            .send(
                self.http
                    .get(self.connection_data_url())
                    .query(&[("api-version", CONNECTION_API_VERSION)]),
            )
            .await?;
        let user = data.authenticated_user;
        if user.id.eq_ignore_ascii_case(ANONYMOUS_ID) {
            bail!("Azure DevOps did not recognise the Personal Access Token. It is invalid or expired; run `prr auth login`.");
        }
        Ok(Identity {
            id: user.id,
            display_name: user.provider_display_name.unwrap_or_default(),
            ..Default::default()
        })
    }

    pub async fn list_prs(&self, filter: &PrFilter) -> Result<Vec<PullRequest>> {
        let (url, query) = self.list_prs_request(filter);
        let resp: ListResponse<PullRequest> = self.send(self.http.get(url).query(&query)).await?;
        Ok(resp.value)
    }

    pub async fn get_pr(&self, id: u64) -> Result<PullRequest> {
        self.send(
            self.http
                .get(self.get_pr_url(id))
                .query(&[("api-version", API_VERSION)]),
        )
        .await
    }

    /// Work items linked to a pull request (at most ten), with their
    /// description and acceptance criteria. Reading the details needs the
    /// token's Work Items (Read) scope.
    pub async fn linked_work_items(&self, repo_id: &str, pr_id: u64) -> Result<Vec<WorkItem>> {
        #[derive(Deserialize)]
        struct Ref {
            id: String,
        }
        #[derive(Deserialize)]
        struct Item {
            id: u64,
            #[serde(default)]
            fields: serde_json::Map<String, Value>,
        }
        let refs: ListResponse<Ref> = self
            .send(
                self.http
                    .get(format!("{}/workitems", self.pr_url(repo_id, pr_id)))
                    .query(&[("api-version", API_VERSION)]),
            )
            .await?;
        let ids: Vec<&str> = refs.value.iter().map(|r| r.id.as_str()).take(10).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let fields = [
            "System.WorkItemType",
            "System.Title",
            "System.State",
            "System.Description",
            "Microsoft.VSTS.Common.AcceptanceCriteria",
            "Microsoft.VSTS.TCM.ReproSteps",
        ];
        let url = format!("{}/{}/_apis/wit/workitems", self.base_url, seg(&self.organization));
        let items: ListResponse<Item> = self
            .send(self.http.get(url).query(&[
                ("ids", ids.join(",")),
                ("fields", fields.join(",")),
                ("api-version", API_VERSION.to_string()),
            ]))
            .await?;
        Ok(items
            .value
            .into_iter()
            .map(|item| {
                let text = |key: &str| item.fields.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                WorkItem {
                    id: item.id,
                    kind: text("System.WorkItemType"),
                    title: text("System.Title"),
                    state: text("System.State"),
                    description: html_to_text(&text("System.Description")),
                    acceptance_criteria: html_to_text(&text("Microsoft.VSTS.Common.AcceptanceCriteria")),
                    repro_steps: html_to_text(&text("Microsoft.VSTS.TCM.ReproSteps")),
                }
            })
            .collect())
    }

    fn properties_url(&self, repo_id: &str, pr_id: u64) -> String {
        format!("{}/properties", self.pr_url(repo_id, pr_id))
    }

    /// Hidden key/value metadata attached to a pull request. Tools use it to
    /// coordinate; it is not shown in the Azure DevOps UI.
    pub async fn pr_properties(&self, repo_id: &str, pr_id: u64) -> Result<std::collections::BTreeMap<String, String>> {
        #[derive(Deserialize)]
        struct Properties {
            #[serde(default)]
            value: serde_json::Map<String, Value>,
        }
        let props: Properties = self
            .send(
                self.http
                    .get(self.properties_url(repo_id, pr_id))
                    .query(&[("api-version", PROPERTIES_API_VERSION)]),
            )
            .await?;
        Ok(props
            .value
            .into_iter()
            .filter_map(|(key, v)| Some((key, v.get("$value")?.as_str()?.to_string())))
            .collect())
    }

    /// Sets (adds or replaces) one hidden property on a pull request.
    pub async fn set_pr_property(&self, repo_id: &str, pr_id: u64, key: &str, value: &str) -> Result<()> {
        let patch = json!([{ "op": "add", "path": format!("/{key}"), "value": value }]);
        let _: Value = self
            .send(
                self.http
                    .patch(self.properties_url(repo_id, pr_id))
                    .query(&[("api-version", PROPERTIES_API_VERSION)])
                    .header("Content-Type", "application/json-patch+json")
                    .body(patch.to_string()),
            )
            .await?;
        Ok(())
    }

    pub fn pr_url(&self, repo_id: &str, pr_id: u64) -> String {
        self.git_api(&format!("repositories/{}/pullRequests/{pr_id}", seg(repo_id)))
    }

    /// Casts `reviewer_id`'s vote (10 approve, 5 approve with suggestions).
    /// Only auto mode with approval enabled calls this.
    pub async fn set_vote(&self, repo_id: &str, pr_id: u64, reviewer_id: &str, vote: i32) -> Result<()> {
        let url = format!("{}/reviewers/{}", self.pr_url(repo_id, pr_id), seg(reviewer_id));
        let _: Value = self
            .send(
                self.http
                    .put(url)
                    .query(&[("api-version", API_VERSION)])
                    .json(&json!({ "vote": vote })),
            )
            .await?;
        Ok(())
    }

    /// Sets the PR to complete automatically once its policies pass.
    /// Only auto mode with auto-complete enabled calls this.
    pub async fn set_auto_complete(&self, repo_id: &str, pr_id: u64, body: &Value) -> Result<()> {
        let _: Value = self
            .send(
                self.http
                    .patch(self.pr_url(repo_id, pr_id))
                    .query(&[("api-version", API_VERSION)])
                    .json(body),
            )
            .await?;
        Ok(())
    }

    /// Creates a comment thread and returns its id.
    pub async fn post_thread(&self, repo_id: &str, pr_id: u64, body: &Value) -> Result<u64> {
        let resp: Value = self
            .send(
                self.http
                    .post(self.threads_url(repo_id, pr_id))
                    .query(&[("api-version", API_VERSION)])
                    .json(body),
            )
            .await?;
        Ok(resp.get("id").and_then(Value::as_u64).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> AdoClient {
        AdoClient::new("my org", "my-project", "secret").unwrap()
    }

    #[test]
    fn list_request_defaults_to_project_wide_active() {
        let (url, query) = client().list_prs_request(&PrFilter::default());
        assert_eq!(
            url,
            "https://dev.azure.com/my%20org/my-project/_apis/git/pullrequests"
        );
        assert_eq!(
            query,
            vec![
                ("searchCriteria.status", "active".to_string()),
                ("$top", "200".to_string()),
                ("api-version", "7.1".to_string()),
            ]
        );
    }

    #[test]
    fn list_request_with_repo_and_identity_filters() {
        let filter = PrFilter {
            repo: Some("my repo".into()),
            creator_id: Some("me-id".into()),
            reviewer_id: Some("rev-id".into()),
            top: Some(1),
            status: None,
        };
        let (url, query) = client().list_prs_request(&filter);
        assert_eq!(
            url,
            "https://dev.azure.com/my%20org/my-project/_apis/git/repositories/my%20repo/pullrequests"
        );
        assert!(query.contains(&("searchCriteria.creatorId", "me-id".to_string())));
        assert!(query.contains(&("searchCriteria.reviewerId", "rev-id".to_string())));
        assert!(query.contains(&("$top", "1".to_string())));
    }

    #[test]
    fn pr_and_thread_urls() {
        let c = client();
        assert_eq!(
            c.get_pr_url(42),
            "https://dev.azure.com/my%20org/my-project/_apis/git/pullrequests/42"
        );
        assert_eq!(
            c.threads_url("repo-guid", 42),
            "https://dev.azure.com/my%20org/my-project/_apis/git/repositories/repo-guid/pullRequests/42/threads"
        );
        assert_eq!(
            c.clone_url("my repo"),
            "https://dev.azure.com/my%20org/my-project/_git/my%20repo"
        );
        assert_eq!(
            c.connection_data_url(),
            "https://dev.azure.com/my%20org/_apis/connectionData"
        );
    }

    #[test]
    fn html_fields_become_plain_text() {
        let html = "<div>As a clinician I want&nbsp;X.</div><ul><li>Shows <b>dose</b> &amp; unit</li><li>Rejects values &lt; 0</li></ul><p>Done<br/>when signed off</p>";
        assert_eq!(
            html_to_text(html),
            "As a clinician I want X.\n- Shows dose & unit\n- Rejects values < 0\nDone\nwhen signed off"
        );
        assert_eq!(html_to_text("plain text"), "plain text");
        assert_eq!(html_to_text(""), "");
        assert_eq!(html_to_text("a < b"), "a < b");
    }

    #[test]
    fn auto_complete_body_shape() {
        assert_eq!(
            auto_complete_body("me-id", None, false),
            json!({
                "autoCompleteSetBy": { "id": "me-id" },
                "completionOptions": { "deleteSourceBranch": false }
            })
        );
        assert_eq!(
            auto_complete_body("me-id", Some("squash"), true)["completionOptions"],
            json!({ "deleteSourceBranch": true, "mergeStrategy": "squash" })
        );
    }

    #[test]
    fn general_thread_body_has_no_context() {
        let body = thread_body("hello", None, Some(3));
        assert_eq!(
            body,
            json!({
                "comments": [{ "parentCommentId": 0, "content": "hello", "commentType": 1 }],
                "status": "active",
            })
        );
    }

    #[test]
    fn inline_thread_body_targets_right_side_line() {
        let body = thread_body("hello", Some("src/a.cs"), Some(42));
        assert_eq!(
            body["threadContext"],
            json!({
                "filePath": "/src/a.cs",
                "rightFileStart": { "line": 42, "offset": 1 },
                "rightFileEnd": { "line": 42, "offset": 1 },
            })
        );
    }

    #[test]
    fn inline_path_gets_exactly_one_leading_slash() {
        assert_eq!(thread_body("x", Some("/src/a.cs"), None)["threadContext"]["filePath"], "/src/a.cs");
        let file_only = thread_body("x", Some("a.cs"), None);
        assert_eq!(file_only["threadContext"], json!({ "filePath": "/a.cs" }));
    }

    #[test]
    fn author_filter_matches_name_or_sign_in_case_insensitively() {
        let who = Identity {
            id: "u".into(),
            display_name: "Shaun Sheppard".into(),
            unique_name: "ss@example.com".into(),
        };
        let list = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(author_matches(&[], &who));
        assert!(author_matches(&list(&["Ann", " shaun sheppard "]), &who));
        assert!(author_matches(&list(&["SS@example.com"]), &who));
        assert!(!author_matches(&list(&["Shaun"]), &who));
        assert!(!author_matches(&list(&[""]), &Identity::default()));
    }

    #[test]
    fn deserialises_pull_request() {
        let pr: PullRequest = serde_json::from_value(json!({
            "pullRequestId": 7,
            "title": "T",
            "sourceRefName": "refs/heads/feature/x",
            "targetRefName": "refs/heads/main",
            "creationDate": "2026-01-15T10:30:00.1234567Z",
            "createdBy": { "id": "u1", "displayName": "Ann" },
            "repository": { "id": "r1", "name": "repo" },
            "reviewers": [{ "id": "ME", "vote": -5 }],
            "lastMergeSourceCommit": { "commitId": "abc" }
        }))
        .unwrap();
        assert_eq!(pr.author_mention().as_deref(), Some("@<u1>"));
        assert_eq!(pr.source_branch(), "feature/x");
        assert_eq!(pr.target_branch(), "main");
        assert!(pr.description.is_none());
        assert_eq!(vote_label(pr.vote_of("me")), "waiting for author");
        assert_eq!(vote_label(pr.vote_of("other")), "-");
    }
}
