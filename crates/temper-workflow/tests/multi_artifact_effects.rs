//! Multi-artifact `create_issues` effect tests (ADR 0022 §C, workspace 4/6).
//!
//! `create_issues` creates one-or-many child issues from the workspace work
//! product: authored titles/bodies/labels with declared parent/dependency
//! relations between them. Like `create_pull_request`, the children are read
//! from a keyed runtime-input seam on [`ExecutionContext`], so these tests bind
//! the children the way the runner binds a workspace work product, then assert
//! the children land independently and idempotently with their relations.

mod support;

use std::collections::BTreeMap;

use support::{TestRoot, block_on, create_issue, issue_labels, new_repo};
use temper_forge_model::{CreateRepository, Forge, Issue, IssueQuery, ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactRef, ArtifactSource, CreateIssuesChild, Effect, ExecutionContext, ExecutionError,
    RawEffect, RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow, WorkflowEffect,
    WorkflowMetadata, global_child_correlation_key, parse_metadata_block,
};

const CREATE_ISSUES_WORKFLOW: &str = r#"{
    "name": "create-issues",
    "roles": [{"id": "architect"}],
    "labels": [{"id": "intake"}, {"id": "planned"}, {"id": "code"}, {"id": "ready"}],
    "artifact_kinds": [
        {"id": "epic", "target": "issue", "identifying_labels": ["intake"]}
    ],
    "transitions": [{"id": "break_into_children", "artifact": "epic", "roles": ["architect"], "effects": [
        {"kind": "create_issues", "correlation_key": "plan-epic-1"},
        {"kind": "add_label", "label": "planned"}
    ]}]
}"#;

const DYNAMIC_CREATE_ISSUES_WORKFLOW: &str = r#"{
    "name": "dynamic-create-issues",
    "roles": [{"id": "architect"}],
    "labels": [{"id": "intake"}, {"id": "code"}],
    "artifact_kinds": [
        {"id": "epic", "target": "issue", "identifying_labels": ["intake"]}
    ],
    "transitions": [{"id": "break_into_children", "artifact": "epic", "roles": ["architect"], "effects": [
        {"kind": "create_issues"}
    ]}]
}"#;

fn validate(spec_json: &str) -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(spec_json).expect("json parses");
    spec.validate().expect("workflow validates")
}

fn transition() -> TransitionId {
    TransitionId::new("break_into_children")
}

fn child_correlation_key(base_correlation_key: &str, slug: &str) -> String {
    format!(
        "{}:{}/child:{}:{}",
        base_correlation_key.len(),
        base_correlation_key,
        slug.len(),
        slug
    )
}

fn architect() -> RoleId {
    RoleId::new("architect")
}

/// The two-child plan a workspace would author: an API child the web child
/// depends on. Labels mark them ready code work.
fn planned_children() -> Vec<CreateIssuesChild> {
    vec![
        CreateIssuesChild::new(
            "api-schema",
            "Define the API schema",
            "Author the OpenAPI doc.",
        )
        .with_labels(["code", "ready"]),
        CreateIssuesChild::new(
            "web-client",
            "Build the web client",
            "Consume the API schema.",
        )
        .with_labels(["code", "ready"])
        .with_dependencies(["api-schema"]),
    ]
}

fn cross_repo_planned_children(target_repo: RepositoryId) -> Vec<CreateIssuesChild> {
    vec![
        CreateIssuesChild::new(
            "api-schema",
            "Define the API schema",
            "Author the OpenAPI doc.",
        )
        .with_labels(["code", "ready"]),
        CreateIssuesChild::new(
            "web-client",
            "Build the web client",
            "Consume the API schema.",
        )
        .with_labels(["code", "ready"])
        .with_dependencies(["api-schema"])
        .with_target_repo(target_repo),
    ]
}

fn create_repo(forge: &impl Forge, name: &str) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository is created")
    .id
}

fn issue_count(forge: &impl Forge, repo: &RepositoryId) -> usize {
    block_on(forge.list_issues(repo, IssueQuery::default()))
        .expect("issues list")
        .len()
}

fn issue_by_title(forge: &impl Forge, repo: &RepositoryId, title: &str) -> Issue {
    block_on(forge.list_issues(repo, IssueQuery::default()))
        .expect("issues list")
        .into_iter()
        .find(|issue| issue.title == title)
        .unwrap_or_else(|| panic!("issue titled `{title}` exists"))
}

