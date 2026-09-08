//! Exact source observations for the mapped decision-gap recovery fixture.
//!
//! Source verification drops provider binding and query annotations. The two
//! root results retain original tool-call order, including parallel completion;
//! read their returned identities and verify source against the seeded bytes.

use serde_json::Value;

const ROUTE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/mapped-live-decision-gap-recovery/repo/src/route.rs"
));
const CALLER: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/mapped-live-decision-gap-recovery/repo/src/lib.rs"
));
const FOCUSED_TEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/mapped-live-decision-gap-recovery/repo/tests/alias_retry.rs"
));

struct ExpectedSource {
    root_index: usize,
    result_index: usize,
    path: &'static str,
    source: &'static str,
}

const SOURCES: [ExpectedSource; 4] = [
    ExpectedSource {
        root_index: 0,
        result_index: 0,
        path: "src/route.rs",
        source: ROUTE,
    },
    ExpectedSource {
        root_index: 1,
        result_index: 0,
        path: "src/route.rs",
        source: ROUTE,
    },
    ExpectedSource {
        root_index: 1,
        result_index: 1,
        path: "src/lib.rs",
        source: CALLER,
    },
    ExpectedSource {
        root_index: 0,
        result_index: 2,
        path: "tests/alias_retry.rs",
        source: FOCUSED_TEST,
    },
];

pub(super) fn selected_value<'a>(
    results: &'a [Value],
    root_index: usize,
    result_index: usize,
    field: &str,
) -> Option<&'a str> {
    results
        .iter()
        .filter(|result| {
            result
                .pointer("/results/0/results/0/qualifiedName")
                .is_some()
        })
        .nth(root_index)?
        .pointer(&format!("/results/0/results/{result_index}/{field}"))?
        .as_str()
        .filter(|value| !value.is_empty())
}

pub(super) fn verified_source_count(results: &[Value]) -> usize {
    let mut observed = [false; SOURCES.len()];
    for (index, result) in results.iter().enumerate() {
        for (kind, expected) in SOURCES.iter().enumerate() {
            let Some(selected) = selected_value(
                &results[..index],
                expected.root_index,
                expected.result_index,
                "qualifiedName",
            ) else {
                continue;
            };
            observed[kind] |= result.get("qualified_name").and_then(Value::as_str)
                == Some(selected)
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

    fn root(name: &str) -> Value {
        let results: Vec<_> = ["implementation", "caller", "behavioral_test"]
            .into_iter()
            .map(|kind| {
                let symbol = format!("{kind}_{name}");
                json!({"name":symbol,"qualifiedName":format!("fixture::{symbol}")})
            })
            .collect();
        json!({"results":[{"results":results}]})
    }

    fn source(roots: &[Value], kind: usize) -> Value {
        let expected = &SOURCES[kind];
        let selected = selected_value(
            roots,
            expected.root_index,
            expected.result_index,
            "qualifiedName",
        )
        .unwrap();
        json!({"qualified_name":selected,"file_path":expected.path,"source":expected.source})
    }

    fn transcript() -> Vec<Value> {
        let mut results = vec![root("a"), root("b")];
        for kind in 0..SOURCES.len() {
            results.push(source(&results, kind));
        }
        results
    }

    #[test]
    fn roots_use_call_order_and_returned_selectors_without_query_annotations() {
        let mut results = transcript();
        assert_eq!(
            selected_value(&results, 0, 0, "qualifiedName"),
            Some("fixture::implementation_a")
        );
        results[1]["results"][0]["root_query"] = json!("routing implementation affinity");
        assert_eq!(selected_value(&results, 1, 1, "name"), Some("caller_b"));
        assert_eq!(selected_value(&results, 2, 0, "qualifiedName"), None);
        assert_eq!(verified_source_count(&results), 4);
    }

    #[test]
    fn a_binding_claim_cannot_hide_foreign_source_path_or_selected_identity() {
        for kind in 0..SOURCES.len() {
            for (field, wrong) in [
                ("source", "// other checkout"),
                ("source", ""),
                ("file_path", "/other/checkout/src/route.rs"),
                ("file_path", "../src/route.rs"),
                ("file_path", "src/unrelated.rs"),
                ("qualified_name", "fixture::unselected"),
            ] {
                let mut results = transcript();
                results[kind + 2][field] = json!(wrong);
                results[kind + 2]["binding"] = json!("current_prepared_checkout");
                assert_eq!(verified_source_count(&results), 3, "{kind}: {field}");
            }
        }
    }

    #[test]
    fn duplicate_sources_and_later_discovery_do_not_supply_missing_evidence() {
        let mut results = transcript();
        results[5] = results[2].clone();
        assert_eq!(verified_source_count(&results), 3);

        let mut results = transcript();
        let early_source = results.remove(2);
        results.insert(0, early_source);
        assert_eq!(verified_source_count(&results), 3);
        assert_eq!(verified_source_count(&transcript()[2..]), 0);
    }
}
