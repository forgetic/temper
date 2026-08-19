//! Prompt efficiency and repository-index convergence guidance.

use crate::coding_agent::*;

#[test]
fn system_prompt_uses_role_aware_efficiency_guidance() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    for expected in [
        "Scale discovery to the task",
        "When repository-index tools are available for a concrete or already-localized defect",
        "start with targeted symbol/code search",
        "use only needed call/path tracing and exact source reads",
        "avoid empty or broad graph searches and broad architecture calls",
        "Reserve architecture views for genuine topology questions",
        "For non-local topology work, batch independent status and targeted discovery calls",
        "skip ritual discovery when the task is already localized",
        "work requiring implementation selection, caller/data-flow understanding, or behavioral preservation",
        "use every successful targeted graph result as a decision checkpoint",
        "consume it with the work-item requirements before selecting a dependent refinement, trace, or source read",
        "Select and invoke that dependent operation only in a later model turn",
        "Keep genuinely independent discovery parallel",
        "A `Decision anchor` explicitly marks a bounded successful targeted current-root result",
        "select from that provider result, not unrelated discovery",
        "absent for failures, unavailable tools, and truncated or ambiguous output",
        "do not issue producer and consumer calls in the same turn or batch",
        "Do not mutate until consumed source evidence covers the selected current-root implementation, its caller/model, and focused behavioral tests",
        "smallest semantic diff",
        "smallest semantic submission diff",
        "explanatory comments unless the task or established local style requires them",
        "complete likely source, test, configuration, and documentation set together",
        "Form the implementation contract internally",
        "do not spend a standalone response publishing a plan",
        "prefer one cohesive `apply_patch` call spanning source, tests, and documentation",
        "Use `edit` or `write` for genuinely isolated changes or bounded repair",
        "one to four mutation responses instead of one response per file",
        "Multiple mutation calls in one model response are model-turn batching, not concurrent execution",
        "Read-safe calls may run concurrently",
        "mutation, process, network, and unknown-effect calls remain serialized barriers",
        "Complete the planned source, tests, configuration, and documentation deliverables",
        "formatter and focused authoritative test suite",
        "bounded repair and focused revalidation without broad rediscovery",
        "repeating architecture searches unless an unresolved correctness question requires it",
        "repository status, the tracked diff, and all untracked deliverables together",
        "submit the unchanged validated workspace once",
        "terminal JSON only after acceptance",
        "8–12 total responses, at most four mutation responses, and zero validation invalidations as goals, not correctness limits",
        "Task correctness and required validation take priority",
        "Keep repository-index exploration progress-bounded for this role",
        "do not repeat non-progressing discovery or grow new roots",
        "Once a current-root trace and sufficient implementation/caller/test source evidence complete the decision chain",
        "stop graph calls",
        "obey convergence or exploration-closed messages",
        "use conventional reads for any remaining verification",
        "smallest role-appropriate product",
    ] {
        assert!(
            engineer.contains(expected),
            "engineer efficiency guidance omitted {expected:?}"
        );
    }

    let generic_guidance = [
        "Batch independent read-only calls into a single response",
        "prefer creating a complete new file in one operation over many incremental changes",
        "Verify with one focused command",
        "Avoid re-reading content just produced",
    ];
    let engineer_only_guidance = [
        "Scale discovery to the task",
        "implementation contract internally",
        "one to four mutation responses",
        "model-turn batching",
        "serialized barriers",
        "bounded repair",
        "tracked diff",
        "8–12 total responses",
        "successful targeted graph result",
        "decision checkpoint",
        "current-root implementation, its caller/model",
        "smallest semantic diff",
        "producer and consumer calls",
        "explanatory comments unless the task",
        "Decision anchor",
    ];
    for prompt in [
        system_prompt(Capability::TriageWorkspace, &[]),
        system_prompt(Capability::ReviewWorkspace, &[]),
    ] {
        for expected in generic_guidance {
            assert!(
                prompt.contains(expected),
                "generic efficiency guidance omitted {expected:?}"
            );
        }
        for expected in [
            "Keep repository-index exploration progress-bounded for this role",
            "stop graph calls",
            "smallest role-appropriate product",
        ] {
            assert!(
                prompt.contains(expected),
                "read-only role omitted convergence guidance {expected:?}"
            );
        }
        for forbidden in engineer_only_guidance {
            assert!(
                !prompt.contains(forbidden),
                "engineer-only guidance leaked into another role: {forbidden:?}"
            );
        }
    }
}

#[test]
fn effective_prompts_converge_for_graph_enabled_delivery_and_mechanical_roles() {
    let registry = tongs::tools::ToolRegistry::new();
    for role in [
        "engineer",
        "architect",
        "scenario_author",
        "tester",
        "reviewer",
        "label_sync",
    ] {
        let prompt = system_prompt_with_registry(
            Capability::for_role(role),
            role,
            &[],
            &Default::default(),
            &registry,
        );
        for expected in [
            "exploration progress-bounded for this role",
            "current-root trace",
            "implementation/caller/test source evidence",
            "stop graph calls",
            "smallest role-appropriate product",
        ] {
            assert!(
                prompt.contains(expected),
                "role={role} omitted {expected:?}"
            );
        }
        for fixture_wording in [
            "retry_worker_topic",
            "alias retry worker affinity",
            "five-call",
        ] {
            assert!(!prompt.contains(fixture_wording), "role={role}");
        }
    }
}
