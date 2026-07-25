//! Durable activity-trace controls for hermetic restart acceptance.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityChildRecordV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentRunEventV1, AgentScopeKindV1, AgentScopeV1, AssistantMessageV1,
    BlobAttachmentV1, BlobMediaTypeV1, CapturedContentV1,
};
use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
};

use super::stack::HermeticRealStack;

/// Complete valid prefix and referenced blobs planted in the worker spool by a
/// restart scenario.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermeticSeededTrace {
    pub run_id: String,
    pub events: Vec<AgentRunEventV1>,
    pub blobs: Vec<BlobAttachmentV1>,
}

/// A trace whose worker lifetime-ownership fence remains held by the fixture.
/// Dropping this value models the prior owner ending without a terminal event.
pub struct HermeticLiveTrace {
    _run: temper_worker::TraceRun,
    evidence: HermeticSeededTrace,
}

impl HermeticLiveTrace {
    pub fn run_id(&self) -> &str {
        &self.evidence.run_id
    }

    pub fn evidence(&self) -> &HermeticSeededTrace {
        &self.evidence
    }

    fn interrupt(self) -> HermeticSeededTrace {
        let Self { _run, evidence } = self;
        drop(_run);
        evidence
    }
}

/// Payload-bearing files and durable cursors for one worker trace spool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermeticTracePayloadSnapshot {
    pub manifest: Vec<u8>,
    pub events: Vec<u8>,
    pub blobs: BTreeMap<String, Vec<u8>>,
    pub last_sequence: u64,
    pub acknowledged_sequence: u64,
    pub compacted: bool,
}

impl HermeticRealStack {
    /// Plants a valid transcript-bearing non-terminal run, then releases its
    /// lifetime owner as if the previous worker process disappeared.
    pub fn seed_interrupted_agent_trace(
        &self,
        job_id: &str,
        blob_bytes: &[u8],
    ) -> Result<HermeticSeededTrace, String> {
        self.seed_live_agent_trace(job_id, blob_bytes)
            .map(HermeticLiveTrace::interrupt)
    }

