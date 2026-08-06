// SPDX-License-Identifier: MPL-2.0

//! Negotiated, contained stdio provider adapter for host-only maintenance.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde_json::{Map, Value, json};
use temper_process_containment::{
    CleanupTrigger, ContainedProcess, ContainmentBackendFactory, ContainmentBackendPolicy,
    ContainmentCommand, ContainmentFactory, ContainmentIdentity, ContainmentScope, ContainmentSpec,
};

use crate::codebase_memory_retention::{
    CodebaseMemoryMaintenanceProvider, CodebaseMemoryProjectPage, CodebaseMemoryProjectRecord,
};

const PROVIDER_NAME: &str = "codebase-memory-mcp";
const MINIMUM_PROVIDER_VERSION: (u64, u64, u64) = (0, 9, 0);
const MAX_PROVIDER_RECORD_BYTES: usize = 1024 * 1024;
const MAX_TOOL_LIST_PAGES: usize = 8;
static PROVIDER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) struct ProviderSession {
    process: ContainedProcess,
    stdin: ChildStdin,
    responses: mpsc::Receiver<Result<Value, String>>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
}

impl ProviderSession {
    pub(super) fn connect(
        command: &str,
        args: &[String],
        timeout: Duration,
    ) -> Result<Self, String> {
        let mut containment_command = ContainmentCommand::new(command);
        containment_command
            .args(args.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let identity = PROVIDER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let spec = ContainmentSpec::new(
            ContainmentIdentity::new(format!("codebase-memory-maintenance-{identity:016x}"))
                .map_err(|error| format!("provider containment identity failed: {error}"))?
                .with_owner_identifier("codebase-memory-maintenance")
                .map_err(|error| format!("provider containment owner failed: {error}"))?,
            ContainmentScope::McpServer,
        )
        .with_timing(Duration::ZERO, Duration::from_millis(10));
        let process = production_containment_factory()
            .prepare(spec)
            .and_then(|prepared| prepared.spawn(containment_command))
            .map_err(|error| format!("spawn contained `{command}` failed: {error}"))?;
        let stdin = process
            .take_stdin()
            .map_err(|error| format!("take provider stdin failed: {error}"))?
            .ok_or("provider stdin was unavailable")?;
        let stdout = process
            .take_stdout()
            .map_err(|error| format!("take provider stdout failed: {error}"))?
            .ok_or("provider stdout was unavailable")?;
        let (send, responses) = mpsc::sync_channel(8);
        let reader = thread::Builder::new()
            .name("temper-codebase-memory-provider-reader".to_string())
            .spawn(move || read_responses(stdout, send))
            .map_err(|error| format!("spawn provider reader failed: {error}"))?;
        let mut session = Self {
            process,
            stdin,
            responses,
            reader: Some(reader),
            next_id: 1,
        };
        let deadline = Instant::now() + timeout;
        let initialize = session.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "temper-worker-maintenance", "version": env!("CARGO_PKG_VERSION")}
            }),
            deadline,
        )?;
        validate_initialize(&initialize)?;
        session.notify("notifications/initialized", json!({}))?;
        let tools = session.list_tools(deadline)?;
        validate_maintenance_tools(&tools)?;
        Ok(session)
    }

    fn list_tools(&mut self, deadline: Instant) -> Result<BTreeMap<String, Value>, String> {
        let mut tools = BTreeMap::new();
        let mut cursor = None;
        for _ in 0..MAX_TOOL_LIST_PAGES {
            let params = cursor
                .as_ref()
                .map(|cursor| json!({"cursor": cursor}))
                .unwrap_or_else(|| json!({}));
            let result = self.request("tools/list", params, deadline)?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or("provider tools/list omitted tools array")?;
            for descriptor in page {
                let name = descriptor
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("provider returned a tool without a name")?;
                if tools.insert(name.to_string(), descriptor.clone()).is_some() {
                    return Err("provider returned a duplicate tool name".to_string());
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.trim().is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err("provider tool inventory exceeded its negotiation page bound".to_string())
    }

    fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        deadline: Instant,
    ) -> Result<Value, String> {
        let result = self.request(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            deadline,
        )?;
        parse_tool_result(result)
    }

    fn request(&mut self, method: &str, params: Value, deadline: Instant) -> Result<Value, String> {
        if Instant::now() >= deadline {
            return Err(format!("provider `{method}` deadline expired"));
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("provider `{method}` deadline expired"));
            }
            let response = self
                .responses
                .recv_timeout(remaining)
                .map_err(|error| format!("provider `{method}` response unavailable: {error}"))??;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(format!(
                    "provider `{method}` RPC failed: {}",
                    bounded_json(error)
                ));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| format!("provider `{method}` response omitted result"));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn write(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|error| format!("encode provider request failed: {error}"))?;
        if bytes.len() >= MAX_PROVIDER_RECORD_BYTES {
            return Err("provider request exceeded protocol byte bound".to_string());
        }
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .and_then(|()| self.stdin.flush())
            .map_err(|error| format!("write provider request failed: {error}"))
    }
}

