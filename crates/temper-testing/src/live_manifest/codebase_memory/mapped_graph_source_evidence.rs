//! Exact source observations for the mapped graph-consumption fixture.
//!
//! Source verification strips the provider's binding claim. Match the selected
//! identity against an earlier discovery result and the checked-in fixture's
//! path and bytes instead of treating that claim as evidence.

use serde_json::Value;

struct ExpectedSource {
    selector_pointer: &'static str,
    symbol: &'static str,
    path: &'static str,
    source: &'static str,
}

const SOURCES: [ExpectedSource; 3] = [
    ExpectedSource {
        selector_pointer: "/results/0/results/0/qualifiedName",
        symbol: "choose_dispatch",
        path: "src/lib.rs",
        source: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/mapped-live-graph-consumption/repo/src/lib.rs"
        )),
    },
    ExpectedSource {
        selector_pointer: "/callers/0/qualified_name",
        symbol: "dispatch",
        path: "src/caller.rs",
        source: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/mapped-live-graph-consumption/repo/src/caller.rs"
        )),
    },
    ExpectedSource {
        selector_pointer: "/results/0/results/1/qualifiedName",
        symbol: "selected_dispatch_is_preserved_after_retry",
        path: "tests/dispatch_behavior.rs",
        source: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scenarios/mapped-live-graph-consumption/repo/tests/dispatch_behavior.rs"
        )),
    },
];

pub(super) fn verified_source_count(results: &[Value]) -> usize {
    let mut observed = [false; SOURCES.len()];
    for (index, result) in results.iter().enumerate() {
        for (kind, expected) in SOURCES.iter().enumerate() {
            let selected = results[..index]
                .iter()
                .rev()
                .find_map(|prior| prior.pointer(expected.selector_pointer)?.as_str());
            let Some(selected) = selected else {
                continue;
            };
            let valid_symbol = selected
                .rsplit_once("::")
                .is_some_and(|(_, symbol)| symbol == expected.symbol);
            observed[kind] |= valid_symbol
                && result.get("qualified_name").and_then(Value::as_str) == Some(selected)
                && result.get("file_path").and_then(Value::as_str) == Some(expected.path)
                && result.get("source").and_then(Value::as_str) == Some(expected.source);
        }
    }
    observed.into_iter().filter(|observed| *observed).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(kind: usize) -> Value {
        let expected = &SOURCES[kind];
        json!({"qualified_name":format!("fixture::scope::{}", expected.symbol),
            "file_path":expected.path,"source":expected.source})
    }

    fn transcript() -> Vec<Value> {
        vec![
            json!({"results":[{"results":[
                {"qualifiedName":"fixture::scope::choose_dispatch","file_path":"src/lib.rs","is_test":false},
                {"qualifiedName":"fixture::scope::selected_dispatch_is_preserved_after_retry",
                    "file_path":"tests/dispatch_behavior.rs","is_test":true}
            ]}]}),
            source(0),
            json!({"callers":[{"qualified_name":"fixture::scope::dispatch"}]}),
            source(1),
            source(2),
        ]
    }

    #[test]
    fn recognizes_all_three_verified_fixture_sources_without_a_binding_claim() {
        assert_eq!(verified_source_count(&transcript()), 3);
        assert_eq!(verified_source_count(&transcript()[..2]), 1);
    }

    #[test]
    fn binding_claim_cannot_hide_foreign_bytes_paths_or_another_selected_identity() {
        for (field, wrong) in [
            (
                "source",
                "pub fn choose_dispatch() { /* other checkout */ }",
            ),
            ("source", ""),
            ("file_path", "/other/checkout/src/lib.rs"),
            ("file_path", "../src/lib.rs"),
            ("file_path", "src/caller.rs"),
            ("qualified_name", "fixture::other::choose_dispatch"),
        ] {
            let mut results = transcript();
            results[1][field] = json!(wrong);
            results[1]["binding"] = json!("current_prepared_checkout");
            assert_eq!(verified_source_count(&results), 2, "{field}: {wrong}");
        }
    }

    #[test]
    fn all_source_roles_and_prior_selector_provenance_are_required() {
        let mut results = transcript();
        results[3] = source(0);
        results[4] = source(0);
        assert_eq!(verified_source_count(&results), 1);

        let mut results = transcript();
        results.swap(0, 1);
        assert_eq!(verified_source_count(&results), 2);
    }
}
