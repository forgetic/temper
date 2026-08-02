use crate::{Decision, Policy};

pub fn evaluate_labels(policy: &Policy, labels: &[&str]) -> Decision {
    let missing = policy
        .required
        .iter()
        .filter(|required| !labels.iter().any(|label| label == required))
        .cloned()
        .collect::<Vec<_>>();
    Decision {
        accepted: missing.is_empty(),
        missing,
    }
}
