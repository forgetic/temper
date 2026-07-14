//! End-to-end Forgejo request budget for the durable ten-child fan-out.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use temper_forge_forgejo::{
    ForgejoConfig, ForgejoForge, HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse,
};
use temper_forge_model::{ItemNumber, RepositoryId};
use temper_testing::block_on;
use temper_testing::counting_http::CountingHttpClient;
use temper_workflow::{
    ArtifactSource, CreateIssuesChild, ExecutionContext, RawWorkflowSpec, RoleId, TransitionId,
    WorkflowMetadata, parse_metadata_block,
};

const OWNER: &str = "acme";
const REPO: &str = "widgets";
const ISSUE_PREFIX: &str = "/api/v1/repos/acme/widgets/issues";
const LABEL_PATH: &str = "/api/v1/repos/acme/widgets/labels";

const WORKFLOW: &str = r#"{
  "name":"forgejo-fanout-budget",
  "roles":[{"id":"architect"}],
  "labels":[{"id":"intake"},{"id":"planned"},{"id":"code"},{"id":"ready"},{"id":"blocked"}],
  "artifact_kinds":[{"id":"epic","target":"issue","identifying_labels":["intake"]}],
  "state_dimensions":[{"id":"code_lifecycle","exclusive":true,"states":[
    {"id":"ready","label":"ready"},
    {"id":"blocked","label":"blocked"}
  ]}],
  "transitions":[{"id":"break_into_children","artifact":"epic","roles":["architect"],"effects":[
    {"kind":"create_issues","correlation_key":"plan-epic-1"},
    {"kind":"add_label","label":"planned"}
  ]}]
}"#;

const LABELS: [(&str, u64); 5] = [
    ("intake", 1),
    ("planned", 2),
    ("code", 3),
    ("ready", 4),
    ("blocked", 5),
];

#[derive(Clone, Debug)]
struct StoredIssue {
    number: u64,
    title: String,
    body: String,
    state: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    revision: u64,
}

impl StoredIssue {
    fn response(&self) -> HttpResponse {
        let labels = self
            .labels
            .iter()
            .map(|name| {
                let id = label_id(name).expect("stored label is declared");
                serde_json::json!({ "id": id, "name": name })
            })
            .collect::<Vec<_>>();
        let assignees = self
            .assignees
            .iter()
            .map(|login| serde_json::json!({ "login": login }))
            .collect::<Vec<_>>();
        HttpResponse {
            status: 200,
            headers: vec![(
                "ETag".into(),
                format!("issue-{}-v{}", self.number, self.revision),
            )],
            body: serde_json::json!({
                "number": self.number,
                "title": self.title,
                "body": self.body,
                "state": self.state,
                "user": { "login": "architect" },
                "labels": labels,
                "assignees": assignees,
                "created_at": "2024-03-01T00:00:00Z",
                "updated_at": "2024-03-02T00:00:00Z"
            })
            .to_string(),
        }
    }
}

#[derive(Debug)]
struct ForgejoState {
    next_number: u64,
    issues: BTreeMap<u64, StoredIssue>,
}

#[derive(Clone, Debug)]
struct StatefulForgejoClient {
    state: Arc<Mutex<ForgejoState>>,
}

impl StatefulForgejoClient {
    fn with_parent() -> Self {
        let parent = StoredIssue {
            number: 1,
            title: "Ten-child parent".into(),
            body: "Plan this work.".into(),
            state: "open".into(),
            labels: vec!["intake".into()],
            assignees: Vec::new(),
            revision: 1,
        };
        Self {
            state: Arc::new(Mutex::new(ForgejoState {
                next_number: 2,
                issues: BTreeMap::from([(1, parent)]),
            })),
        }
    }

    fn issues(&self) -> Vec<StoredIssue> {
        self.state
            .lock()
            .expect("stateful Forgejo mutex poisoned")
            .issues
            .values()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl HttpClient for StatefulForgejoClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        if request.method == HttpMethod::Get && request.path == LABEL_PATH {
            let labels = LABELS
                .iter()
                .map(|(name, id)| serde_json::json!({ "id": id, "name": name }))
                .collect::<Vec<_>>();
            return Ok(HttpResponse::new(
                200,
                serde_json::to_string(&labels).expect("labels serialize"),
            ));
        }

        if request.path == ISSUE_PREFIX && request.method == HttpMethod::Post {
            let body = request_json(&request)?;
            let mut state = self.state.lock().expect("stateful Forgejo mutex poisoned");
            let number = state.next_number;
            state.next_number += 1;
            let issue = StoredIssue {
                number,
                title: string_field(&body, "title")?,
                body: string_field(&body, "body")?,
                state: "open".into(),
                labels: label_names(body.get("labels"))?,
                assignees: string_array(body.get("assignees"))?,
                revision: 1,
            };
            let response = issue.response();
            state.issues.insert(number, issue);
            return Ok(HttpResponse {
                status: 201,
                ..response
            });
        }

        let Some(suffix) = request.path.strip_prefix(&format!("{ISSUE_PREFIX}/")) else {
            return Err(HttpError::Transport(format!(
                "unsupported Forgejo request: {} {}",
                request.method, request.path
            )));
        };
        let mut segments = suffix.split('/');
        let number = segments
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| HttpError::Transport(format!("invalid issue path: {}", request.path)))?;
        let child_resource = segments.next();
        let resource_id = segments.next();
        if segments.next().is_some() {
            return Err(HttpError::Transport(format!(
                "unsupported Forgejo request: {} {}",
                request.method, request.path
            )));
        }

