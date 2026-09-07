use super::*;

#[test]
#[ignore = "installed provider external root rebinding and account lifetime; isolated account only"]
fn installed_provider_rebind_never_presents_foreign_source_and_external_session_retains_daemon() {
    let runtime = ProviderRuntime::new();
    let context = workspace_context(runtime.directory.path(), &[("acme", "demo", "demo")]);
    let a_root = runtime.directory.path().join("demo");
    let b_root = runtime.directory.path().join("external");
    fs::create_dir(&b_root).unwrap();
    for (root, literal) in [(&a_root, "CHECKOUT_A_1280"), (&b_root, "CHECKOUT_B_1280")] {
        fs::write(
            root.join("same.py"),
            format!("def same_symbol():\n    return '{literal}'\n"),
        )
        .unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(root)
                .status()
                .unwrap()
                .success()
        );
    }
    let (manager, bootstrap) = runtime.bootstrap(&a_root);
    let completion = bootstrap.completion();
    let daemon = lifecycle::daemon_pid(&runtime).expect("cold bootstrap daemon");
    let daemon_start = lifecycle::start_tick(daemon).unwrap();
    let project = scope::provider_key_for_repo(&context.repos[0]);
    temper_agent_io::block_on(async move {
        let config = runtime.agent_config();
        let toolset = super::super::super::build_managed_codebase_memory_toolset(
            Some(&config),
            "engineer",
            &context,
            runtime.directory.path(),
            TIMEOUT,
            &crate::containment_tests::containment_context(),
            bootstrap,
        )
        .await
        .unwrap();
        assert_eq!(lifecycle::daemon_pid(&runtime), Some(daemon));
        assert_eq!(
            lifecycle::start_tick(daemon),
            Some(daemon_start),
            "healthy discovery and fresh serving preserve native generation"
        );
        let tools = toolset.into_tools();
        let graph = call(
            &tools,
            "search_graph",
            json!({"name_pattern":"^same_symbol$"}),
        )
        .await;
        let graph: Value = serde_json::Deserializer::from_str(&graph)
            .into_iter::<Value>()
            .next()
            .unwrap()
            .unwrap();
        let qualified = graph["results"][0]["qualified_name"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = call(
            &tools,
            "get_code_snippet",
            json!({"qualified_name":qualified}),
        )
        .await;
        assert!(before.contains("CHECKOUT_A_1280") && !before.contains("CHECKOUT_B_1280"));

        // This independent account session is outside Temper's serving registry.
        let external =
            StdioMcpClient::connect(runtime.mcp_config().with_working_directory(&b_root))
                .await
                .unwrap();
        let rebind = external
            .call_tool(
                "index_repository",
                // Requesting a cheaper mode forces full publication when bytes
                // change; upstream preserves the existing full coverage level.
                // The installed provider's incremental repair can retain A's
                // root metadata despite reporting a successful B index request.
                json!({"repo_path":b_root,"name":project,"mode":"fast"}),
                TIMEOUT,
            )
            .await
            .unwrap();
        assert!(!rebind.is_error);
        drop(external);
        let external =
            StdioMcpClient::connect(runtime.mcp_config().with_working_directory(&b_root))
                .await
                .unwrap();
        let fresh_status = external
            .call_tool("index_status", json!({"project":project}), TIMEOUT)
            .await
            .unwrap();
        let fresh_status: Value = serde_json::from_str(&fresh_status.text).unwrap();
        assert_eq!(fresh_status["root_path"], b_root.display().to_string());
        let foreign = external
            .call_tool(
                "get_code_snippet",
                json!({"project":project,"qualified_name":qualified}),
                TIMEOUT,
            )
            .await
            .unwrap();
        let foreign: Value = serde_json::from_str(&foreign.text).unwrap();
        assert_eq!(
            foreign["file_path"],
            b_root.join("same.py").display().to_string()
        );
        assert_eq!(
            foreign["source"],
            "def same_symbol():\n    return 'CHECKOUT_B_1280'\n"
        );
        let snippet = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
            .unwrap();
        let guarded = snippet
            .execute("rebound", json!({"qualified_name":qualified}), None)
            .await
            .unwrap();
        let guarded_text = output_text(&guarded);
        let details = serde_json::to_string(&guarded.details).unwrap();
        assert!(!guarded_text.contains("CHECKOUT_B_1280") && !details.contains("CHECKOUT_B_1280"));
        assert!(guarded.is_error || guarded_text.contains("CHECKOUT_A_1280"));
        let restored = external
            .call_tool(
                "index_repository",
                json!({"repo_path":a_root,"name":project,"mode":"fast"}),
                TIMEOUT,
            )
            .await
            .unwrap();
        assert!(!restored.is_error);
        // Native sessions cache opened generations. Keep the external account
        // admission continuous while replacing its B reader with an A reader.
        let renewed_external =
            StdioMcpClient::connect(runtime.mcp_config().with_working_directory(&a_root))
                .await
                .unwrap();
        drop(external);
        let external = renewed_external;
        let restored_status = external
            .call_tool("index_status", json!({"project":project}), TIMEOUT)
            .await
            .unwrap();
        let restored_status: Value = serde_json::from_str(&restored_status.text).unwrap();
        assert_eq!(restored_status["root_path"], a_root.display().to_string());
        let recovery = snippet
            .execute("restored", json!({"qualified_name":qualified}), None)
            .await
            .unwrap();
        if guarded.is_error {
            assert!(
                recovery.is_error,
                "source rejection keeps the old circuit closed"
            );
        } else {
            let recovery_text = output_text(&recovery);
            assert!(recovery.is_error || recovery_text.contains("CHECKOUT_A_1280"));
            assert!(!recovery_text.contains("CHECKOUT_B_1280"));
        }
        drop(tools);
        let (recovery_manager, recovery_bootstrap) = runtime.bootstrap(&a_root);
        let recovery_completion = recovery_bootstrap.completion();
        let recovered = super::super::super::build_managed_codebase_memory_toolset(
            Some(&config),
            "engineer",
            &context,
            runtime.directory.path(),
            TIMEOUT,
            &crate::containment_tests::containment_context(),
            recovery_bootstrap,
        )
        .await
        .unwrap()
        .into_tools();
        let recovered_source = call(
            &recovered,
            "get_code_snippet",
            json!({"qualified_name":qualified}),
        )
        .await;
        assert!(recovered_source.contains("CHECKOUT_A_1280"));
        assert!(!recovered_source.contains("CHECKOUT_B_1280"));
        drop(recovered);
        assert_eq!(
            recovery_completion
                .wait(Duration::from_secs(4))
                .unwrap()
                .disposition(),
            temper_process_containment::CleanupDisposition::AlreadyEmpty
        );
        recovery_manager.reap_completed();
        assert!(
            completion.wait(Duration::from_secs(2)).is_none(),
            "last local Temper client does not kill another account session"
        );
        let external_query = external
            .call_tool(
                "get_code_snippet",
                json!({"project":project,"qualified_name":qualified}),
                TIMEOUT,
            )
            .await
            .unwrap();
        assert!(!external_query.is_error);
        let external_query: Value = serde_json::from_str(&external_query.text).unwrap();
        assert_eq!(
            external_query["source"],
            "def same_symbol():\n    return 'CHECKOUT_A_1280'\n"
        );
        drop(external);
        let report = completion
            .wait(Duration::from_secs(40))
            .expect("upstream last account session naturally shuts down");
        assert_eq!(
            report.disposition(),
            temper_process_containment::CleanupDisposition::AlreadyEmpty
        );
        assert!(
            lifecycle::daemon_pid(&runtime).is_none(),
            "natural exit before fallback stop"
        );
        manager.reap_completed();
        println!(
            "cbm_source_rebind healthy_handoff_same_generation=true same_project_symbol_path_lines=true external_b_source_positive=true wrapped_a_foreign_bytes=false restored_a_fresh_toolset_source_positive=true final_temper_client_preserves_external=true final_account_exit=natural"
        );
    });
}
