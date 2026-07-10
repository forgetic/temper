// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::breakdown_child_kind::{
    create_feature_issue, plan_centric_workflow, plan_feature_in_flight_job,
};
use crate::support::*;

#[test]
fn needs_plan_without_required_target_branch_is_quarantined_before_mutation() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let issue = create_feature_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(plan_centric_workflow()));
        let job = plan_feature_in_flight_job("acme/service", issue);
        let mut plan = job_child(
            "plan",
            "Plan the feature",
            "A prose-only plan with no target branch metadata.",
            &[],
        );
        plan.kind = Some("plan".to_string());
        let result =
            verdict_result_with_children("worker-a", &job.job_id, "needs_plan", vec![plan]);

        applier.apply(job.clone(), result.clone()).await;
        applier.apply(job, result).await;

        let (body, labels) = issue_body_and_labels(&forge, &repo, issue).await;
        assert_eq!(body, "build the feature");
        assert!(has_label(&labels, "feature"));
        assert!(has_label(&labels, "needs-human"));
        assert!(!has_label(&labels, "planned"));
        assert_eq!(list_issues(&forge, &repo).await.len(), 1);
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("workflow metadata `target_branch`"));
    })
}
