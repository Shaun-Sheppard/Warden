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
// Work item comments are only published as a preview resource.
const COMMENTS_API_VERSION: &str = "7.1-preview.4";
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
    #[serde(default)]
    properties: Value,
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

/// A work item on a board, with its text fields as plain text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardItem {
    pub id: u64,
    pub rev: i64,
    pub project: String,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub area_path: String,
    pub priority: Option<i64>,
    pub severity: String,
    pub tags: Vec<String>,
    pub description: String,
    pub repro_steps: String,
    pub acceptance_criteria: String,
    pub system_info: String,
    /// Who raised the item: their id (for a mention) and name.
    pub created_by_id: String,
    pub created_by_name: String,
    /// Who it is assigned to (their id), if anyone.
    pub assigned_to_id: String,
    /// Its child work items (tasks under a bug, say).
    pub children: Vec<u64>,
    /// The item it sits under (the PBI a task belongs to, say).
    pub parent: Option<u64>,
}

impl BoardItem {
    fn from_fields(id: u64, fields: &serde_json::Map<String, Value>) -> Self {
        let text = |key: &str| fields.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let tags = text("System.Tags");
        BoardItem {
            id,
            rev: fields.get("System.Rev").and_then(Value::as_i64).unwrap_or(0),
            project: text("System.TeamProject"),
            kind: text("System.WorkItemType"),
            title: text("System.Title"),
            state: text("System.State"),
            area_path: text("System.AreaPath"),
            priority: fields.get("Microsoft.VSTS.Common.Priority").and_then(Value::as_i64),
            severity: text("Microsoft.VSTS.Common.Severity"),
            tags: tags.split(';').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect(),
            description: html_to_text(&text("System.Description")),
            repro_steps: html_to_text(&text("Microsoft.VSTS.TCM.ReproSteps")),
            acceptance_criteria: html_to_text(&text("Microsoft.VSTS.Common.AcceptanceCriteria")),
            system_info: html_to_text(&text("Microsoft.VSTS.TCM.SystemInfo")),
            created_by_id: fields.get("System.CreatedBy").and_then(|c| c["id"].as_str()).unwrap_or("").to_string(),
            created_by_name: fields.get("System.CreatedBy").and_then(|c| c["displayName"].as_str()).unwrap_or("").to_string(),
            assigned_to_id: fields.get("System.AssignedTo").and_then(|c| c["id"].as_str()).unwrap_or("").to_string(),
            children: Vec::new(),
            parent: fields.get("System.Parent").and_then(Value::as_u64),
        }
    }

    /// Reads the child links from a work item's relations.
    fn with_relations(mut self, relations: &[Value]) -> Self {
        let linked = |rel: &str| -> Vec<u64> {
            relations
                .iter()
                .filter(|r| r["rel"].as_str() == Some(rel))
                .filter_map(|r| r["url"].as_str()?.rsplit('/').next()?.parse().ok())
                .collect()
        };
        self.children = linked("System.LinkTypes.Hierarchy-Forward");
        if self.parent.is_none() {
            self.parent = linked("System.LinkTypes.Hierarchy-Reverse").first().copied();
        }
        self
    }
}

/// A work item type and the states it can be in.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemTypeInfo {
    pub name: String,
    pub states: Vec<String>,
}

/// A git repository in a project.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub default_branch: Option<String>,
}

/// WIQL for the items assigned to the token's user in the current project,
/// in any of the given states and types.
pub fn my_items_query(states: &[String], types: &[String], iteration: Option<&str>) -> String {
    let list = |values: &[String]| {
        values.iter().map(|v| format!("'{}'", v.replace('\'', "''"))).collect::<Vec<_>>().join(", ")
    };
    let mut query = String::from(
        "SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] = @project AND [System.AssignedTo] = @Me",
    );
    if !states.is_empty() {
        query.push_str(&format!(" AND [System.State] IN ({})", list(states)));
    }
    if !types.is_empty() {
        query.push_str(&format!(" AND [System.WorkItemType] IN ({})", list(types)));
    }
    if let Some(path) = iteration.filter(|p| !p.trim().is_empty()) {
        query.push_str(&format!(" AND [System.IterationPath] UNDER '{}'", path.replace('\'', "''")));
    }
    query.push_str(" ORDER BY [System.Id]");
    query
}

