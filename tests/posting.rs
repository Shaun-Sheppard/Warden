//! Safety tests for the posting flow against a mocked Azure DevOps API.

use anyhow::Result;
use std::collections::VecDeque;

use prr::ado::{AdoClient, PrFilter, PullRequest};
use prr::flow::{post_flow, Choice, Outcome, Prompter};
use prr::review::{parse_review, Review};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const THREADS_PATH: &str = "/org/proj/_apis/git/repositories/repo-id/pullRequests/7/threads";

struct Scripted {
    choices: VecDeque<Choice>,
    edits: VecDeque<Option<String>>,
    confirm: bool,
    confirm_asked: Vec<String>,
}

impl Scripted {
    fn new(choices: &[Choice], confirm: bool) -> Self {
        Self {
            choices: choices.iter().copied().collect(),
            edits: VecDeque::new(),
            confirm,
            confirm_asked: Vec::new(),
        }
    }
}

impl Prompter for Scripted {
    fn show(&mut self, _heading: &str, _body: &str) {}
    fn choose(&mut self) -> Result<Choice> {
        Ok(self.choices.pop_front().expect("unexpected extra prompt"))
    }
    fn edit(&mut self, _text: &str) -> Result<Option<String>> {
        Ok(self.edits.pop_front().expect("unexpected edit"))
    }
    fn confirm(&mut self, question: &str) -> Result<bool> {
        self.confirm_asked.push(question.to_string());
        Ok(self.confirm)
    }
    fn info(&mut self, _message: &str) {}
}

fn pr() -> PullRequest {
    serde_json::from_value(json!({
        "pullRequestId": 7,
        "title": "Add cache",
        "sourceRefName": "refs/heads/feature/cache",
        "targetRefName": "refs/heads/main",
        "creationDate": "2026-10-01T10:00:00Z",
        "createdBy": { "id": "author-guid", "displayName": "Ann Author" },
        "repository": { "id": "repo-id", "name": "repo" }
    }))
    .unwrap()
}

fn review() -> Review {
    parse_review(
        r#"{
          "verdict": "changes_requested",
          "summary": "Risky change.",
          "comments": [
            { "file": "src/a.cs", "line": 42, "severity": "critical", "body": "Null deref." },
            { "file": null, "line": null, "severity": "minor", "body": "No tests." }
          ]
        }"#,
    )
    .unwrap()
}

fn client(server: &MockServer) -> AdoClient {
    AdoClient::new("org", "proj", "the-pat")
        .unwrap()
        .with_base_url(&server.uri())
}

/// Mounts the threads endpoint and asserts (on drop) it is hit exactly `calls` times.
async fn mount_threads(server: &MockServer, calls: u64) {
    Mock::given(method("POST"))
        .and(path(THREADS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 99 })))
        .expect(calls)
        .mount(server)
        .await;
}