    /// Plants a valid transcript-bearing non-terminal run while retaining its
    /// lifetime ownership fence. Recovery must classify this run as protected.
    pub fn seed_live_agent_trace(
        &self,
        job_id: &str,
        blob_bytes: &[u8],
    ) -> Result<HermeticLiveTrace, String> {
        if blob_bytes.is_empty() {
            return Err("hermetic trace evidence blob must not be empty".to_string());
        }
        let context = self.trace_seed_context(job_id);
        let run = self
            .trace_collector
            .begin_run(job_id, &context)
            .map_err(|error| format!("begin hermetic trace seed `{job_id}`: {error}"))?
            .ok_or_else(|| "agent traces are not enabled for this fixture".to_string())?;
        let attachment =
            BlobAttachmentV1::from_bytes(BlobMediaTypeV1::TextMarkdownUtf8, blob_bytes);
        let frame = AgentActivityFrameV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            occurred_at: "2026-07-24T10:00:00.000Z".to_string(),
            elapsed_ms: 7,
            scope: AgentScopeV1 {
                id: format!("seed-main-{job_id}"),
                kind: AgentScopeKindV1::Main,
                parent_id: None,
            },
            turn: Some(1),
            event: AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
                message_id: format!("seed-message-{job_id}"),
                content: CapturedContentV1::Blob {
                    blob: attachment.blob.clone(),
                },
            }),
        };
        run.accept_record(AgentActivityChildRecordV1 {
            frame,
            blobs: vec![attachment.clone()],
        })
        .map_err(|error| format!("append hermetic trace seed `{job_id}`: {error}"))?;
        let events = read_complete_trace_events(&run.spool_dir().join("events.jsonl"))?;
        let evidence = HermeticSeededTrace {
            run_id: run.run_id().to_string(),
            events,
            blobs: vec![attachment],
        };
        Ok(HermeticLiveTrace {
            _run: run,
            evidence,
        })
    }

    /// Appends an incomplete final JSONL fragment to an otherwise valid run.
    /// Startup recovery must retain the complete prefix and truncate only this
    /// fragment before terminalizing the abandoned stream.
    pub fn append_agent_trace_partial_tail(
        &self,
        run_id: &str,
        partial: &[u8],
    ) -> Result<(), String> {
        if partial.is_empty() || partial.contains(&b'\n') {
            return Err("partial trace tail must be non-empty and newline-free".to_string());
        }
        let path = self.trace_run_dir(run_id)?.join("events.jsonl");
        let mut events = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        events
            .write_all(partial)
            .and_then(|()| events.sync_all())
            .map_err(|error| format!("append {}: {error}", path.display()))
    }

    /// Adds one malformed active-spool sibling for quarantine acceptance.
    pub fn seed_malformed_agent_trace_sibling(
        &self,
        name: &str,
        manifest_bytes: &[u8],
    ) -> Result<(), String> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name == "quarantine"
        {
            return Err("malformed trace sibling requires one safe leaf name".to_string());
        }
        let root = self.trace_spool_root()?;
        let directory = root.join(name);
        std::fs::create_dir_all(directory.join("blobs"))
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
        std::fs::write(directory.join("manifest.json"), manifest_bytes)
            .map_err(|error| format!("write malformed trace sibling: {error}"))
    }

    /// Deterministic active/quarantine inventory used by restart assertions.
    pub fn trace_spool_inventory(&self) -> Result<temper_worker::TraceSpoolInventory, String> {
        self.trace_collector
            .inventory()
            .map_err(|error| format!("inventory hermetic trace spool: {error}"))
    }

    /// Reads only the immutable manifest and payload-bearing files plus durable
    /// acknowledgement state for one run. Lock files are intentionally omitted.
    pub fn trace_payload_snapshot(
        &self,
        run_id: &str,
    ) -> Result<HermeticTracePayloadSnapshot, String> {
        let run_dir = self.trace_run_dir(run_id)?;
        let manifest = std::fs::read(run_dir.join("manifest.json"))
            .map_err(|error| format!("read trace manifest `{run_id}`: {error}"))?;
        let events = std::fs::read(run_dir.join("events.jsonl"))
            .map_err(|error| format!("read trace events `{run_id}`: {error}"))?;
        let last_sequence = complete_trace_events(&events)?
            .last()
            .map_or(0, |event| event.seq);
        let acknowledgement = std::fs::read(run_dir.join("acknowledgement.json"))
            .map_err(|error| format!("read trace acknowledgement `{run_id}`: {error}"))?;
        let acknowledgement: serde_json::Value = serde_json::from_slice(&acknowledgement)
            .map_err(|error| format!("parse trace acknowledgement `{run_id}`: {error}"))?;
        let acknowledged_sequence = acknowledgement["highest_contiguous_seq"]
            .as_u64()
            .ok_or_else(|| format!("trace acknowledgement `{run_id}` has no cursor"))?;
        let mut blobs = BTreeMap::new();
        for entry in std::fs::read_dir(run_dir.join("blobs"))
            .map_err(|error| format!("read trace blobs `{run_id}`: {error}"))?
        {
            let entry = entry.map_err(|error| format!("read trace blob entry: {error}"))?;
            if entry
                .file_type()
                .map_err(|error| format!("inspect trace blob: {error}"))?
                .is_file()
            {
                blobs.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path())
                        .map_err(|error| format!("read trace blob: {error}"))?,
                );
            }
        }
        Ok(HermeticTracePayloadSnapshot {
            manifest,
            events,
            blobs,
            last_sequence,
            acknowledged_sequence,
            compacted: run_dir.join("compacted.json").is_file(),
        })
    }

    fn trace_seed_context(&self, job_id: &str) -> WorkspaceContext {
        let (owner, name) = self
            .primary_repo_path
            .split_once('/')
            .expect("hermetic primary repository is owner/name");
        WorkspaceContext {
            trace_context: None,
            repos: vec![WorkspaceRepository {
                id: format!("forgejo:{}", self.primary_repo_path),
                owner: owner.to_string(),
                name: name.to_string(),
                default_branch: "main".to_string(),
                dir: name.to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some(format!("agent/{job_id}")),
            }],
            work_item: WorkspaceWorkItem {
                role: self.role.clone(),
                queue: "code_ready".to_string(),
                kind: "code".to_string(),
                target: format!(
                    "Issue {{ number: ItemNumber({}) }}",
                    self.issue_number.get()
                ),
                context: "{}".to_string(),
            },
            artifact_context: None,
            action: "open_pr".to_string(),
            correlation_key: format!("trace-seed-{job_id}"),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: Some(AgentSessionState::new(format!("trace-seed-{job_id}"))),
        }
    }

    fn trace_spool_root(&self) -> Result<&Path, String> {
        self.worker_config
            .agent_traces
            .spool_root
            .as_deref()
            .ok_or_else(|| "agent traces are not enabled for this fixture".to_string())
    }

    fn trace_run_dir(&self, run_id: &str) -> Result<PathBuf, String> {
        if run_id.is_empty() || run_id.contains('/') || run_id.contains('\\') {
            return Err("trace run id must be one safe path component".to_string());
        }
        let directory = self.trace_spool_root()?.join(run_id);
        if !directory.is_dir() {
            return Err(format!("trace spool `{run_id}` does not exist"));
        }
        Ok(directory)
    }
}

fn read_complete_trace_events(path: &Path) -> Result<Vec<AgentRunEventV1>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    complete_trace_events(&bytes)
}

fn complete_trace_events(bytes: &[u8]) -> Result<Vec<AgentRunEventV1>, String> {
    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    bytes[..complete_len]
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| {
            serde_json::from_slice(record)
                .map_err(|error| format!("parse complete hermetic trace event: {error}"))
        })
        .collect()
}