/// Escapes text for an HTML work item comment.
pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Markup that mentions a user in a work item comment, notifying them.
pub fn work_item_mention(user_id: &str, name: &str) -> String {
    format!(
        "<a href=\"#\" data-vss-mention=\"version:2.0,{}\">@{}</a>",
        html_escape(user_id),
        html_escape(name)
    )
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

/// One open comment thread on a pull request.
#[derive(Debug, Clone, PartialEq)]
pub struct PrThread {
    pub id: u64,
    /// The file and line it is on, for inline comments.
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Author and text of each comment, oldest first.
    pub comments: Vec<(String, String)>,
}

/// The threads still open (active or pending) with at least one comment from a person.
pub fn open_threads_from(threads: &[Value]) -> Vec<PrThread> {
    threads
        .iter()
        .filter(|t| !t["isDeleted"].as_bool().unwrap_or(false))
        .filter(|t| matches!(t["status"].as_str(), Some("active") | Some("pending") | None))
        .filter_map(|t| {
            let comments: Vec<(String, String)> = t["comments"]
                .as_array()?
                .iter()
                .filter(|c| !c["isDeleted"].as_bool().unwrap_or(false))
                .filter(|c| c["commentType"].as_str().map_or(true, |k| k == "text"))
                .filter_map(|c| {
                    let text = c["content"].as_str()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let author = c["author"]["displayName"].as_str().unwrap_or("Someone").to_string();
                    Some((author, text.to_string()))
                })
                .collect();
            if comments.is_empty() {
                return None;
            }
            let ctx = &t["threadContext"];
            Some(PrThread {
                id: t["id"].as_u64().unwrap_or(0),
                file: ctx["filePath"].as_str().map(|f| f.trim_start_matches('/').to_string()),
                line: ctx["rightFileStart"]["line"].as_u64().map(|l| l as u32),
                comments,
            })
        })
        .collect()
}

/// A comment on a work item.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemComment {
    pub author: String,
    pub date: String,
    pub text: String,
}

