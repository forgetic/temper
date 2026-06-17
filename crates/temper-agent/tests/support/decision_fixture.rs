use serde_json::json;

pub(crate) fn role_prompt(workflow_id: &str, prompt_guidance: &str) -> String {
    format!(
        "## Role and workflow\nWorkflow: {workflow_id}\nRole: banana\n\n## Decision output\nReturn exactly one JSON object and no surrounding prose.\nSchema: {{\"action\":\"advance\",\"reason\":\"short rationale\"}}\n\n## User guidance\n{prompt_guidance}\n"
    )
}

pub(crate) fn role_context() -> String {
    let context = json!({
        "work_item": {
            "repository": "forgejo:acme/service",
            "queue": "todo",
            "role": "banana",
            "kind": "task",
            "artifact": {
                "type": "issue",
                "number": 1,
                "title": "Advance a generic task",
                "body": "This synthetic task is ready for the generic action.",
                "labels": ["task", "todo"],
                "state": "Open"
            }
        },
        "allowed_actions": ["advance"],
        "authorized_actions": [{
            "action": "advance",
            "transition": "advance",
            "artifact": "task",
            "requires_gates": []
        }]
    });
    serde_json::to_string_pretty(&context).expect("context serializes")
}
