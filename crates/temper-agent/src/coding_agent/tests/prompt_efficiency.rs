//! Prompt efficiency and repository-index convergence guidance.

use crate::coding_agent::*;

#[test]
fn system_prompt_uses_role_aware_efficiency_guidance() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    for expected in [
        "Scale discovery to the task",
        "When repository-index tools are available and the task requires code discovery",
        "make the first discovery action a targeted graph symbol/code search",
        "Do not run a compound shell inventory before graph-based source selection",
        "combine repository status, formatting/output, file discovery, sorting, and truncation",
        "Use conventional discovery before selection only when repository-index tools are disabled or unavailable",
        "bounded targeted graph attempt cannot provide the required evidence",
        "fallback shell discovery minimal—one simple discovery command plus a necessary directory change",
        "make every command-list segment fully classifiable",
        "only `&&`, `||`, `;`, or newline separators",
        "Do not use pipelines, redirects, expansions or substitutions, assignments, grouping, globbing, comments, or background operators",
        "Repository status, validation, and other operational checks remain available after source selection",
        "run them as separate calls rather than bundling them with discovery",
        "use only needed call/path tracing and exact source reads",
        "avoid empty or broad graph searches and broad architecture calls",
        "Reserve architecture views for genuine topology questions",
        "For non-local topology work, batch genuinely independent targeted discovery calls",
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
        "Derive the initial graph query from the requested behavior and intended repair",
        "task also names an incidental field, accessor, or symbol",
        "begin with a task-semantic graph query",
        "rather than a name pattern or identifier token",
        "narrow only with identifiers returned by that result",
        "behaviorally relevant implementation candidate",
        "consume its exact source first",
        "implementation-purpose result may over-return caller- or test-shaped candidates",
        "direct source reads of those shapes cannot complete later evidence kinds",
        "Only after implementation source evidence",
        "traverse inbound calls from that exact selected implementation in a later turn",
        "shortest provider-derived refinement needed for that staged route",
        "Consume caller source only from an exact identity returned by the selected-implementation traversal",
        "outer wrapper or incidental caller",
        "merely accepts a caller evidence label",
        "Only the selected-implementation traversal's provider-returned caller identities",
        "eligible later-turn caller/model selectors",
        "complete empty inbound trace settles that selected symbol's graph-caller relationship",
        "do not manufacture caller evidence by rereading the traced symbol as its own caller",
        "search the graph for the requested behavioral regression",
        "before broad or identifier-derived focused-test search",
        "consume the exact test returned by that semantic search",
        "independent test root remains discovery evidence but never satisfies the active implementation root",
        "if focused-test evidence is still missing and the recovery menu lists it",
        "provider-returned caller in a later turn with `trace_path` / `function_name`",
        "relationship `calls`, direction",
        "`inbound`, and test inclusion enabled",
        "Read the exact test identity returned by that traversal",
        "`get_code_snippet` / `qualified_name` / `focused_test` in another later turn",
        "selector discovery, not focused-test source evidence",
        "complete traversal returns no eligible test identity",
        "one listed same-root `search_graph` / `graph_query` fallback",
        "behavioral-regression query from task intent",
        "never copy a fixture or test name from task text, source, diagnostics, or an independent root",
        "exact test returned by that fallback in one still-later focused-test source read",
        "stop without a product if both routes return no eligible test",
        "multiple typed routes could fill a decision gap",
        "producer query, returned implementation, and consumer chain are semantically connected",
        "evidence-kind declaration alone cannot make an incidental route preferable",
        "Keep every source selector",
        "provider-derived and on the active root",
        "After a local decision-evidence denial",
        "follow only this compatible menu",
        "never repeat the denied tool/selector/evidence-kind tuple",
        "Do not batch speculative snippet reads with a traversal",
        "never issue the test source read until a later",
        "turn after the traversal has returned its exact provider-typed test identity",
        "Do not mutate until consumed source evidence covers the selected current-root implementation, its caller/model, and focused behavioral tests",
        "smallest semantic diff",
        "closed decision-evidence recovery guidance lists compatible actions",
        "complete next-step menu for the selected active root",
        "use only those tool/selector/evidence combinations",
        "up to the remaining allowance",
        "matching values already visible in that root's typed results",
        "listed task-semantic graph-query fallback must be newly derived from task intent",
        "Do not issue unlisted discovery searches, start or switch to an unrelated root",
        "repeat a locally denied graph call, or mutate",
        "Independent root creation remains available during ordinary discovery",
        "it is not recovery progress once this menu exists",
        "recovery is exhausted with no compatible action",
        "stop without a product",
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
        "compound shell inventory before graph-based source selection",
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
        "complete next-step menu for the selected active root",
        "Independent root creation remains available during ordinary discovery",
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
fn coding_prompt_orders_graph_discovery_before_classifiable_shell_fallback() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    let graph_first = engineer
        .find("make the first discovery action a targeted graph symbol/code search")
        .expect("coding prompt requires graph-first discovery");
    let fallback = engineer
        .find("fallback shell discovery minimal")
        .expect("coding prompt retains bounded conventional fallback");
    assert!(graph_first < fallback);
    assert!(engineer.contains("make every command-list segment fully classifiable"));
    assert!(engineer.contains("after source selection; run them as separate calls"));
}

#[test]
fn coding_prompt_routes_caller_work_from_provider_selected_implementation() {
    let engineer = system_prompt(Capability::CodingWorkspace, &[]);
    let selection = engineer
        .find("Choose the behaviorally relevant implementation candidate")
        .expect("coding prompt selects the implementation root");
    let implementation = engineer
        .find("consume its exact source first")
        .expect("coding prompt consumes selected implementation source");
    let trace = engineer
        .find("traverse inbound calls from that exact selected implementation in a later turn")
        .expect("coding prompt traces inbound from the implementation root");
    let relationships = engineer
        .find("Only the selected-implementation traversal's provider-returned caller identities")
        .expect("coding prompt consumes typed traversal relationships");
    let empty = engineer
        .find(
            "complete empty inbound trace settles that selected symbol's graph-caller relationship",
        )
        .expect("coding prompt treats an empty trace as complete evidence");
    let denial = engineer
        .find("never repeat the denied tool/selector/evidence-kind tuple")
        .expect("coding prompt closes denied selector/evidence pairs");

    assert!(selection < implementation);
    assert!(implementation < trace && trace < relationships && relationships < empty);
    assert!(empty < denial);
    assert!(engineer.contains("do not manufacture caller evidence by rereading the traced symbol"));
    assert!(engineer.contains(
        "independent test root remains discovery evidence but never satisfies the active implementation root"
    ));
    assert!(engineer.contains("Read the exact test identity returned by that traversal"));
    assert!(engineer.contains("another later turn"));
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
