use mapped_live_graph_consumption_fixture::choose_dispatch;

#[test]
fn selected_dispatch_is_preserved_after_retry() {
    assert_eq!(choose_dispatch("primary", Some("selected"), 2), "selected");
}

#[test]
fn unselected_dispatch_is_preserved_after_retry() {
    assert_eq!(choose_dispatch("primary", None, 2), "primary");
}
