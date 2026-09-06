//! Live coverage and attributed child handoff, after normal graph convergence.
use super::mapped_focused_test_relevance_fake as graph;
use super::{FakeMcpServer, McpToolCallEvidence, ModelObservations, messages_contain};
use jig_core::{Reply, Script};
use jig_server::FakeLlm;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

pub(super) fn start(
    count: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
) -> Result<FakeLlm, String> {
    FakeLlm::start(Script::rule(move |view| {
        if messages_contain(view,"You are an investigation sub-agent") {
            assert!(messages_contain(view,"fixture-generation-1279"));
            assert!(messages_contain(view,"relationship resolution is best effort"));
            return match view.prior_tool_results {
                0=>graph::tool_reply("child-consumes-source","read",json!({"path":"demo/src/route.rs"})),
                1=>{
                    assert!(messages_contain(view,"affinity_topic"));
                    Reply::text("Attributed Verify finding: src/route.rs selects topic on retries; use affinity_topic for preserved worker selection. Limitations: relationship resolution is best effort; coverage does not prove completeness. Parent must revalidate before editing.")
                },
                _=>panic!("unexpected child evidence turn"),
            };
        }
        assert!(messages_contain(view,"ROLE: engineer"));
        count.fetch_add(1,Ordering::SeqCst);
        graph::record_observations(view,&mut observations.lock().unwrap());
        match view.prior_tool_results {
            0=>graph::tool_reply("discover-scoped-evidence-root","codebase_memory_search_graph",json!({"query":"route affinity","limit":4})),
            1..=3=>graph::reply_at(view,view.prior_tool_results),
            4=>graph::tool_reply("focused-relevance-exact-test-source","codebase_memory_get_code_snippet",json!({"qualified_name":graph::focused_test_target(view),"decision_evidence_kind":"focused_test"})),
            5=> {
                assert!(messages_contain(view,temper_agent_core::DECISION_ANCHOR_CONVERGENCE_MESSAGE));
                graph::tool_reply("coverage-after-convergence","codebase_memory_check_index_coverage",json!({"paths":["src/route.rs","src/lib.rs","tests/alias_retry.rs"],"scopes":["src","tests"],"scope_limit":4}))
            },
            6=>{
                let report=graph::provider_results(view).into_iter().find(|v|v.get("evidence_id").is_some()).expect("coverage receipt");
                assert_eq!(report["status"],"clean");assert_eq!(report["pagination_complete"],true);
                graph::tool_reply("handoff-graph-evidence","investigate",json!({"task":"Verify the bounded retry-affinity repair from supplied graph evidence and direct source.","graph_context":{
                    "evidence_id":report["evidence_id"],"task_scope":"retry worker affinity in src/route.rs","tier":"Verify","claim":"positive",
                    "sources":[{"path":"src/route.rs","qualified_symbol":graph::implementation_target(view),"origin":"graph_snippet"},{"path":"src/lib.rs","qualified_symbol":graph::caller_target(view),"origin":"graph_snippet"},{"path":"tests/alias_retry.rs","qualified_symbol":graph::focused_test_target(view),"origin":"graph_snippet"}],
                    "relationships":[{"from":graph::caller_target(view),"to":graph::implementation_target(view),"kind":"calls"}],"query_bounds":["targeted semantic search limit 4; selected implementation inbound trace"],"pagination":"complete","limitations":["relationship resolution is best effort"]
                }}))
            },
            7=> {
                assert!(messages_contain(view,"\"status\":\"revalidated\""));
                assert!(messages_contain(view,"Attributed Verify finding"));
                graph::reply_at(view,10)
            },
            turn=>graph::reply_at(view,turn+3),
        }
    })).map_err(|e|format!("start scoped evidence Jig: {e}"))
}

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let base = [
        "index_status",
        "index_repository",
        "index_status",
        "search_graph",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
        "get_code_snippet",
    ];
    if calls
        .iter()
        .take(8)
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        != base
        || calls
            .iter()
            .any(|c| c.is_error || c.arguments.get("decision_evidence_kind").is_some())
    {
        return Err("scoped evidence requires the ordered current-root implementation/caller/focused-test sources".into());
    }
    let tokens = super::mapped_focused_test_relevance::relevance_tokens(mcp)?;
    for (index, key, name) in [
        (4, "qualified_name", "implementation"),
        (5, "function_name", "implementation"),
        (6, "qualified_name", "caller"),
        (7, "qualified_name", "focused_test"),
    ] {
        if calls[index].arguments[key]
            != super::mapped_focused_test_relevance::token(&tokens, name)?
        {
            return Err("scoped evidence did not consume the returned source selectors".into());
        }
    }
    if calls[3].arguments["query"] != "route affinity"
        || calls[3].arguments["limit"] != 4
        || calls[5].arguments["direction"] != "inbound"
    {
        return Err("scoped evidence omitted bounded discovery and inbound caller trace".into());
    }
    let requested = calls[1].arguments["name"]
        .as_str()
        .ok_or("missing stable project")?;
    let actual = super::stable_rebind::confirmed_project_from_calls(&calls[..8], requested)?;
    if calls[3..].iter().any(|c| c.arguments["project"] != actual) {
        return Err("scoped source evidence lost current project binding".into());
    }
    super::stable_rebind::validate_stable_rebind_contract(mcp, &calls[..8], requested)?;
    let diagnostic = calls.iter().skip(8).collect::<Vec<_>>();
    let expected = [
        "index_status",
        "check_index_coverage",
        "check_index_coverage",
        "index_status",
    ]
    .repeat(3);
    if diagnostic
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        != expected
        || diagnostic.iter().any(|c| c.is_error)
    {
        return Err(
            "coverage must run after source convergence and revalidate before and after the child"
                .into(),
        );
    }
    let project = &calls[2].arguments["project"];
    if diagnostic
        .iter()
        .any(|c| c.arguments["project"] != *project)
    {
        return Err("coverage lost current project identity".into());
    }
    for call in diagnostic
        .iter()
        .filter(|c| c.name == "check_index_coverage")
    {
        if call.arguments["paths"] != json!(["src/route.rs", "src/lib.rs", "tests/alias_retry.rs"])
            || call.arguments["scopes"] != json!(["src", "tests"])
            || call.arguments["scope_limit"] != 4
        {
            return Err("coverage did not retain cited paths and explicit bounded scope".into());
        }
    }
    Ok(())
}