        let mut state = self.state.lock().expect("stateful Forgejo mutex poisoned");
        let Some(issue) = state.issues.get_mut(&number) else {
            return Ok(HttpResponse::new(404, r#"{"message":"not found"}"#));
        };

        match (request.method, child_resource) {
            (HttpMethod::Get, None) => Ok(issue.response()),
            (HttpMethod::Patch, None) => {
                let body = request_json(&request)?;
                if let Some(title) = body.get("title").and_then(Value::as_str) {
                    issue.title = title.to_string();
                }
                if let Some(value) = body.get("body").and_then(Value::as_str) {
                    issue.body = value.to_string();
                }
                if let Some(value) = body.get("state").and_then(Value::as_str) {
                    issue.state = value.to_string();
                }
                if body.get("assignees").is_some() {
                    issue.assignees = string_array(body.get("assignees"))?;
                }
                issue.revision += 1;
                Ok(issue.response())
            }
            (HttpMethod::Put, Some("labels")) => {
                let body = request_json(&request)?;
                issue.labels = label_names(body.get("labels"))?;
                issue.revision += 1;
                Ok(HttpResponse::new(200, "[]"))
            }
            (HttpMethod::Post, Some("labels")) => {
                let body = request_json(&request)?;
                issue.labels.extend(label_names(body.get("labels"))?);
                issue.labels.sort();
                issue.labels.dedup();
                issue.revision += 1;
                Ok(HttpResponse::new(200, "[]"))
            }
            (HttpMethod::Delete, Some("labels")) => {
                let id = resource_id
                    .and_then(|label| label.parse::<u64>().ok())
                    .ok_or_else(|| {
                        HttpError::Transport(format!("invalid label path: {}", request.path))
                    })?;
                issue.labels.retain(|name| label_id(name) != Some(id));
                issue.revision += 1;
                Ok(HttpResponse::new(204, ""))
            }
            _ => Err(HttpError::Transport(format!(
                "unsupported Forgejo request: {} {}",
                request.method, request.path
            ))),
        }
    }
}

fn request_json(request: &HttpRequest) -> Result<Value, HttpError> {
    serde_json::from_str(
        request
            .body
            .as_deref()
            .ok_or_else(|| HttpError::Transport("request body is missing".into()))?,
    )
    .map_err(|error| HttpError::Transport(format!("invalid request JSON: {error}")))
}

fn string_field(value: &Value, name: &str) -> Result<String, HttpError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| HttpError::Transport(format!("missing string field `{name}`")))
}

fn string_array(value: Option<&Value>) -> Result<Vec<String>, HttpError> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| HttpError::Transport("expected a string array".into()))
        })
        .collect()
}

fn label_names(value: Option<&Value>) -> Result<Vec<String>, HttpError> {
    let mut names = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| {
            let id = value
                .as_u64()
                .ok_or_else(|| HttpError::Transport("expected a label id array".into()))?;
            LABELS
                .iter()
                .find_map(|(name, candidate)| (*candidate == id).then(|| (*name).to_string()))
                .ok_or_else(|| HttpError::Transport(format!("unknown label id {id}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    names.dedup();
    Ok(names)
}

fn label_id(name: &str) -> Option<u64> {
    LABELS
        .iter()
        .find_map(|(candidate, id)| (*candidate == name).then_some(*id))
}

fn ten_child_dag() -> Vec<CreateIssuesChild> {
    (0..10)
        .map(|index| {
            CreateIssuesChild::new(
                format!("child-{index}"),
                format!("Child {index}"),
                format!("Implement child {index}."),
            )
            .with_labels(["code", "ready"])
            .with_dependencies((0..index).map(|dependency| format!("child-{dependency}")))
        })
        .collect()
}

#[test]
fn known_first_ten_child_fanout_stays_within_forgejo_http_budget() {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    let workflow = spec.validate().expect("workflow validates");
    let inner = StatefulForgejoClient::with_parent();
    let client = CountingHttpClient::new(inner.clone());
    let forge = ForgejoForge::with_client(
        ForgejoConfig::new("https://forge.example.com", "test-token"),
        client.clone(),
    );
    let transition = TransitionId::new("break_into_children");
    let context =
        ExecutionContext::new().with_create_issues_at(transition.clone(), 0, ten_child_dag());

    block_on(workflow.executor_with_context(&forge, context).execute(
        &RepositoryId::new(format!("forgejo:{OWNER}/{REPO}")),
        ArtifactSource::Issue {
            number: ItemNumber::new(1),
        },
        &transition,
        &RoleId::new("architect"),
    ))
    .expect("ten-child Forgejo fan-out completes");

    client
        .check_budget(24, 36, 60)
        .unwrap_or_else(|error| panic!("{error}"));
    let requests = client.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == HttpMethod::Get && request.path == LABEL_PATH)
            .count(),
        1,
        "sibling creates must share one label-id lookup\n{}",
        request_paths(&requests)
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.path.ends_with("/dependencies")),
        "metadata-only fan-out must not enrich dependencies\n{}",
        request_paths(&requests)
    );

    let issues = inner.issues();
    assert_eq!(issues.len(), 11);
    for child in issues.iter().filter(|issue| issue.number != 1) {
        let metadata = parse_metadata_block(&child.body)
            .expect("child metadata parses")
            .expect("child metadata exists");
        assert!(!metadata.staged, "child #{} remained staged", child.number);
    }
    let parent = issues
        .iter()
        .find(|issue| issue.number == 1)
        .expect("parent remains");
    let metadata: WorkflowMetadata = parse_metadata_block(&parent.body)
        .expect("parent metadata parses")
        .expect("parent metadata exists");
    assert!(
        metadata
            .create_issue_intents
            .values()
            .all(|intent| intent.completed)
    );
}

fn request_paths(requests: &[HttpRequest]) -> String {
    requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect::<Vec<_>>()
        .join("\n")
}
