use super::*;

#[test]
fn batched_mutations_preserve_prior_read_evidence_without_becoming_selections() {
    let mut baseline = typed_graph_consumption_trace();
    set_tool_name(&mut baseline, "patch-route", "read");
    set_started_arguments(&mut baseline, "patch-route", "repo/src/route.rs", false);
    let expected = analyze_trace(&baseline, &graph_consumption_options())
        .metrics
        .graph;
    for (name, arguments) in [
        (
            "edit_files",
            r#"{"files":[{"path":"repo/src/route.rs","edits":[{"oldText":"old","newText":"new"}]}]}"#,
        ),
        (
            "format_rust",
            r#"{"paths":["repo/src/route.rs"],"edition":"2021"}"#,
        ),
    ] {
        for captured in [true, false] {
            let mut trace = baseline.clone();
            let mut pair = trace
                .events
                .iter()
                .filter(|event| match &event.event {
                    AgentActivityEventV1::ToolStarted(tool) => tool.call_id == "patch-route",
                    AgentActivityEventV1::ToolFinished(tool) => tool.call_id == "patch-route",
                    _ => false,
                })
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(pair.len(), 2);
            let next = pair[1].seq + 1;
            for event in &mut trace.events {
                if event.seq >= next {
                    event.seq += 2;
                }
            }
            for (offset, event) in pair.iter_mut().enumerate() {
                event.seq = next + offset as u64;
                event.turn = Some(99);
                match &mut event.event {
                    AgentActivityEventV1::ToolStarted(tool) => {
                        tool.call_id = "batch-mutation".into();
                        tool.name = name.into();
                        tool.arguments = captured.then(|| {
                            CapturedContentV1::Inline(temper_protocol_activity::InlineContentV1 {
                                text: arguments.into(),
                                truncated: false,
                            })
                        });
                    }
                    AgentActivityEventV1::ToolFinished(tool) => {
                        tool.call_id = "batch-mutation".into();
                        tool.name = name.into();
                    }
                    _ => unreachable!(),
                }
            }
            trace.events.extend(pair);
            trace.events.sort_by_key(|event| event.seq);
            let summary = analyze_trace(&trace, &graph_consumption_options());
            assert_eq!(
                serde_json::to_value(&summary.metrics.graph).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            assert_eq!(summary.metrics.structure.unwrap().mutations, Some(1));
        }
    }
}
