use crate::Decision;

pub fn render_decision(decision: &Decision) -> String {
    if decision.accepted {
        "accepted".to_string()
    } else {
        format!("missing: {}", decision.missing.join(", "))
    }
}
