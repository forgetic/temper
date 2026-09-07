use crate::Policy;
use crate::labels::normalize_label;

pub fn parse_policy(input: &str) -> Policy {
    Policy {
        required: input
            .split(',')
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(normalize_label)
            .collect(),
    }
}