/// Comments from a work item comments response (newest first), oldest first.
pub fn comments_from(resp: &Value) -> Vec<ItemComment> {
    let mut comments: Vec<ItemComment> = resp["comments"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|c| !c["isDeleted"].as_bool().unwrap_or(false))
                .filter_map(|c| {
                    let text = html_to_text(c["text"].as_str()?);
                    (!text.trim().is_empty()).then(|| ItemComment {
                        author: c["createdBy"]["displayName"].as_str().unwrap_or("Someone").to_string(),
                        date: c["createdDate"].as_str().unwrap_or("").chars().take(10).collect(),
                        text,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    comments.reverse();
    comments
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
            // The sign-in address, when Azure DevOps includes it.
            unique_name: user.properties["Account"]["$value"].as_str().unwrap_or_default().to_string(),
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

    /// Ids of the items assigned to the token's user in this project, in the given states and types.
    /// The project's current sprint (of its default team), as an iteration path,
    /// if one is set for today.
    pub async fn current_iteration_path(&self) -> Result<Option<String>> {
        let url = format!("{}/{}/{}/_apis/work/teamsettings/iterations", self.base_url, seg(&self.organization), seg(&self.project));
        let resp: ListResponse<Value> =
            self.send(self.http.get(url).query(&[("$timeframe", "current"), ("api-version", API_VERSION)])).await?;
        Ok(resp.value.first().and_then(|i| i["path"].as_str()).map(str::to_string))
    }

    pub async fn my_work_item_ids(&self, states: &[String], types: &[String], iteration: Option<&str>) -> Result<Vec<u64>> {
        #[derive(Deserialize)]
        struct Ref {
            id: u64,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wiql {
            #[serde(default)]
            work_items: Vec<Ref>,
        }
        let url = format!("{}/{}/{}/_apis/wit/wiql", self.base_url, seg(&self.organization), seg(&self.project));
        let result: Wiql = self
            .send(
                self.http
                    .post(url)
                    .query(&[("api-version", API_VERSION)])
                    .json(&json!({ "query": my_items_query(states, types, iteration) })),
            )
            .await?;
        Ok(result.work_items.into_iter().map(|r| r.id).collect())
    }

    /// Full details of work items, fetched in batches.
    pub async fn board_items(&self, ids: &[u64]) -> Result<Vec<BoardItem>> {
        #[derive(Deserialize)]
        struct Item {
            id: u64,
            #[serde(default)]
            fields: serde_json::Map<String, Value>,
            #[serde(default)]
            relations: Vec<Value>,
        }
        let url = format!("{}/{}/_apis/wit/workitems", self.base_url, seg(&self.organization));
        let mut items = Vec::new();
        for batch in ids.chunks(200) {
            let ids_text = batch.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            // errorPolicy=omit: an item deleted since it was listed comes back as null
            // rather than failing the whole batch.
            let resp: ListResponse<Option<Item>> = self
                .send(self.http.get(&url).query(&[
                    ("ids", ids_text.as_str()),
                    ("$expand", "relations"),
                    ("errorPolicy", "omit"),
                    ("api-version", API_VERSION),
                ]))
                .await?;
            items.extend(resp.value.iter().flatten().map(|item| BoardItem::from_fields(item.id, &item.fields).with_relations(&item.relations)));
        }
        Ok(items)
    }

    /// Every area path in this project, e.g. `Project\\Admin CMS\\Reports`.
    pub async fn area_paths(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Node {
            name: String,
            #[serde(default)]
            children: Vec<Node>,
        }
        fn walk(node: &Node, prefix: &str, out: &mut Vec<String>) {
            let path = if prefix.is_empty() { node.name.clone() } else { format!("{prefix}\\{}", node.name) };
            out.push(path.clone());
            for child in &node.children {
                walk(child, &path, out);
            }
        }
        let url = format!(
            "{}/{}/{}/_apis/wit/classificationnodes/Areas",
            self.base_url,
            seg(&self.organization),
            seg(&self.project)
        );
        let root: Node = self.send(self.http.get(url).query(&[("$depth", "20"), ("api-version", API_VERSION)])).await?;
        let mut paths = Vec::new();
        walk(&root, "", &mut paths);
        Ok(paths)
    }

    /// The git repositories in this project.
    pub async fn repositories(&self) -> Result<Vec<RepoInfo>> {
        let resp: ListResponse<RepoInfo> =
            self.send(self.http.get(self.git_api("repositories")).query(&[("api-version", API_VERSION)])).await?;
        let mut repos = resp.value;
        repos.sort_by_key(|r| r.name.to_lowercase());
        Ok(repos)
    }

    /// The work item types people use in this project, each with its states. Hidden system types
    /// (test plans, code review requests and the like) and disabled types are left out.
    pub async fn work_item_types(&self) -> Result<Vec<WorkItemTypeInfo>> {
        let base = format!("{}/{}/{}/_apis/wit", self.base_url, seg(&self.organization), seg(&self.project));
        let types: ListResponse<Value> =
            self.send(self.http.get(format!("{base}/workitemtypes")).query(&[("api-version", API_VERSION)])).await?;
        let categories: ListResponse<Value> = self
            .send(self.http.get(format!("{base}/workitemtypecategories")).query(&[("api-version", API_VERSION)]))
            .await?;
        let hidden: Vec<String> = categories
            .value
            .iter()
            .filter(|c| c["referenceName"].as_str() == Some("Microsoft.HiddenCategory"))
            .flat_map(|c| c["workItemTypes"].as_array().cloned().unwrap_or_default())
            .filter_map(|t| t["name"].as_str().map(str::to_lowercase))
            .collect();
        let mut found: Vec<WorkItemTypeInfo> = types
            .value
            .iter()
            .filter(|t| !t["isDisabled"].as_bool().unwrap_or(false))
            .filter_map(|t| {
                let name = t["name"].as_str()?.to_string();
                let states = t["states"]
                    .as_array()
                    .map(|s| s.iter().filter_map(|s| s["name"].as_str().map(String::from)).collect())
                    .unwrap_or_default();
                Some(WorkItemTypeInfo { name, states })
            })
            .filter(|t| !hidden.contains(&t.name.to_lowercase()))
            .collect();
        found.sort_by_key(|t| t.name.to_lowercase());
        Ok(found)
    }

    /// Opens a pull request from `source` into `target`, linked to the given work items.
    /// Returns its id and web address.
    pub async fn create_pull_request(
        &self,
        repo: &RepoInfo,
        source: &str,
        target: &str,
        title: &str,
        description: &str,
        work_items: &[u64],
    ) -> Result<(u64, String)> {
        let url = self.git_api(&format!("repositories/{}/pullrequests", seg(&repo.id)));
        // Azure DevOps rejects descriptions over 4000 characters.
        let description: String = description.chars().take(3900).collect();
        let body = json!({
            "sourceRefName": format!("refs/heads/{source}"),
            "targetRefName": format!("refs/heads/{target}"),
            "title": title,
            "description": description,
            "workItemRefs": work_items.iter().map(|id| json!({ "id": id.to_string() })).collect::<Vec<_>>(),
        });
        let created: Value = self.send(self.http.post(url).query(&[("api-version", API_VERSION)]).json(&body)).await?;
        let id = created["pullRequestId"].as_u64().context("Azure DevOps did not return the new pull request's id")?;
        let web = format!(
            "{}/{}/{}/_git/{}/pullrequest/{id}",
            self.base_url,
            seg(&self.organization),
            seg(&self.project),
            seg(&repo.name)
        );
        Ok((id, web))
    }

    /// The states a work item type can be in, with each one's category
    /// (`Proposed`, `InProgress`, `Resolved`, `Completed`, `Removed`).
    pub async fn work_item_states(&self, kind: &str) -> Result<Vec<(String, String)>> {
        let url = format!(
            "{}/{}/{}/_apis/wit/workitemtypes/{}/states",
            self.base_url,
            seg(&self.organization),
            seg(&self.project),
            seg(kind)
        );
        let resp: ListResponse<Value> = self.send(self.http.get(url).query(&[("api-version", API_VERSION)])).await?;
        Ok(resp
            .value
            .iter()
            .filter_map(|s| Some((s["name"].as_str()?.to_string(), s["category"].as_str().unwrap_or("").to_string())))
            .collect())
    }

    /// Moves a work item to another state.
    pub async fn set_work_item_state(&self, id: u64, state: &str) -> Result<()> {
        let url = format!("{}/{}/{}/_apis/wit/workitems/{id}", self.base_url, seg(&self.organization), seg(&self.project));
        let patch = json!([{ "op": "add", "path": "/fields/System.State", "value": state }]);
        let _: Value = self
            .send(
                self.http
                    .patch(url)
                    .query(&[("api-version", API_VERSION)])
                    .header("Content-Type", "application/json-patch+json")
                    .body(patch.to_string()),
            )
            .await?;
        Ok(())
    }

    /// The latest comments on a work item (at most `top`), oldest first, as plain text.
    pub async fn work_item_comments(&self, id: u64, top: usize) -> Result<Vec<ItemComment>> {
        let url = format!(
            "{}/{}/{}/_apis/wit/workItems/{id}/comments",
            self.base_url,
            seg(&self.organization),
            seg(&self.project)
        );
        let top = top.to_string();
        let resp: Value = self
            .send(self.http.get(url).query(&[("api-version", COMMENTS_API_VERSION), ("$top", top.as_str()), ("order", "desc")]))
            .await?;
        Ok(comments_from(&resp))
    }

    /// Adds a comment (HTML) to a work item.
    pub async fn add_work_item_comment(&self, id: u64, html: &str) -> Result<()> {
        let url = format!(
            "{}/{}/{}/_apis/wit/workItems/{id}/comments",
            self.base_url,
            seg(&self.organization),
            seg(&self.project)
        );
        let _: Value = self
            .send(self.http.post(url).query(&[("api-version", COMMENTS_API_VERSION)]).json(&json!({ "text": html })))
            .await?;
        Ok(())
    }

    /// Replaces a work item's tags.
    pub async fn set_work_item_tags(&self, id: u64, tags: &[String]) -> Result<()> {
        let url = format!("{}/{}/{}/_apis/wit/workitems/{id}", self.base_url, seg(&self.organization), seg(&self.project));
        let patch = json!([{ "op": "add", "path": "/fields/System.Tags", "value": tags.join("; ") }]);
        let _: Value = self
            .send(
                self.http
                    .patch(url)
                    .query(&[("api-version", API_VERSION)])
                    .header("Content-Type", "application/json-patch+json")
                    .body(patch.to_string()),
            )
            .await?;
        Ok(())
    }

    pub fn work_item_web_url(&self, id: u64) -> String {
        format!("{}/{}/{}/_workitems/edit/{id}", self.base_url, seg(&self.organization), seg(&self.project))
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
    /// The open comment threads on a pull request: active or pending, with
    /// their people's comments (system messages such as "voted" are left out).
    pub async fn open_threads(&self, repo_id: &str, pr_id: u64) -> Result<Vec<PrThread>> {
        let resp: ListResponse<Value> =
            self.send(self.http.get(self.threads_url(repo_id, pr_id)).query(&[("api-version", API_VERSION)])).await?;
        Ok(open_threads_from(&resp.value))
    }

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

    #[test]
    fn work_item_comments_come_oldest_first_as_text() {
        let resp = json!({ "comments": [
            { "text": "<div>Still broken in <b>Safari</b></div>", "createdBy": { "displayName": "QA Person" }, "createdDate": "2026-10-14T09:00:00Z" },
            { "text": "", "createdBy": { "displayName": "Empty" } },
            { "text": "gone", "isDeleted": true },
            { "text": "<p>Fixed in PR 5803</p>", "createdBy": { "displayName": "Shaun" }, "createdDate": "2026-10-13T09:00:00Z" }
        ]});
        let comments = comments_from(&resp);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0], ItemComment { author: "Shaun".into(), date: "2026-10-13".into(), text: "Fixed in PR 5803".into() });
        assert_eq!(comments[1].text, "Still broken in Safari");
    }

    #[test]
    fn only_open_threads_with_comments_from_people_are_kept() {
        let threads = json!([
            { "id": 1, "status": "active", "comments": [{ "content": "Rename this.", "commentType": "text", "author": { "displayName": "Ann" } }],
              "threadContext": { "filePath": "/src/a.cs", "rightFileStart": { "line": 12, "offset": 1 } } },
            { "id": 2, "status": "fixed", "comments": [{ "content": "Done already", "commentType": "text", "author": { "displayName": "Ann" } }] },
            { "id": 3, "comments": [{ "content": "Ann voted -10", "commentType": "system", "author": { "displayName": "Ann" } }] },
            { "id": 4, "status": "active", "isDeleted": true, "comments": [{ "content": "gone", "commentType": "text" }] },
            { "id": 5, "status": "pending", "comments": [{ "content": "Why?", "commentType": "text", "author": { "displayName": "Bob" } },
                                                       { "content": "", "commentType": "text" }] }
        ]);
        let open = open_threads_from(threads.as_array().unwrap());
        assert_eq!(open.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 5]);
        assert_eq!(open[0].file.as_deref(), Some("src/a.cs"));
        assert_eq!(open[0].line, Some(12));
        assert_eq!(open[0].comments, vec![("Ann".to_string(), "Rename this.".to_string())]);
        assert_eq!(open[1].comments.len(), 1);
    }

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
    fn my_items_query_filters_by_assignee_state_and_type() {
        let q = my_items_query(&["To Do".into(), "New".into()], &["Bug".into()], None);
        assert!(q.contains("[System.AssignedTo] = @Me"));
        assert!(q.contains("[System.TeamProject] = @project"));
        assert!(q.contains("[System.State] IN ('To Do', 'New')"));
        assert!(q.contains("[System.WorkItemType] IN ('Bug')"));
        // Quotes in names cannot break out of the query.
        assert!(my_items_query(&["It's".into()], &[], None).contains("IN ('It''s')"));
        assert!(!my_items_query(&[], &[], None).contains("IN ("));
        assert!(!my_items_query(&[], &[], None).contains("IterationPath"));
        let sprint = my_items_query(&[], &[], Some("Clinic\\Sprint 119 (13th - 26th Oct)"));
        assert!(sprint.contains("AND [System.IterationPath] UNDER 'Clinic\\Sprint 119 (13th - 26th Oct)' ORDER BY"));
        assert!(my_items_query(&[], &[], Some("P\\Bob's sprint")).contains("UNDER 'P\\Bob''s sprint'"));
    }

    #[test]
    fn mentions_and_comment_text_are_escaped() {
        assert_eq!(
            work_item_mention("abc-123", "Shaun Sheppard"),
            r##"<a href="#" data-vss-mention="version:2.0,abc-123">@Shaun Sheppard</a>"##
        );
        assert_eq!(html_escape("a < b & \"c\""), "a &lt; b &amp; &quot;c&quot;");
    }

    #[test]
    fn board_items_are_read_from_fields() {
        let fields = json!({
            "System.Rev": 7, "System.TeamProject": "Clinic", "System.WorkItemType": "Bug",
            "System.Title": "Crash on save", "System.State": "To Do", "System.AreaPath": "Clinic\\Admin CMS",
            "Microsoft.VSTS.Common.Priority": 2, "System.Tags": "ui; regression ",
            "Microsoft.VSTS.TCM.ReproSteps": "<ol><li>Open form</li><li>Save</li></ol>",
            "System.CreatedBy": { "displayName": "Alex Rivera", "id": "u-9", "uniqueName": "andy@example.com" }
        });
        let item = BoardItem::from_fields(42, fields.as_object().unwrap());
        assert_eq!((item.id, item.rev, item.priority), (42, 7, Some(2)));
        assert_eq!(item.area_path, "Clinic\\Admin CMS");
        assert_eq!(item.tags, ["ui", "regression"]);
        assert_eq!((item.created_by_id.as_str(), item.created_by_name.as_str()), ("u-9", "Alex Rivera"));
        let item = item.with_relations(&[
            json!({ "rel": "System.LinkTypes.Hierarchy-Forward", "url": "https://dev.azure.com/o/_apis/wit/workItems/36258" }),
            json!({ "rel": "System.LinkTypes.Hierarchy-Reverse", "url": "https://dev.azure.com/o/_apis/wit/workItems/1" }),
            json!({ "rel": "ArtifactLink", "url": "vstfs:///Git/PullRequestId/x" }),
        ]);
        assert_eq!(item.children, [36258]);
        assert_eq!(item.parent, Some(1));
        assert_eq!(item.repro_steps, "- Open form\n- Save");
        assert_eq!(item.description, "");
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
