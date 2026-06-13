use serde_json::json;
use temper_process_protocol::{
    WorkflowEffect, WorkflowPromptManifest, WorkflowPromptSection, WorkflowRoleManifest,
    WorkflowRolePromptExtension, WorkflowToolManifest,
};

pub(crate) fn role_manifest(workflow_id: &str, prompt_guidance: &str) -> WorkflowRoleManifest {
    WorkflowRoleManifest {
        id: "banana".to_string(),
        charter: None,
        prompt_extension: WorkflowRolePromptExtension {
            guidance: Some(prompt_guidance.to_string()),
            tool_guidance: None,
        },
        concurrency: None,
        queues: vec!["todo".to_string()],
        authority: Vec::new(),
        tools: vec![WorkflowToolManifest {
            name: "advance".to_string(),
            transition: "advance".to_string(),
            artifact: "task".to_string(),
            requires_gates: Vec::new(),
            effects: vec![
                WorkflowEffect::RemoveLabel("todo".to_string()),
                WorkflowEffect::AddLabel("done".to_string()),
            ],
        }],
        external_tools: Vec::new(),
        prompt: WorkflowPromptManifest {
            role: "banana".to_string(),
            sections: vec![
                WorkflowPromptSection {
                    heading: "Workflow".to_string(),
                    lines: vec![format!("Workflow: {workflow_id}")],
                },
                WorkflowPromptSection {
                    heading: "Role".to_string(),
                    lines: vec!["Role: banana".to_string(), prompt_guidance.to_string()],
                },
            ],
        },
    }
}

pub(crate) fn role_context(role: &WorkflowRoleManifest) -> String {
    let context = json!({
        "work_item": {
            "repository": "forgejo:acme/service",
            "queue": "todo",
            "role": role.id.as_str(),
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
        "allowed_actions": ["no_action", "advance"],
        "authorized_actions": [{
            "action": "advance",
            "transition": "advance",
            "artifact": "task",
            "requires_gates": []
        }]
    });
    serde_json::to_string_pretty(&context).expect("context serializes")
}
