use super::super::tests::test_support::*;
use super::super::{build_codebase_memory_toolset, build_codebase_memory_toolset_with_timeout};
use super::*;
use std::time::Duration;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};

fn server() -> tempfile::TempDir {
    let directory = fake_server_script();
    let path = directory.path().join("fake_codebase_memory_mcp.py");
    let mut script = std::fs::read_to_string(&path).unwrap();
    script=script.replace("def send(value):",r#"
TOOLS.append({"name":"check_index_coverage","description":"coverage","inputSchema":{"type":"object","properties":{"project":{"type":"string"},"paths":{"type":"array"},"scopes":{"type":"array"},"scope_limit":{"type":"integer"},"scope_offset":{"type":"integer"}}}})
if mode == "coverage-missing":
    TOOLS = [t for t in TOOLS if t["name"] != "check_index_coverage"]
def send(value):"#);
    script=script.replace("        elif name == \"index_repository\":",r#"        elif name == "check_index_coverage":
            if mode == "coverage-timeout": time.sleep(60)
            if mode == "coverage-malformed":
                tool_result(request["id"], "not-json")
                continue
            generation = "g2" if os.path.exists(log_path + ".generation") else "g1"
            payload = {"project":args["project"],"signal":"best_effort","indexed_at":generation,"metadata":{"generation":generation,"generation_matches":True,"recording_status":"complete","hash_records_complete":True},"paths":[{"requested_path":p,"path":p,"status":"no_recorded_issue","freshness":"metadata_match","coverage":[]} for p in args.get("paths",[])],"scopes":[{"requested_scope":s,"scope":s,"total":0,"has_more":False,"entries":[],"status":"no_known_gaps"} for s in args.get("scopes",[])]}
            if mode == "coverage-project": payload["project"] = "other"
            tool_result(request["id"], json.dumps(payload), structured=payload)
        elif name == "index_repository":"#);
    let confirmation = r#"confirmation = {"project": actual, "status": "ready", "root_path": binding["repo_path"]}"#;
    assert!(script.contains(confirmation));
    script = script.replace(
        confirmation,
        &format!(
            "{confirmation}\n                    if os.path.exists(log_path + \".checkout\"):\n                        confirmation[\"root_path\"] = os.path.join(binding[\"repo_path\"], \"other-checkout\")"
        ),
    );
    std::fs::write(path, script).unwrap();
    directory
}

#[test]
fn protocol_missing_malformed_timeout_and_mismatched_project_are_explicit() {
    for mode in [
        "coverage-missing",
        "coverage-malformed",
        "coverage-timeout",
        "coverage-project",
    ] {
        let server = server();
        let workspace = tempfile::tempdir().unwrap();
        let log = workspace.path().join("calls");
        let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
        let source = workspace.path().join("demo/src/lib.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(source, "source").unwrap();
        let config = config(
            &server,
            CodebaseMemoryMode::Required,
            CodebaseMemoryIndex::Blocking,
            mode,
            &log,
            json!({}),
        );
        temper_agent_io::block_on(async move {
            let tools = build_codebase_memory_toolset_with_timeout(
                Some(&config),
                "engineer",
                &context,
                workspace.path(),
                Duration::from_millis(250),
            )
            .await
            .unwrap();
            if mode == "coverage-missing" {
                assert!(tools.coverage().is_none());
                assert!(
                    tools
                        .prompt_status()
                        .unwrap()
                        .contains("Coverage capability unavailable")
                );
                return;
            }
            let coverage = CoverageTool(tools.coverage().unwrap());
            let output = coverage
                .execute("test", json!({"paths":["src/lib.rs"]}), None)
                .await
                .unwrap();
            assert!(output.is_error, "{mode}: {:?}", output.content);
            assert!(!output.details.unwrap().to_string().contains("lineage"));
            if mode == "coverage-timeout" {
                let before = std::fs::read_to_string(&log).unwrap();
                let output = coverage
                    .execute("no-retry", json!({"paths":["src/lib.rs"]}), None)
                    .await
                    .unwrap();
                assert!(output.is_error);
                assert_eq!(before, std::fs::read_to_string(&log).unwrap());
            }
        });
    }
}

struct Child {
    captured: Arc<Mutex<String>>,
    change: Option<std::path::PathBuf>,
}
#[async_trait]
impl Tool for Child {
    fn name(&self) -> &str {
        "investigate"
    }
    fn label(&self) -> &str {
        "investigate"
    }
    fn description(&self) -> &str {
        "Read-only child"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"task":{"type":"string"}}})
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }
    async fn execute(
        &self,
        _id: &str,
        input: Value,
        _update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::error::Result<ToolOutput> {
        *self.captured.lock().unwrap() = input["task"].as_str().unwrap().to_string();
        if let Some(path) = &self.change {
            std::fs::write(path, "changed source").unwrap();
        }
        Ok(ToolOutput {
            content: vec![],
            details: None,
            is_error: false,
        })
    }
}
fn handoff(id: &Value) -> Value {
    json!({"task":"Inspect the implementation","graph_context":{"evidence_id":id,"task_scope":"src/lib.rs implementation","sources":[{"path":"src/lib.rs","qualified_symbol":"demo::increment","origin":"graph_snippet"}],"relationships":[{"from":"demo::run","to":"demo::increment","kind":"calls"}],"query_bounds":["targeted search limit 4"],"pagination":"complete","limitations":["Parent graph evidence; relationship resolution is best effort"]}})
}

#[test]
fn child_context_returns_limitations_and_rejects_changed_checkout_generation_and_fresh_session() {
    for change in ["none", "source", "generation", "checkout", "fresh-session"] {
        let server = server();
        let workspace = tempfile::tempdir().unwrap();
        let log = workspace.path().join("calls");
        let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
        let path = workspace.path().join("demo/src/lib.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "source").unwrap();
        let config = config(
            &server,
            CodebaseMemoryMode::Required,
            CodebaseMemoryIndex::Blocking,
            "normal",
            &log,
            json!({}),
        );
        temper_agent_io::block_on(async move {
            let tools = build_codebase_memory_toolset(
                Some(&config),
                "engineer",
                &context,
                workspace.path(),
            )
            .await
            .unwrap();
            let service = tools.coverage().unwrap();
            let missing = CoverageTool(service.clone())
                .execute("missing", json!({"paths":["missing.rs"]}), None)
                .await
                .unwrap();
            assert!(missing.is_error);
            assert_eq!(missing.details.unwrap()["coverage_status"], "unavailable");
            let report = service
                .checked(json!({"paths":["src/lib.rs"],"scopes":["src"]}))
                .await
                .unwrap();
            assert_eq!(report["status"], "clean");
            let captured = Arc::new(Mutex::new(String::new()));
            let child = Box::new(Child {
                captured: captured.clone(),
                change: match change {
                    "source" => Some(path.clone()),
                    "checkout" => Some(std::path::PathBuf::from(format!(
                        "{}.checkout",
                        log.display()
                    ))),
                    "generation" => Some(std::path::PathBuf::from(format!(
                        "{}.generation",
                        log.display()
                    ))),
                    _ => None,
                },
            });
            let service = if change == "fresh-session" {
                build_codebase_memory_toolset(Some(&config), "engineer", &context, workspace.path())
                    .await
                    .unwrap()
                    .coverage()
                    .unwrap()
            } else {
                service
            };
            let pages = service.clone();
            let wrapper = GraphHandoffTool::new(child, service);
            let output = wrapper
                .execute("test", handoff(&report["evidence_id"]), None)
                .await
                .unwrap();
            assert_eq!(
                output.is_error,
                change != "none",
                "{change}: {:?}",
                output.content
            );
            if change == "none" {
                let mut first = report.clone();
                first["query_bounds"]["scope_limit"] = json!(1);
                first["provider"]["scopes"][0]["entries"] =
                    json!([{"path":"src/excluded.rs","kind":"excluded"}]);
                first["provider"]["scopes"][0]["total"] = json!(2);
                first["provider"]["scopes"][0]["has_more"] = json!(true);
                first["status"] = json!("flagged");
                first["pagination_complete"] = json!(false);
                pages.record(&mut first).unwrap();
                let mut second = first.clone();
                second["query_bounds"]["scope_offset"] = json!(1);
                second["provider"]["scopes"][0]["has_more"] = json!(false);
                second["provider"]["scopes"][0]["entries"] =
                    json!([{"path":"src/other.rs","kind":"excluded"}]);
                second["status"] = json!("clean");
                pages.record(&mut second).unwrap();
                assert_eq!(second["status"], "flagged");
                assert_eq!(second["pagination_complete"], true);
                assert_eq!(
                    second["scope_pages"][0][0]["entries"][0]["path"],
                    "src/excluded.rs"
                );
                assert_eq!(second["supports_scoped_claim"], false);
                second["provider"]["scopes"][0]["nextCursor"] = json!("pending");
                pages.record(&mut second).unwrap();
                assert_eq!(second["pagination_complete"], false);
                second["checkout_id"] = json!("other-checkout");
                pages.record(&mut second).unwrap();
                assert_eq!(second["pagination_complete"], false);

                let task = captured.lock().unwrap();
                assert!(task.contains("demo::increment"));
                assert!(task.contains("Parent graph evidence"));
                assert!(!task.contains(workspace.path().to_str().unwrap()));
                assert!(output.content.iter().any(|c| matches!(c,ContentBlock::Text(t) if t.text.contains("limitations") && t.text.contains("revalidated"))));
                assert!(output.details.is_none_or(|d| {
                    d.get(temper_agent_core::SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)
                        .is_none()
                }));
            }
        });
    }
}