fn issue_metadata(forge: &impl Forge, repo: &RepositoryId, number: ItemNumber) -> WorkflowMetadata {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    metadata(&issue)
}

/// Returns the children that carry a parent back-reference to `parent`, keyed by
/// title for stable assertions.
fn children_by_title(
    forge: &impl Forge,
    repo: &RepositoryId,
    parent: ItemNumber,
) -> BTreeMap<String, Issue> {
    let parent_ref = ArtifactRef::same_repo(parent);
    block_on(forge.list_issues(repo, IssueQuery::default()))
        .expect("issues list")
        .into_iter()
        .filter(|issue| {
            parse_metadata_block(&issue.body)
                .ok()
                .flatten()
                .is_some_and(|metadata| metadata.parents.contains(&parent_ref))
        })
        .map(|issue| (issue.title.clone(), issue))
        .collect()
}

fn metadata(issue: &Issue) -> WorkflowMetadata {
    parse_metadata_block(&issue.body)
        .expect("metadata parses")
        .expect("metadata exists")
}

#[test]
fn create_issues_deserializes_with_and_without_a_correlation_key() {
    let with_key: RawEffect =
        serde_json::from_str(r#"{"kind": "create_issues", "correlation_key": "k1"}"#).unwrap();
    assert!(matches!(
        with_key,
        RawEffect::CreateIssues { correlation_key: Some(ref k) } if k == "k1"
    ));
    let defaulted: RawEffect = serde_json::from_str(r#"{"kind": "create_issues"}"#).unwrap();
    assert!(matches!(
        defaulted,
        RawEffect::CreateIssues {
            correlation_key: None
        }
    ));
}

#[test]
fn create_issues_validates_and_compiles_into_a_tool_manifest() {
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let compiled = workflow.compile();
    let role = compiled.role(&architect()).expect("role compiles");
    let tool = role
        .tools
        .iter()
        .find(|tool| tool.transition == transition())
        .expect("break_into_children tool compiles");
    assert!(tool.effects.iter().any(|effect| matches!(
        effect,
        Effect::CreateIssues { correlation_key: Some(key) } if key == "plan-epic-1"
    )));
}

#[test]
fn create_issues_creates_dependent_children_with_authored_content() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let context =
        ExecutionContext::new().with_create_issues_at(transition(), 0, planned_children());
    let executor = workflow.executor_with_context(&forge, context);

    let report = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect("create_issues executes");
    assert!(report.applied.contains(&WorkflowEffect::CreateIssues {
        correlation_key: Some("plan-epic-1".into()),
    }));

    let children = children_by_title(&forge, &repo, epic);
    assert_eq!(children.len(), 2, "both children land under the parent");

    let api = children
        .get("Define the API schema")
        .expect("api child exists");
    let web = children
        .get("Build the web client")
        .expect("web child exists");

    // Authored content lands: labels are present on the created children.
    let mut api_labels = api.labels.clone();
    api_labels.sort();
    assert_eq!(api_labels, vec!["code".to_string(), "ready".to_string()]);

    // The web child declares a dependency on the api child's number.
    let web_meta = metadata(web);
    assert!(
        web_meta
            .dependencies
            .contains(&ArtifactRef::same_repo(api.number)),
        "web child depends on the api child, got {:?}",
        web_meta.dependencies
    );
    // The api child has no declared dependency.
    assert!(metadata(api).dependencies.is_empty());

    // The co-declared label effect applied on the parent through the same
    // transition: the parent becomes the plan/aggregation record.
    assert_eq!(
        issue_labels(&forge, &repo, epic),
        vec!["intake".to_string(), "planned".to_string()]
    );
}

#[test]
fn create_issues_is_idempotent_across_retries() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(DYNAMIC_CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let context = ExecutionContext::new()
        .with_create_issues_at(transition(), 0, planned_children())
        .with_create_issues_correlation_key_at(transition(), 0, "plan-epic-9");
    let executor = workflow.executor_with_context(&forge, context);

    let target = ArtifactSource::Issue { number: epic };

    block_on(executor.execute(&repo, target, &transition(), &architect()))
        .expect("first create_issues executes");
    let first = children_by_title(&forge, &repo, epic);
    assert_eq!(first.len(), 2);

    // A retry reuses the per-child correlation keys and resolves the existing
    // children instead of duplicating them.
    block_on(executor.execute(&repo, target, &transition(), &architect()))
        .expect("retry create_issues executes");
    let second = children_by_title(&forge, &repo, epic);
    assert_eq!(second.len(), 2, "retry creates no duplicate children");

    // Dependency links are not duplicated either.
    let web = second
        .get("Build the web client")
        .expect("web child exists");
    assert_eq!(
        metadata(web).dependencies.len(),
        1,
        "the sibling dependency is recorded exactly once"
    );
}

#[test]
fn create_issues_cross_repo_child_lands_in_target_repo() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let repo_a = create_repo(&forge, "api-service");
    let repo_b = create_repo(&forge, "web-service");
    let epic = create_issue(&forge, &repo_a, &["intake"], "raw human epic");
    let context = ExecutionContext::new().with_create_issues_at(
        transition(),
        0,
        cross_repo_planned_children(repo_b.clone()),
    );
    let executor = workflow.executor_with_context(&forge, context);

    block_on(executor.execute(
        &repo_a,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect("cross-repo create_issues executes");

    let api = issue_by_title(&forge, &repo_a, "Define the API schema");
    let web = issue_by_title(&forge, &repo_b, "Build the web client");

    let api_meta = metadata(&api);
    let expected_api_key = child_correlation_key("plan-epic-1", "api-schema");
    assert_eq!(
        api_meta.correlation_key.as_deref(),
        Some(expected_api_key.as_str())
    );
    assert!(
        api_meta.parents.contains(&ArtifactRef::same_repo(epic)),
        "same-repo child keeps same-repo parent backref, got {:?}",
        api_meta.parents
    );

    let mut web_labels = web.labels.clone();
    web_labels.sort();
    assert_eq!(web_labels, vec!["code".to_string(), "ready".to_string()]);
    let web_meta = metadata(&web);
    let expected_web_key = global_child_correlation_key(&repo_a, epic, "web-client");
    assert_eq!(
        web_meta.correlation_key.as_deref(),
        Some(expected_web_key.as_str())
    );
    assert!(
        web_meta
            .parents
            .contains(&ArtifactRef::in_repo(repo_a.clone(), epic)),
        "cross-repo child records repo-qualified parent, got {:?}",
        web_meta.parents
    );
    assert_eq!(
        web_meta.dependencies,
        vec![ArtifactRef::in_repo(repo_a.clone(), api.number)],
        "web child depends on the api child across repositories"
    );

    let parent_meta = issue_metadata(&forge, &repo_a, epic);
    assert_eq!(
        parent_meta.dependencies,
        vec![
            ArtifactRef::in_repo(repo_a.clone(), api.number),
            ArtifactRef::in_repo(repo_b.clone(), web.number),
        ],
        "parent records repo-qualified dependency refs for every child"
    );
}

#[test]
fn create_issues_cross_repo_replay_is_idempotent() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(DYNAMIC_CREATE_ISSUES_WORKFLOW);
    let repo_a = create_repo(&forge, "api-service");
    let repo_b = create_repo(&forge, "web-service");
    let epic = create_issue(&forge, &repo_a, &["intake"], "raw human epic");
    let context = ExecutionContext::new()
        .with_create_issues_at(transition(), 0, cross_repo_planned_children(repo_b.clone()))
        .with_create_issues_correlation_key_at(transition(), 0, "plan-epic-9");
    let executor = workflow.executor_with_context(&forge, context);
    let target = ArtifactSource::Issue { number: epic };

    block_on(executor.execute(&repo_a, target, &transition(), &architect()))
        .expect("first cross-repo create_issues executes");
    let first_repo_a_count = issue_count(&forge, &repo_a);
    let first_repo_b_count = issue_count(&forge, &repo_b);

    block_on(executor.execute(&repo_a, target, &transition(), &architect()))
        .expect("retry cross-repo create_issues executes");
    assert_eq!(
        issue_count(&forge, &repo_a),
        first_repo_a_count,
        "retry creates no duplicate children in the parent repository"
    );
    assert_eq!(
        issue_count(&forge, &repo_b),
        first_repo_b_count,
        "retry creates no duplicate children in the target repository"
    );

    let api = issue_by_title(&forge, &repo_a, "Define the API schema");
    let web = issue_by_title(&forge, &repo_b, "Build the web client");
    let parent_meta = issue_metadata(&forge, &repo_a, epic);
    assert_eq!(
        parent_meta.dependencies,
        vec![
            ArtifactRef::in_repo(repo_a.clone(), api.number),
            ArtifactRef::in_repo(repo_b.clone(), web.number),
        ],
        "parent dependency refs are not duplicated"
    );
    let web_meta = metadata(&web);
    assert_eq!(
        web_meta.dependencies,
        vec![ArtifactRef::in_repo(repo_a.clone(), api.number)],
        "the sibling dependency is recorded exactly once"
    );
}

#[test]
fn create_issues_same_repo_records_no_parent_dependencies() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let context =
        ExecutionContext::new().with_create_issues_at(transition(), 0, planned_children());
    let executor = workflow.executor_with_context(&forge, context);

    block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect("same-repo create_issues executes");

    let parent_issue = block_on(forge.get_issue_by_number(&repo, epic))
        .expect("lookup succeeds")
        .expect("parent exists");
    let parent_meta = parse_metadata_block(&parent_issue.body)
        .expect("metadata parses")
        .unwrap_or_default();
    assert!(
        parent_meta.dependencies.is_empty(),
        "all-same-repo create_issues must not record parent dependency refs, got {:?}",
        parent_meta.dependencies
    );
}

