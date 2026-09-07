use crate::{edited, removed, renamed_old};

pub fn artifact_probe_route(value: u64) -> u64 {
    edited::artifact_probe_edited(value)
        + removed::artifact_probe_deleted(value)
        + renamed_old::artifact_probe_renamed(value)
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_source_contract() {
        assert_eq!(super::artifact_probe_route(0), 6);
    }
}