impl Drop for ProviderSession {
    fn drop(&mut self) {
        let _ = self.process.cleanup(CleanupTrigger::Cancellation);
        // Containment cleanup closes provider stdio and proves recursive
        // emptiness. Drop the receiver before joining so an overflowed reader
        // cannot remain blocked on delivery.
        let (_replacement_send, replacement) = mpsc::sync_channel(1);
        let old = std::mem::replace(&mut self.responses, replacement);
        drop(old);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl CodebaseMemoryMaintenanceProvider for ProviderSession {
    fn inventory_page(
        &mut self,
        cursor: Option<&str>,
        limit: u32,
        deadline: Instant,
    ) -> Result<CodebaseMemoryProjectPage, String> {
        let mut arguments = Map::new();
        arguments.insert("limit".to_string(), Value::from(limit));
        if let Some(cursor) = cursor {
            arguments.insert("cursor".to_string(), Value::String(cursor.to_string()));
        }
        let result = self.call_tool("list_projects", Value::Object(arguments), deadline)?;
        parse_inventory_page(&result)
    }

    fn delete_project(&mut self, project: &str, deadline: Instant) -> Result<(), String> {
        match self.call_tool("delete_project", json!({"project": project}), deadline) {
            Ok(_) => Ok(()),
            Err(error) if error.to_ascii_lowercase().contains("not found") => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "linux")]
fn production_containment_factory() -> ContainmentFactory {
    let backend: Arc<dyn ContainmentBackendFactory> =
        Arc::new(temper_process_containment::LinuxSupervisorBackendFactory::default());
    ContainmentFactory::new(ContainmentBackendPolicy::ForceLinuxSupervisor, backend)
}

#[cfg(windows)]
fn production_containment_factory() -> ContainmentFactory {
    let backend: Arc<dyn ContainmentBackendFactory> =
        Arc::new(temper_process_containment::WindowsJobBackendFactory);
    ContainmentFactory::new(ContainmentBackendPolicy::RequireWindowsJob, backend)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn production_containment_factory() -> ContainmentFactory {
    let backend: Arc<dyn ContainmentBackendFactory> =
        Arc::new(temper_process_containment::UnsupportedPlatformBackendFactory);
    ContainmentFactory::new(ContainmentBackendPolicy::Auto, backend)
}

fn read_responses(
    stdout: std::process::ChildStdout,
    send: mpsc::SyncSender<Result<Value, String>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_bounded_record(&mut reader) {
            Ok(None) => break,
            Ok(Some(record)) => match serde_json::from_slice::<Value>(&record) {
                Ok(value) => {
                    if send.try_send(Ok(value)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = send.try_send(Err(format!("decode provider response failed: {error}")));
                    break;
                }
            },
            Err(error) => {
                let _ = send.try_send(Err(error));
                break;
            }
        }
    }
}

fn read_bounded_record(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut record = Vec::with_capacity(8 * 1024);
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("read provider response failed: {error}"))?;
        if available.is_empty() {
            return if record.is_empty() {
                Ok(None)
            } else {
                Err("provider closed stdout in the middle of a response".to_string())
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if record.len().saturating_add(take) > MAX_PROVIDER_RECORD_BYTES {
            return Err("provider response exceeded protocol byte bound".to_string());
        }
        record.extend_from_slice(&available[..take]);
        let consumed = take + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(record));
        }
    }
}

fn validate_initialize(result: &Value) -> Result<(), String> {
    let server = result
        .get("serverInfo")
        .and_then(Value::as_object)
        .ok_or("provider initialize omitted serverInfo")?;
    let name = server.get("name").and_then(Value::as_str).unwrap_or("");
    if name != PROVIDER_NAME {
        return Err(format!(
            "provider identified `{name}` instead of `{PROVIDER_NAME}`"
        ));
    }
    let version = server.get("version").and_then(Value::as_str).unwrap_or("");
    if !version_at_least(version, MINIMUM_PROVIDER_VERSION) {
        return Err(format!("provider version `{version}` is incompatible"));
    }
    if result
        .get("capabilities")
        .and_then(|value| value.get("tools"))
        .is_none()
    {
        return Err("provider initialize did not advertise tools capability".to_string());
    }
    Ok(())
}

fn validate_maintenance_tools(tools: &BTreeMap<String, Value>) -> Result<(), String> {
    let list = tools
        .get("list_projects")
        .ok_or("provider did not advertise bounded list_projects maintenance")?;
    require_property(list, "limit", "integer", true)?;
    require_property(list, "cursor", "string", false)?;
    let delete = tools
        .get("delete_project")
        .ok_or("provider did not advertise delete_project maintenance")?;
    require_property(delete, "project", "string", true)
}

fn require_property(
    descriptor: &Value,
    property: &str,
    expected_type: &str,
    required: bool,
) -> Result<(), String> {
    let schema = descriptor
        .get("inputSchema")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("provider tool omitted inputSchema for `{property}`"))?;
    let actual_type = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(property))
        .and_then(|property| property.get("type"))
        .and_then(Value::as_str);
    if actual_type != Some(expected_type) {
        return Err(format!(
            "provider tool did not advertise {expected_type} `{property}`"
        ));
    }
    if required
        && !schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields.iter().any(|field| field == property))
    {
        return Err(format!("provider tool did not require `{property}`"));
    }
    Ok(())
}