#[test]
fn create_issues_unknown_target_repo_error_names_the_repo() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let missing_repo = RepositoryId::new("repo-does-not-exist");
    let children = vec![
        CreateIssuesChild::new("elsewhere", "Elsewhere", "body")
            .with_target_repo(missing_repo.clone()),
    ];
    let context = ExecutionContext::new().with_create_issues_at(transition(), 0, children);
    let executor = workflow.executor_with_context(&forge, context);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect_err("unknown target repo is rejected");
    let message = error.to_string();
    assert!(
        message.contains("cannot ensure issue in target repository"),
        "error should identify the target-repository create path, got {message}"
    );
    assert!(
        message.contains(missing_repo.as_str()),
        "error should name the unknown target repository `{missing_repo}`, got {message}"
    );
}

#[test]
fn create_issues_without_bound_children_fails_before_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    // No children bound: the seam is empty.
    let executor = workflow.executor(&forge);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect_err("create_issues with no bound children is rejected");
    assert!(matches!(
        error,
        ExecutionError::UnresolvedCreateIssues {
            effect_index: 0,
            ..
        }
    ));
    // Validation precedes mutation: no children, and the label effect did not apply.
    assert!(children_by_title(&forge, &repo, epic).is_empty());
    assert_eq!(
        issue_labels(&forge, &repo, epic),
        vec!["intake".to_string()]
    );
}

