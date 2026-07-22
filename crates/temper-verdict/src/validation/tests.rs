use super::*;
use crate::{VerdictChildView, VerdictContract, VerdictResultView};

#[derive(Default)]
struct Child {
    slug: String,
    title: String,
    body: String,
    kind: Option<String>,
    depends_on: Vec<String>,
}

impl Child {
    fn valid(slug: &str, kind: &str) -> Self {
        Self {
            slug: slug.to_string(),
            title: format!("Title {slug}"),
            body: format!("Body {slug}"),
            kind: Some(kind.to_string()),
            depends_on: Vec::new(),
        }
    }
}

impl VerdictChildView for Child {
    fn slug(&self) -> &str {
        &self.slug
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn body(&self) -> &str {
        &self.body
    }
    fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }
    fn depends_on(&self) -> &[String] {
        &self.depends_on
    }
}

struct ResultView {
    verdict: Option<String>,
    title: Option<String>,
    body: Option<String>,
    children: Vec<Child>,
}

impl VerdictResultView for ResultView {
    type Child = Child;

    fn verdict(&self) -> Option<&str> {
        self.verdict.as_deref()
    }
    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
    fn children(&self) -> &[Self::Child] {
        &self.children
    }
}

fn contracts(contract: VerdictContract) -> VerdictContracts {
    BTreeMap::from([("needs_plan".to_string(), contract)])
}

#[test]
fn rejects_wrong_count_kind_and_blank_fields() {
    let contract = VerdictContract {
        min_children: 1,
        max_children: Some(1),
        allowed_child_kinds: vec!["plan".to_string()],
        ..VerdictContract::default()
    };
    let missing = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: Vec::new(),
    };
    let error = validate_verdict_result(
        &missing,
        &contracts(contract.clone()),
        &SourceMetadata::new(),
    )
    .expect_err("missing child is rejected");
    assert!(
        error
            .to_string()
            .contains("exactly 1 child product(s), received 0")
    );

    let invalid = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: vec![Child::valid("work", "code")],
    };
    let error = validate_verdict_result(&invalid, &contracts(contract), &SourceMetadata::new())
        .expect_err("wrong kind is rejected");
    assert!(error.to_string().contains("allowed kinds: plan"));
}

#[test]
fn validates_required_child_workflow_metadata() {
    let contract = VerdictContract {
        min_children: 1,
        max_children: Some(1),
        allowed_child_kinds: vec!["plan".to_string()],
        required_child_metadata: vec!["target_branch".to_string()],
        ..VerdictContract::default()
    };
    let mut child = Child::valid("plan", "plan");
    let mut result = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: vec![child],
    };
    let error = validate_verdict_result(
        &result,
        &contracts(contract.clone()),
        &SourceMetadata::new(),
    )
    .expect_err("missing child metadata is rejected")
    .to_string();
    assert!(error.contains("child `plan` requires non-blank workflow metadata `target_branch`"));

    child = result.children.pop().expect("child");
    child.body = "Body\n\n<!-- temper:workflow\n{\"target_branch\":\"  \"}\n-->".to_string();
    result.children.push(child);
    assert!(
        validate_verdict_result(
            &result,
            &contracts(contract.clone()),
            &SourceMetadata::new(),
        )
        .is_err()
    );

    result.children[0].body = "Body\n\n<!-- temper:workflow\n{".to_string();
    let error = validate_verdict_result(
        &result,
        &contracts(contract.clone()),
        &SourceMetadata::new(),
    )
    .expect_err("malformed child metadata is rejected")
    .to_string();
    assert!(error.contains("malformed workflow metadata"));

    result.children[0].body =
        "Body\n\n<!-- temper:workflow\n{\"target_branch\":\"feature/207-webhook\"}\n-->"
            .to_string();
    validate_verdict_result(&result, &contracts(contract), &SourceMetadata::new())
        .expect("non-blank child metadata is valid");
}