async fn posted_bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test]
async fn dry_run_makes_no_calls_and_never_prompts() {
    let server = MockServer::start().await;
    mount_threads(&server, 0).await;
    // An empty script panics if any prompt is shown.
    let mut prompter = Scripted::new(&[], true);

    let outcome = post_flow(&client(&server), &pr(), &review(), "🤖", true, true, &mut prompter)
        .await
        .unwrap();

    assert!(matches!(outcome, Outcome::DryRun));
    assert!(prompter.confirm_asked.is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn declining_final_confirmation_posts_nothing() {
    let server = MockServer::start().await;
    mount_threads(&server, 0).await;
    let mut prompter = Scripted::new(&[Choice::Post, Choice::Post, Choice::Post], false);

    let outcome = post_flow(&client(&server), &pr(), &review(), "🤖", true, false, &mut prompter)
        .await
        .unwrap();

    assert!(matches!(outcome, Outcome::Declined));
    assert_eq!(prompter.confirm_asked, ["Post 3 comments to PR #7?"]);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn quitting_posts_nothing_even_after_approving_some() {
    let server = MockServer::start().await;
    mount_threads(&server, 0).await;
    let mut prompter = Scripted::new(&[Choice::Post, Choice::Quit], true);

    let outcome = post_flow(&client(&server), &pr(), &review(), "🤖", true, false, &mut prompter)
        .await
        .unwrap();

    assert!(matches!(outcome, Outcome::Quit));
    assert!(prompter.confirm_asked.is_empty());
}

#[tokio::test]
async fn skipping_everything_posts_nothing_and_skips_confirmation() {
    let server = MockServer::start().await;
    mount_threads(&server, 0).await;
    let mut prompter = Scripted::new(&[Choice::Skip, Choice::Skip, Choice::Skip], true);

    let outcome = post_flow(&client(&server), &pr(), &review(), "🤖", true, false, &mut prompter)
        .await
        .unwrap();

    assert!(matches!(outcome, Outcome::NothingSelected));
    assert!(prompter.confirm_asked.is_empty());
}

#[tokio::test]
async fn confirmed_comments_are_posted_inline_and_general() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(THREADS_PATH))
        .and(query_param("api-version", "7.1"))
        // Basic auth with empty username and the PAT as password.
        .and(header("authorization", "Basic OnRoZS1wYXQ="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 99 })))
        .expect(2)
        .mount(&server)
        .await;
    // Post the inline comment, skip the general one, post the summary.
    let mut prompter = Scripted::new(&[Choice::Post, Choice::Skip, Choice::Post], true);

    let outcome = post_flow(
        &client(&server),
        &pr(),
        &review(),
        "🤖 AI-assisted review:",
        true,
        false,
        &mut prompter,
    )
    .await
    .unwrap();

    let Outcome::Posted(results) = outcome else { panic!("expected Posted") };
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| matches!(r.result, Ok(99))));
    assert_eq!(prompter.confirm_asked, ["Post 2 comments to PR #7?"]);

    let bodies = posted_bodies(&server).await;
    assert_eq!(
        bodies[0],
        json!({
            "comments": [{
                "parentCommentId": 0,
                "content": "🤖 AI-assisted review: @<author-guid> **[critical]** Null deref.",
                "commentType": 1
            }],
            "status": "active",
            "threadContext": {
                "filePath": "/src/a.cs",
                "rightFileStart": { "line": 42, "offset": 1 },
                "rightFileEnd": { "line": 42, "offset": 1 }
            }
        })
    );
    assert!(bodies[1].get("threadContext").is_none());
    assert_eq!(
        bodies[1]["comments"][0]["content"],
        "🤖 AI-assisted review: @<author-guid> **Decision: Reject**\n\nRisky change."
    );
}

/// With `mention_author` off, comments carry no tag.
#[tokio::test]
async fn edited_text_is_what_gets_posted() {
    let server = MockServer::start().await;
    mount_threads(&server, 1).await;
    let mut prompter = Scripted::new(
        &[Choice::Edit, Choice::Post, Choice::Skip, Choice::Skip],
        true,
    );
    prompter.edits.push_back(Some("  Reworded.\n".to_string()));

    post_flow(&client(&server), &pr(), &review(), "", false, false, &mut prompter)
        .await
        .unwrap();

    let bodies = posted_bodies(&server).await;
    assert_eq!(bodies[0]["comments"][0]["content"], "**[critical]** Reworded.");
}

#[tokio::test]
async fn failures_are_reported_per_comment_with_status_and_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(THREADS_PATH))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({ "message": "Invalid file path" })),
        )
        .expect(2)
        .mount(&server)
        .await;
    let mut prompter = Scripted::new(&[Choice::Post, Choice::Post, Choice::Skip], true);

    let outcome = post_flow(&client(&server), &pr(), &review(), "🤖", true, false, &mut prompter)
        .await
        .unwrap();

    let Outcome::Posted(results) = outcome else { panic!("expected Posted") };
    assert_eq!(results.len(), 2);
    for r in results {
        let err = r.result.unwrap_err().to_string();
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("Invalid file path"), "{err}");
    }
}

#[tokio::test]
async fn rejected_pat_gives_actionable_error_without_leaking_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = client(&server)
        .list_prs(&PrFilter::default())
        .await
        .unwrap_err();
    let text = format!("{err:#}");

    assert!(text.contains("prr auth login"), "{text}");
    assert!(!text.contains("the-pat"));
}