fn parse_tool_result(result: Value) -> Result<Value, String> {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let value = if let Some(structured) = result.get("structuredContent") {
        structured.clone()
    } else {
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| content.iter().find_map(|block| block.get("text")))
            .and_then(Value::as_str)
            .ok_or("provider tool result omitted JSON content")?;
        serde_json::from_str(text)
            .map_err(|error| format!("provider tool result was not JSON: {error}"))?
    };
    if is_error {
        return Err(format!(
            "provider tool returned an error: {}",
            bounded_json(&value)
        ));
    }
    Ok(value)
}

fn parse_inventory_page(value: &Value) -> Result<CodebaseMemoryProjectPage, String> {
    let object = value
        .as_object()
        .ok_or("provider inventory page was not an object")?;
    let projects = object
        .get("projects")
        .and_then(Value::as_array)
        .ok_or("provider inventory page omitted projects")?
        .iter()
        .map(parse_project_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CodebaseMemoryProjectPage {
        cache_instance_id: string_field(object, &["cache_instance_id", "cacheInstanceId"]),
        projects,
        next_cursor: string_field(object, &["next_cursor", "nextCursor"]),
    })
}

fn parse_project_record(value: &Value) -> Result<CodebaseMemoryProjectRecord, String> {
    let object = value
        .as_object()
        .ok_or("provider project record was not an object")?;
    let metadata = object.get("metadata").and_then(Value::as_object);
    let project = string_field(object, &["project", "name", "id"]);
    let repo_path = string_field(object, &["repo_path", "repoPath", "path"])
        .or_else(|| {
            metadata.and_then(|value| string_field(value, &["repo_path", "repoPath", "path"]))
        })
        .map(PathBuf::from);
    let updated_at_unix_secs = unix_timestamp_field(
        object,
        &[
            "updated_at_unix_secs",
            "updatedAtUnixSecs",
            "updated_at",
            "updatedAt",
        ],
    )
    .or_else(|| {
        metadata.and_then(|value| {
            unix_timestamp_field(
                value,
                &[
                    "updated_at_unix_secs",
                    "updatedAtUnixSecs",
                    "updated_at",
                    "updatedAt",
                ],
            )
        })
    });
    let ownership = string_field(object, &["ownership", "managed_by", "managedBy"]).or_else(|| {
        metadata.and_then(|value| string_field(value, &["ownership", "managed_by", "managedBy"]))
    });
    Ok(CodebaseMemoryProjectRecord {
        project,
        repo_path,
        updated_at_unix_secs,
        ownership,
    })
}

fn string_field(object: &Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn unix_timestamp_field(object: &Map<String, Value>, fields: &[&str]) -> Option<u64> {
    fields.iter().find_map(|field| {
        let value = object.get(*field)?;
        value.as_u64().or_else(|| {
            let raw = value.as_str()?;
            raw.parse().ok().or_else(|| {
                DateTime::parse_from_rfc3339(raw)
                    .ok()
                    .and_then(|time| u64::try_from(time.timestamp()).ok())
            })
        })
    })
}

fn bounded_json(value: &Value) -> String {
    let rendered = serde_json::to_string(value).unwrap_or_else(|_| "<invalid-json>".to_string());
    rendered.chars().take(512).collect()
}

fn version_at_least(raw: &str, minimum: (u64, u64, u64)) -> bool {
    let raw = raw.strip_prefix('v').unwrap_or(raw);
    let without_build = raw.split_once('+').map_or(raw, |(version, _)| version);
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, false), |(version, _)| (version, true));
    let mut pieces = core.split('.');
    let parsed = (
        pieces.next().and_then(|value| value.parse().ok()),
        pieces.next().and_then(|value| value.parse().ok()),
        pieces.next().and_then(|value| value.parse().ok()),
    );
    if pieces.next().is_some() {
        return false;
    }
    matches!(
        parsed,
        (Some(major), Some(minor), Some(patch))
            if (major, minor, patch) >= minimum
                && (!prerelease || (major, minor, patch) > minimum)
    )
}

pub(super) fn unix_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "provider/tests.rs"]
mod tests;