#[test]
fn validates_resolved_target_branch_and_engine_stamping_omission() {
    let contract = VerdictContract {
        min_children: 1,
        target_branch: Some(crate::TargetBranchRequirement {
            expected: "agent/pr-for-feature-207".to_string(),
            repository_default: "main".to_string(),
            allow_omission: true,
        }),
        ..VerdictContract::default()
    };
    let mut result = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: vec![Child::valid("plan", "plan")],
    };
    validate_verdict_result(
        &result,
        &contracts(contract.clone()),
        &SourceMetadata::new(),
    )
    .expect("policy-authorized omission is valid for engine stamping");

    result.children[0].body =
        "Body\n\n<!-- temper:workflow\n{\"target_branch\":\"agent/pr-for-feature-207\"}\n-->"
            .to_string();
    validate_verdict_result(
        &result,
        &contracts(contract.clone()),
        &SourceMetadata::new(),
    )
    .expect("the exact explicit branch is valid");

    for (authored, expected_message) in [
        ("   ", "explicitly sets blank"),
        ("main", "repository default branch `main`"),
        ("feature/other", "expected `agent/pr-for-feature-207`"),
    ] {
        result.children[0].body =
            format!("Body\n\n<!-- temper:workflow\n{{\"target_branch\":\"{authored}\"}}\n-->");
        let error = validate_verdict_result(
            &result,
            &contracts(contract.clone()),
            &SourceMetadata::new(),
        )
        .expect_err("explicit divergence is rejected")
        .to_string();
        assert!(error.contains(expected_message), "error: {error}");
    }

    result.children[0].body =
        "Body\n\n<!-- temper:workflow\n{\"target_branch\":null}\n-->".to_string();
    assert!(
        validate_verdict_result(&result, &contracts(contract), &SourceMetadata::new())
            .expect_err("explicit null is rejected")
            .to_string()
            .contains("blank or non-string")
    );

    result.children[0].body =
        "Body\n\n<!-- temper:workflow\n{\"target_branch\":\"main\"}\n-->".to_string();
    validate_verdict_result(
        &result,
        &contracts(VerdictContract {
            min_children: 1,
            target_branch: Some(crate::TargetBranchRequirement {
                expected: "main".to_string(),
                repository_default: "main".to_string(),
                allow_omission: true,
            }),
            ..VerdictContract::default()
        }),
        &SourceMetadata::new(),
    )
    .expect("an explicit repository-default policy remains supported");
}

#[test]
fn resolved_target_branch_can_require_explicit_metadata_and_is_wire_compatible() {
    let contract = VerdictContract {
        min_children: 1,
        target_branch: Some(crate::TargetBranchRequirement {
            expected: "release".to_string(),
            repository_default: "main".to_string(),
            allow_omission: false,
        }),
        ..VerdictContract::default()
    };
    let result = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: vec![Child::valid("plan", "plan")],
    };
    assert!(
        validate_verdict_result(&result, &contracts(contract), &SourceMetadata::new(),)
            .expect_err("omission is not always authorized")
            .to_string()
            .contains("must explicitly set")
    );

    let legacy: VerdictContract = serde_json::from_value(serde_json::json!({
        "min_children": 1,
        "required_child_metadata": ["target_branch"]
    }))
    .expect("older contract parses without resolved branch requirement");
    assert!(legacy.target_branch.is_none());
}

#[test]
fn rejects_duplicate_unknown_self_and_cyclic_dependencies() {
    let contract = VerdictContract {
        min_children: 1,
        allowed_child_kinds: vec!["code".to_string()],
        ..VerdictContract::default()
    };
    let mut first = Child::valid("first", "code");
    first.depends_on = vec!["second".to_string(), "missing".to_string()];
    let mut second = Child::valid("second", "code");
    second.depends_on = vec!["first".to_string(), "second".to_string()];
    let duplicate = Child::valid("first", "code");
    let result = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: vec![first, second, duplicate],
    };
    let error = validate_verdict_result(&result, &contracts(contract), &SourceMetadata::new())
        .expect_err("invalid dependency graph is rejected")
        .to_string();
    assert!(error.contains("duplicated"));
    assert!(error.contains("unknown sibling `missing`"));
    assert!(error.contains("depends on itself"));
    assert!(error.contains("contains a cycle"));
}

#[test]
fn validates_required_pr_text_and_source_metadata() {
    let contract = VerdictContract {
        max_children: Some(0),
        requires_pr_title: true,
        requires_pr_body: true,
        required_source_metadata: vec!["target_branch".to_string()],
        ..VerdictContract::default()
    };
    let result = ResultView {
        verdict: Some("needs_plan".to_string()),
        title: Some("Land feature".to_string()),
        body: Some("Validation evidence".to_string()),
        children: Vec::new(),
    };
    let error = validate_verdict_result(
        &result,
        &contracts(contract.clone()),
        &SourceMetadata::new(),
    )
    .expect_err("missing metadata is rejected");
    assert!(
        error
            .to_string()
            .contains("source metadata `target_branch`")
    );

    let metadata = SourceMetadata::from([("target_branch".to_string(), "feature/x".to_string())]);
    validate_verdict_result(&result, &contracts(contract), &metadata).expect("valid output");
}