#[test]
fn create_issues_without_a_correlation_key_fails_before_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    // The dynamic workflow declares no static key; bind children but no runtime key.
    let workflow = validate(DYNAMIC_CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let context =
        ExecutionContext::new().with_create_issues_at(transition(), 0, planned_children());
    let executor = workflow.executor_with_context(&forge, context);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect_err("create_issues with no correlation key is rejected");
    assert!(matches!(
        error,
        ExecutionError::MissingCorrelationKey {
            effect: WorkflowEffect::CreateIssues { .. }
        }
    ));
    assert!(children_by_title(&forge, &repo, epic).is_empty());
}

#[test]
fn create_issues_with_an_unknown_dependency_fails_before_mutation() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = validate(CREATE_ISSUES_WORKFLOW);
    let repo = new_repo(&forge);
    let epic = create_issue(&forge, &repo, &["intake"], "raw human epic");
    let children = vec![
        CreateIssuesChild::new("only", "Lonely child", "body")
            .with_dependencies(["missing-sibling"]),
    ];
    let context = ExecutionContext::new().with_create_issues_at(transition(), 0, children);
    let executor = workflow.executor_with_context(&forge, context);

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: epic },
        &transition(),
        &architect(),
    ))
    .expect_err("create_issues with an unknown dependency slug is rejected");
    assert!(matches!(
        error,
        ExecutionError::UnknownCreateIssuesDependency { ref dependency, .. }
            if dependency == "missing-sibling"
    ));
    // No child was created: dependency validation precedes mutation.
    assert!(children_by_title(&forge, &repo, epic).is_empty());
    assert_eq!(
        issue_labels(&forge, &repo, epic),
        vec!["intake".to_string()]
    );
}
