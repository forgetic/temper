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
