//! Logical-round selection for repeated durable issue fan-out.

use super::{child_correlation_key, decode_intent_body, metadata_error};
use crate::metadata::{
    CreateIssuesCompletion, CreateIssuesIntent, WorkflowMetadata, global_child_correlation_key,
    parse_metadata_block, replace_metadata_block,
};
use temper_forge::Issue;

use super::super::ExecutionError;

pub(super) enum IntentRound {
    Existing {
        key: String,
        intent: CreateIssuesIntent,
    },
    Insert {
        key: String,
        intent: CreateIssuesIntent,
    },
}

pub(super) fn select_intent_round(
    metadata: &WorkflowMetadata,
    parent: &Issue,
    base_key: &str,
    proposed: &CreateIssuesIntent,
) -> Result<IntentRound, ExecutionError> {
    let mut next_round = 0;
    let mut latest = None;
    loop {
        let key = intent_round_key(base_key, next_round);
        let Some(intent) = metadata.create_issue_intents.get(&key) else {
            break;
        };
        latest = Some((key, intent.clone()));
        next_round += 1;
    }

    let Some((key, mut existing)) = latest else {
        return Ok(IntentRound::Insert {
            key: base_key.to_string(),
            intent: proposed.clone(),
        });
    };
    if existing.completion.is_none() && !existing.completed {
        existing.completion = proposed.completion.clone();
    }
    if !existing.completed {
        if !same_intent_request(&existing, proposed) {
            return Err(ExecutionError::Backend {
                message: format!(
                    "incomplete create-issues intent `{key}` does not match the current child payload"
                ),
            });
        }
        return Ok(IntentRound::Existing {
            key,
            intent: existing,
        });
    }
    if same_intent_request(&existing, proposed)
        && source_reflects_completion(parent, existing.completion.as_ref())?
    {
        return Ok(IntentRound::Existing {
            key,
            intent: existing,
        });
    }

    Ok(IntentRound::Insert {
        key: intent_round_key(base_key, next_round),
        intent: qualify_intent_round(parent, proposed.clone(), next_round),
    })
}

fn intent_round_key(base_key: &str, round: usize) -> String {
    if round == 0 {
        base_key.to_string()
    } else {
        format!("{base_key}/round:{round}")
    }
}

fn qualify_intent_round(
    parent: &Issue,
    mut intent: CreateIssuesIntent,
    round: usize,
) -> CreateIssuesIntent {
    if round == 0 {
        return intent;
    }
    let base = format!(
        "{}:{}/round:{round}",
        intent.correlation_key.len(),
        intent.correlation_key
    );
    for child in &mut intent.children {
        child.correlation_key = if child.repository_id == parent.repo_id {
            child_correlation_key(&base, &child.slug)
        } else {
            let slug = format!("round:{round}/{}:{}", child.slug.len(), child.slug);
            global_child_correlation_key(&parent.repo_id, parent.number, &slug)
        };
    }
    intent
}

fn same_intent_request(left: &CreateIssuesIntent, right: &CreateIssuesIntent) -> bool {
    left.transition == right.transition
        && left.effect_index == right.effect_index
        && left.correlation_key == right.correlation_key
        && left.record_parent_dependencies == right.record_parent_dependencies
        && left.completion == right.completion
        && left.children.len() == right.children.len()
        && left
            .children
            .iter()
            .zip(&right.children)
            .all(|(left, right)| {
                left.slug == right.slug
                    && left.title == right.title
                    && left.body_hex == right.body_hex
                    && left.final_labels == right.final_labels
                    && left.dependencies == right.dependencies
                    && left.repository_id == right.repository_id
            })
}

fn source_reflects_completion(
    parent: &Issue,
    completion: Option<&CreateIssuesCompletion>,
) -> Result<bool, ExecutionError> {
    let Some(completion) = completion else {
        return Ok(false);
    };
    let labels_match = completion
        .add_labels
        .iter()
        .all(|label| parent.labels.contains(label))
        && completion
            .remove_labels
            .iter()
            .all(|label| !parent.labels.contains(label));
    let assignees_match = completion
        .add_assignees
        .iter()
        .all(|assignee| parent.assignees.contains(assignee))
        && completion
            .remove_assignees
            .iter()
            .all(|assignee| !parent.assignees.contains(assignee));
    if !labels_match || !assignees_match {
        return Ok(false);
    }
    let Some(encoded_body) = completion.body_hex.as_deref() else {
        return Ok(true);
    };
    let expected = decode_intent_body(encoded_body)?;
    let metadata = parse_metadata_block(&parent.body)
        .map_err(metadata_error)?
        .unwrap_or_default();
    let expected = replace_metadata_block(&expected, &metadata).map_err(metadata_error)?;
    Ok(expected == parent.body)
}