#[tokio::test]
async fn list_and_get_parse_api_responses() {
    let server = MockServer::start().await;
    let pr_json = json!({
        "pullRequestId": 7,
        "title": "Add cache",
        "description": "desc",
        "sourceRefName": "refs/heads/feature/cache",
        "targetRefName": "refs/heads/main",
        "creationDate": "2026-10-01T10:00:00Z",
        "createdBy": { "id": "u", "displayName": "Ann" },
        "repository": { "id": "repo-id", "name": "repo" },
        "reviewers": [{ "id": "me", "vote": 10 }]
    });
    Mock::given(method("GET"))
        .and(path("/org/proj/_apis/git/pullrequests"))
        .and(query_param("searchCriteria.status", "active"))
        .and(query_param("searchCriteria.reviewerId", "me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 1, "value": [pr_json] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/proj/_apis/git/pullrequests/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_json.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/_apis/connectionData"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authenticatedUser": { "id": "me", "providerDisplayName": "Me" }
        })))
        .mount(&server)
        .await;

    let c = client(&server);
    let me = c.current_user().await.unwrap();
    assert_eq!((me.id.as_str(), me.display_name.as_str()), ("me", "Me"));

    let filter = PrFilter { reviewer_id: Some(me.id.clone()), ..Default::default() };
    let prs = c.list_prs(&filter).await.unwrap();
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].vote_of("me"), Some(10));

    let pr = c.get_pr(7).await.unwrap();
    assert_eq!(pr.description.as_deref(), Some("desc"));
    assert!(c.pr_web_url(&pr).ends_with("/org/proj/_git/repo/pullrequest/7"));
}

#[tokio::test]
async fn auto_posting_is_one_comment_with_decision_and_grouped_issues() {
    use prr::flow::{post_all, single_comment, Lead};

    let server = MockServer::start().await;
    mount_threads(&server, 1).await;
    let pr = pr();
    let lead = Lead::for_pr("🤖", true, &pr);

    let results = post_all(&client(&server), &pr, vec![single_comment(&review(), &lead)]).await;

    assert!(results[0].result.is_ok());
    let bodies = posted_bodies(&server).await;
    // PR-level, not attached to a file.
    assert!(bodies[0].get("threadContext").is_none());
    assert_eq!(
        bodies[0]["comments"][0]["content"],
        "🤖 @<author-guid> **Decision: Reject**\n\nRisky change.\n\n\
**Critical (1)**\n\n1. `src/a.cs:42` — Null deref.\n\n\
**Minor (1)**\n\n2. No tests."
    );
}

/// Starting auto mode must not comment on PRs that were already open.
#[tokio::test]
async fn auto_first_run_leaves_existing_prs_alone() {
    use prr::auto::{AutoOptions, AutoState, Runner};
    use prr::config::Config;

    let server = MockServer::start().await;
    mount_threads(&server, 0).await;
    let open = |id: u64, author: &str, draft: bool| {
        json!({
            "pullRequestId": id, "title": "t", "isDraft": draft,
            "sourceRefName": "refs/heads/s", "targetRefName": "refs/heads/main",
            "creationDate": "2026-10-01T10:00:00Z",
            "createdBy": { "id": "u", "displayName": author },
            "repository": { "id": "repo-id", "name": "repo" }
        })
    };
    Mock::given(method("GET"))
        .and(path("/org/proj/_apis/git/pullrequests"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [open(7, "Shaun Sheppard", false), open(8, "Someone Else", false), open(9, "Shaun Sheppard", true)]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let state_path = tmp.path().join("auto-state.json");
    let config = Config::parse(
        "organization = \"org\"\nproject = \"proj\"\nauthors = [\"Shaun Sheppard\"]\n",
    )
    .unwrap();
    let opts = AutoOptions {
        interval: std::time::Duration::from_secs(60),
        once: true,
        dry_run: false,
        include_existing: false,
        approve: true,
        autocomplete: true,
    };

    let mut runner = Runner::new(state_path.clone()).unwrap();
    runner.cycle(&config, &client(&server), "the-pat", &opts).await.unwrap();
    // Only the matching, non-draft PR is recorded; nothing was reviewed or posted.
    assert_eq!(runner.state().handled.iter().copied().collect::<Vec<_>>(), [7]);
    assert_eq!(AutoState::load(&state_path).unwrap().unwrap().handled.len(), 1);

    // A restart sees the same PRs as already handled.
    let mut runner = Runner::new(state_path).unwrap();
    runner.cycle(&config, &client(&server), "the-pat", &opts).await.unwrap();
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|r| r.method.as_str() == "GET"));
}

