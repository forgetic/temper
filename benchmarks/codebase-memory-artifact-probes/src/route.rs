use crate::{added, edited, renamed_new};

pub fn artifact_probe_route(value: u64) -> u64 {
    edited::artifact_probe_edited(value)
        + added::artifact_probe_added(value)
        + renamed_new::artifact_probe_renamed(value)
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_source_contract() {
        assert_eq!(super::artifact_probe_route(0), 1104);
    }
}
