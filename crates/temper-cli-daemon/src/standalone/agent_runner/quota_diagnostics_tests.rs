use std::io;
use std::sync::{Arc, Mutex};

use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry;

use super::*;

#[derive(Clone, Default)]
struct SharedRenderBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedRenderBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("render buffer").write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedRenderBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn standalone_aggregate_warning_renders_all_capacity_values_without_changing_result() {
    let human = SharedRenderBuffer::default();
    let json = SharedRenderBuffer::default();
    let subscriber = registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(human.clone()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(json.clone()),
        );
    let error = temper_engine_io::block_on_with(move |_cx, handle| {
        async move {
            let temp = tempfile::tempdir().expect("standalone quota tempdir");
            std::fs::create_dir_all(temp.path().join("temper")).expect("prepared repo dir");
            let policy = AgentActivityCapturePolicyV1 {
                max_run_bytes: 5_000,
                max_inline_bytes: 1,
                max_blob_bytes: 1,
                ..Default::default()
            };
            let collector = TraceCollector::new(WorkerAgentTraceConfig {
                policy: policy.clone(),
                spool_root: Some(temp.path().join("spool")),
            });
            let context = context();
            let mut reservations = Vec::new();
            for index in 0..temper_worker::WORKER_SPOOL_RUN_CAPACITY {
                reservations.push(
                    collector
                        .begin_run(&format!("standalone-held-{index}"), &context)
                        .expect("reserve standalone trace")
                        .expect("trace enabled"),
                );
            }
            let provider = ProviderConfig::new(
                "test-provider",
                "test-model",
                "https://llm.example",
                "test-key",
            );
            let runner = InProcessAgentRunner::new(handle, provider, 1, None, false)
                .with_tool_config(Some(required_bad_tool_config()))
                .with_trace_policy(policy)
                .with_shared_trace_collector(collector);
            let error = runner
                .run("standalone-product-result", &context, temp.path())
                .await
                .expect_err("tool setup result remains an agent failure");
            assert_eq!(
                reservations.len() as u64,
                temper_worker::WORKER_SPOOL_RUN_CAPACITY
            );
            error
        }
        .with_subscriber(subscriber)
    });
    assert_eq!(error.class, FailureClass::Transient);
    assert!(error.message.contains("codebase-memory tool setup failed"));

    let records = String::from_utf8(json.0.lock().expect("JSON logs").clone()).expect("UTF-8 logs");
    let warning = records
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON event"))
        .find(|record| record["fields"]["event"] == "agent.activity.start_failed")
        .expect("standalone trace admission warning");
    let fields = &warning["fields"];
    assert_eq!(fields["runner"], "standalone");
    assert_eq!(fields["logical_reserved_bytes"], 80_000);
    assert_eq!(fields["requested_bytes"], 5_000);
    assert_eq!(fields["limit"], 80_000);
    assert_eq!(fields["dirty_run_count"], 16);
    let physical = fields["physical_used_bytes"]
        .as_u64()
        .expect("physical used bytes");
    let rendered =
        String::from_utf8(human.0.lock().expect("human logs").clone()).expect("UTF-8 logs");
    assert!(rendered.contains(&format!(
        "physical used bytes {physical}, logical reserved bytes 80000, requested bytes 5000, limit 80000, dirty runs 16"
    )));
}

fn context() -> WorkspaceContext {
    use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};

    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: "forgejo:ai/temper".to_string(),
            owner: "ai".to_string(),
            name: "temper".to_string(),
            default_branch: "main".to_string(),
            dir: "temper".to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: None,
        }],
        work_item: WorkspaceWorkItem {
            role: "architect".to_string(),
            queue: "intake".to_string(),
            kind: "issue".to_string(),
            target: "Issue { number: ItemNumber(746) }".to_string(),
            context: "{}".to_string(),
        },
        action: "triage_intake".to_string(),
        correlation_key: "k".to_string(),
        checkout: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}

fn required_bad_tool_config() -> AgentToolConfig {
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    };

    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
            args: Vec::new(),
            roles: vec!["architect".to_string()],
            index: CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 1,
            retention: Default::default(),
        }),
    }
}