#[tokio::test]
async fn vote_and_auto_complete_requests() {
    use prr::ado::auto_complete_body;
    use wiremock::matchers::body_json;

    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/org/proj/_apis/git/repositories/repo-id/pullRequests/7/reviewers/me-id"))
        .and(query_param("api-version", "7.1"))
        .and(body_json(json!({ "vote": 5 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "vote": 5 })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/org/proj/_apis/git/repositories/repo-id/pullRequests/7"))
        .and(body_json(json!({
            "autoCompleteSetBy": { "id": "me-id" },
            "completionOptions": { "deleteSourceBranch": false, "mergeStrategy": "squash" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "pullRequestId": 7 })))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(&server);
    c.set_vote("repo-id", 7, "me-id", 5).await.unwrap();
    c.set_auto_complete("repo-id", 7, &auto_complete_body("me-id", Some("squash"), false))
        .await
        .unwrap();
}

#[tokio::test]
async fn linked_work_items_are_fetched_with_their_acceptance_criteria() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/org/proj/_apis/git/repositories/repo-id/pullRequests/7/workitems"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{ "id": "5120", "url": "x" }, { "id": "5121", "url": "y" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/_apis/wit/workitems"))
        .and(query_param("ids", "5120,5121"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                { "id": 5120, "fields": {
                    "System.WorkItemType": "User Story",
                    "System.Title": "Refunds are safe to retry",
                    "System.State": "Active",
                    "System.Description": "<div>As a merchant&nbsp;I want retries.</div>",
                    "Microsoft.VSTS.Common.AcceptanceCriteria": "<ul><li>Same key returns the original response</li><li>Keys are isolated per merchant</li></ul>"
                }},
                { "id": 5121, "fields": { "System.WorkItemType": "Task", "System.Title": "Tidy" } }
            ]
        })))
        .mount(&server)
        .await;

    let items = client(&server).linked_work_items("repo-id", 7).await.unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!((items[0].id, items[0].kind.as_str()), (5120, "User Story"));
    assert_eq!(items[0].description, "As a merchant I want retries.");
    assert_eq!(
        items[0].acceptance_criteria,
        "- Same key returns the original response\n- Keys are isolated per merchant"
    );
    assert_eq!(items[1].acceptance_criteria, "");
}

#[tokio::test]
async fn no_linked_work_items_makes_one_call_and_returns_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/org/proj/_apis/git/repositories/repo-id/pullRequests/7/workitems"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [] })))
        .expect(1)
        .mount(&server)
        .await;
    assert!(client(&server).linked_work_items("repo-id", 7).await.unwrap().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn pull_request_properties_round_trip() {
    use wiremock::matchers::body_json;

    let server = MockServer::start().await;
    let props = "/org/proj/_apis/git/repositories/repo-id/pullRequests/7/properties";
    Mock::given(method("GET"))
        .and(path(props))
        .and(query_param("api-version", "7.1-preview.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 2,
            "value": {
                "Warden.Review": { "$type": "System.String", "$value": "{\"state\":\"reviewing\"}" },
                "Other.Number": { "$type": "System.Int32", "$value": 3 }
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(props))
        .and(header("content-type", "application/json-patch+json"))
        .and(body_json(json!([{ "op": "add", "path": "/Warden.Review", "value": "x" }])))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "count": 1, "value": {} })))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(&server);
    let found = c.pr_properties("repo-id", 7).await.unwrap();
    // Only string properties are returned.
    assert_eq!(found.len(), 1);
    assert_eq!(found["Warden.Review"], "{\"state\":\"reviewing\"}");
    c.set_pr_property("repo-id", 7, "Warden.Review", "x").await.unwrap();
}

/// An approving review must not leave an open thread that blocks the merge.
#[tokio::test]
async fn approving_reviews_post_a_closed_thread() {
    use prr::flow::{post_all, single_comment, Lead};

    let server = MockServer::start().await;
    mount_threads(&server, 2).await;
    let pr = pr();
    let lead = Lead::for_pr("", false, &pr);

    let mut approved = single_comment(&review(), &lead);
    approved.closed = true;
    let rejected = single_comment(&review(), &lead);
    post_all(&client(&server), &pr, vec![approved, rejected]).await;

    let bodies = posted_bodies(&server).await;
    assert_eq!(bodies[0]["status"], "closed");
    assert_eq!(bodies[1]["status"], "active");
}
