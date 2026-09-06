use super::*;

fn input() -> Value {
    json!({"project":"demo","paths":["src/lib.rs"],"scopes":["src"],"scope_limit":1,"scope_offset":0})
}
fn provider() -> Value {
    json!({"project":"demo","signal":"best_effort","indexed_at":"g1","metadata":{"generation":"g1","generation_matches":true,"recording_status":"complete","hash_records_complete":true},"paths":[{"requested_path":"src/lib.rs","path":"src/lib.rs","status":"no_recorded_issue","freshness":"metadata_match","coverage":[]}],"scopes":[{"requested_scope":"src","scope":"src","total":0,"has_more":false,"entries":[],"status":"no_known_gaps"}]})
}

#[test]
fn coverage_preserves_flags_and_rejects_unknown_completeness() {
    let report = result::normalize(provider(), &input(), "g1", None).unwrap();
    assert_eq!(report["status"], "clean");
    assert_eq!(report["supports_scoped_claim"], true);
    for (field, value, status) in [
        ("status", "excluded", "flagged"),
        ("freshness", "content_changed", "stale"),
        ("freshness", "unknown", "unavailable"),
        ("status", "future", "unavailable"),
    ] {
        let mut p = provider();
        p["paths"][0][field] = json!(value);
        let r = result::normalize(p, &input(), "g1", None).unwrap();
        assert_eq!(r["status"], status);
        assert_eq!(r["supports_scoped_claim"], false);
    }
    let mut p = provider();
    p["paths"][0]["coverage"] =
        json!([{"kind":"parse_partial","detail":"lines 5-7","start_line":5,"end_line":7}]);
    let r = result::normalize(p, &input(), "g1", None).unwrap();
    assert_eq!(r["status"], "flagged");
    assert_eq!(r["provider"]["paths"][0]["coverage"][0]["start_line"], 5);
}

#[test]
fn stale_mismatched_and_incomplete_results_never_support_exhaustive_claims() {
    for expected in [Some("g0"), Some("g2")] {
        assert_eq!(
            result::normalize(provider(), &input(), "g1", expected).unwrap()["status"],
            "stale"
        );
    }
    let mut p = provider();
    p["metadata"]["generation_matches"] = json!(false);
    assert_eq!(
        result::normalize(p, &input(), "g1", None).unwrap()["status"],
        "stale"
    );
    let mut p = provider();
    p["project"] = json!("other");
    assert!(result::normalize(p, &input(), "g1", None).is_err());
    let mut p = provider();
    p["scopes"][0]["entries"] =
        json!([{"path":"src/excluded.rs","kind":"not_indexed_file","detail":"excluded"}]);
    p["scopes"][0]["total"] = json!(2);
    p["scopes"][0]["has_more"] = json!(true);
    p["scopes"][0]["status"] = json!("known_gaps");
    let r = result::normalize(p, &input(), "g1", None).unwrap();
    assert_eq!(r["pagination_complete"], false);
    assert_eq!(r["supports_scoped_claim"], false);
    let mut p = provider();
    p["paths"] = json!([]);
    assert!(result::normalize(p, &input(), "g1", None).is_err());
    let mut p = provider();
    p["scopes"][0]["has_more"] = json!(true);
    assert!(result::normalize(p, &input(), "g1", None).is_err());
}

#[test]
fn request_bounds_reject_escape_symlink_and_oversize_before_provider() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    #[cfg(unix)]
    assert!(request::validate(&mut json!({"paths":["escape/file"]}), root.path()).is_err());
    for path in [
        "/etc/passwd",
        "../file",
        "a/../file",
        "C:\\file",
        "./file",
        "a//b",
        "",
    ] {
        assert!(
            request::validate(&mut json!({"paths":[path]}), root.path()).is_err(),
            "{path}"
        );
    }
    assert!(request::validate(&mut json!({"paths":vec!["x";17]}), root.path()).is_err());
    assert!(request::validate(&mut json!({"scopes":["."],"scope_limit":33}), root.path()).is_err());
    assert!(request::validate(&mut json!({"scopes":["."],"scope_offset":1}), root.path()).is_err());
    assert!(
        request::validate(
            &mut json!({"scopes":["."],"scope_offset":1,"generation":"g1"}),
            root.path()
        )
        .is_ok()
    );
}

#[test]
fn projection_omits_raw_transcript_extras_and_detects_generation_race() {
    let mut p = provider();
    p["raw_transcript"] = json!("SECRET SOURCE");
    p["paths"][0]["source"] = json!("SECRET SOURCE");
    let report = result::normalize(p, &input(), "g1", None).unwrap();
    assert!(!report.to_string().contains("SECRET"));
    assert!(!provider_generation_changed(&report, &provider()));
    let mut changed = provider();
    changed["metadata"]["generation"] = json!("g2");
    assert!(provider_generation_changed(&report, &changed));
}

#[test]
fn second_confirmation_revalidates_metadata_and_ignores_unrecognized_extras() {
    let report = result::normalize(provider(), &input(), "g1", None).unwrap();
    let mut next = provider();
    next["ignored_future_field"] = json!("benign");
    assert!(!provider_generation_changed(&report, &next));
    next["metadata"]["recording_status"] = json!("partial");
    assert!(provider_generation_changed(&report, &next));
    next = provider();
    next["metadata"]["hash_records_complete"] = json!(false);
    assert!(provider_generation_changed(&report, &next));
}
#[test]
fn unequal_scope_totals_allow_continuing_after_one_scope_is_exhausted() {
    let mut request = input();
    request["scopes"] = json!(["src", "tests"]);
    request["scope_offset"] = json!(2);
    let mut p = provider();
    p["scopes"] = json!([
        {"requested_scope":"src","scope":"src","total":0,"has_more":false,"entries":[],"status":"no_recorded_issue"},
        {"requested_scope":"tests","scope":"tests","total":3,"has_more":false,"entries":[{"path":"tests/third.rs","kind":"excluded"}],"status":"known_gaps"}
    ]);
    let r = result::normalize(p, &request, "g1", Some("g1")).unwrap();
    assert_eq!(r["status"], "flagged");
    assert_eq!(
        r["pagination_complete"], false,
        "missing earlier pages cannot be claimed complete"
    );
}

#[test]
fn short_nonterminal_page_and_next_cursor_cannot_hide_pagination() {
    let mut p = provider();
    p["scopes"][0]["total"] = json!(3);
    p["scopes"][0]["has_more"] = json!(true);
    p["scopes"][0]["entries"] = json!([{"path":"src/gap.rs","kind":"excluded"}]);
    let mut request = input();
    request["scope_limit"] = json!(2);
    assert!(result::normalize(p, &request, "g1", None).is_err());
    let mut p = provider();
    p["scopes"][0]["nextCursor"] = json!("pending");
    let report = result::normalize(p, &input(), "g1", None).unwrap();
    assert_eq!(report["pagination_complete"], false);
    assert_eq!(report["supports_scoped_claim"], false);
}

#[test]
fn provider_coverage_flag_paths_cannot_escape_the_cited_source() {
    for path in ["/etc/passwd", "../outside", "other/file.rs"] {
        let mut p = provider();
        p["paths"][0]["coverage"] = json!([{"path":path,"kind":"excluded"}]);
        assert!(result::normalize(p, &input(), "g1", None).is_err());
    }
}
