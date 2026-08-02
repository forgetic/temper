use crate::Policy;

pub fn parse_policy(input: &str) -> Policy {
    Policy {
        required: input
            .split(',')
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_ascii_lowercase)
            .collect(),
    }
}
