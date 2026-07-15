fn read_events_recovering_final_fragment(
    path: &Path,
) -> Result<(Vec<AgentRunEventV1>, bool), TraceJournalError> {
    if !path.exists() {
        return Ok((Vec::new(), false));
    }
    ensure_private_regular_file(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error(format!("open {} for recovery", path.display()), error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error(format!("read {}", path.display()), error))?;
    let mut events = Vec::new();
    let mut start = 0usize;
    let mut truncated = false;
    while start < bytes.len() {
        let newline = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| start + offset);
        let (end, terminated) = newline.map_or((bytes.len(), false), |end| (end, true));
        let record = &bytes[start..end];
        if record.is_empty() {
            return Err(TraceJournalError::CorruptRun(format!(
                "events JSONL contains an empty record at byte {start}"
            )));
        }
        match serde_json::from_slice::<AgentRunEventV1>(record) {
            Ok(event) => events.push(event),
            Err(_error) if !terminated && end == bytes.len() => {
                file.set_len(start as u64)
                    .map_err(|io| io_error("truncate incomplete final trace event", io))?;
                file.sync_all()
                    .map_err(|io| io_error("sync recovered trace events", io))?;
                truncated = true;
                break;
            }
            Err(error) => {
                return Err(TraceJournalError::CorruptRun(format!(
                    "invalid events JSONL record at byte {start}: {error}"
                )));
            }
        }
        start = if terminated { end + 1 } else { end };
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") && !truncated {
        file.seek(SeekFrom::End(0))
            .map_err(|error| io_error("seek recovered trace events", error))?;
        file.write_all(b"\n")
            .map_err(|error| io_error("terminate recovered trace event", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync terminated trace events", error))?;
    }
    Ok((events, truncated))
}

fn build_summary(
    paths: &RunPaths,
    manifest: &AgentTraceManifest,
    events: &[AgentRunEventV1],
) -> Result<AgentTraceSummary, TraceJournalError> {
    let mut status = AgentTraceRunStatus::Active;
    let mut completed_at = None;
    let mut usage = UsageV1 {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };
    let mut dropped_events = 0u64;
    for event in events {
        match &event.event {
            AgentActivityEventV1::RunFinished(finished) => {
                status = match finished.status {
                    temper_protocol_activity::RunStatusV1::Succeeded => {
                        AgentTraceRunStatus::Succeeded
                    }
                    temper_protocol_activity::RunStatusV1::Cancelled => {
                        AgentTraceRunStatus::Cancelled
                    }
                };
                completed_at = Some(event.occurred_at.clone());
            }
            AgentActivityEventV1::RunFailed(_) => {
                status = AgentTraceRunStatus::Failed;
                completed_at = Some(event.occurred_at.clone());
            }
            AgentActivityEventV1::Usage(value) => {
                usage.input_tokens = usage.input_tokens.saturating_add(value.input_tokens);
                usage.output_tokens = usage.output_tokens.saturating_add(value.output_tokens);
                usage.cache_read_tokens = usage
                    .cache_read_tokens
                    .saturating_add(value.cache_read_tokens);
                usage.cache_write_tokens = usage
                    .cache_write_tokens
                    .saturating_add(value.cache_write_tokens);
            }
            AgentActivityEventV1::TraceGap(value) => {
                dropped_events = dropped_events.saturating_add(value.dropped_events);
            }
            _ => {}
        }
    }
    let events_bytes = if paths.events.exists() {
        fs::metadata(&paths.events)
            .map_err(|error| io_error(format!("inspect {}", paths.events.display()), error))?
            .len()
    } else {
        0
    };
    let (blob_count, blob_bytes) = blob_usage(&paths.blobs)?;
    let stored_bytes = events_bytes.saturating_add(blob_bytes);
    Ok(AgentTraceSummary {
        format_version: JOURNAL_FORMAT_VERSION,
        run_id: manifest.run_id.clone(),
        status,
        first_seq: events.first().map(|event| event.seq),
        last_accepted_seq: events.last().map_or(0, |event| event.seq),
        event_count: events.len() as u64,
        started_at: events.first().map(|event| event.occurred_at.clone()),
        completed_at,
        usage,
        dropped_events,
        blob_count,
        blob_bytes,
        stored_bytes,
        quota_exceeded_for_required_boundaries: stored_bytes
            > manifest.capture_policy.max_run_bytes,
    })
}

fn blob_usage(directory: &Path) -> Result<(u64, u64), TraceJournalError> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for entry in fs::read_dir(directory).map_err(|error| {
        io_error(
            format!("read blob directory {}", directory.display()),
            error,
        )
    })? {
        let entry = entry.map_err(|error| io_error("read blob directory entry", error))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| io_error("inspect blob directory entry", error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if metadata.file_type().is_symlink() {
            return Err(TraceJournalError::CorruptRun(
                "blob directory contains a symbolic link".to_string(),
            ));
        }
        if metadata.is_file()
            && name.len() == 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            count += 1;
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok((count, bytes))
}

fn load_or_create_source_digest_key(root: &Path) -> Result<[u8; 32], TraceJournalError> {
    let path = root.join(".source-digest.key");
    if path.exists() {
        ensure_private_regular_file(&path)?;
        let bytes =
            fs::read(&path).map_err(|error| io_error(format!("read {}", path.display()), error))?;
        return bytes.try_into().map_err(|_| {
            TraceJournalError::CorruptRun(
                "source digest key must contain exactly 32 bytes".to_string(),
            )
        });
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|error| TraceJournalError::Io {
        operation: "generate source digest key".to_string(),
        source: std::io::Error::other(error.to_string()),
    })?;
    write_atomic_bytes(&path, &key, false)?;
    Ok(key)
}

fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), TraceJournalError> {
    let mut content = serde_json::to_vec_pretty(value)
        .map_err(|error| TraceJournalError::Serialization(error.to_string()))?;
    content.push(b'\n');
    write_atomic_bytes(path, &content, true)
}

fn write_atomic_bytes(path: &Path, content: &[u8], replace: bool) -> Result<(), TraceJournalError> {
    let parent = path.parent().ok_or_else(|| TraceJournalError::Io {
        operation: format!("resolve parent for {}", path.display()),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent"),
    })?;
    ensure_existing_secure_directory(parent)?;
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{counter}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut options = private_open_options();
    options.write(true).create_new(true);
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|error| {
            io_error(
                format!("create temporary file {}", temporary.display()),
                error,
            )
        })?;
        file.write_all(content)
            .map_err(|error| io_error(format!("write {}", temporary.display()), error))?;
        file.sync_all()
            .map_err(|error| io_error(format!("sync {}", temporary.display()), error))?;
        if !replace && path.exists() {
            return Err(TraceJournalError::Io {
                operation: format!("install immutable file {}", path.display()),
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "destination exists",
                ),
            });
        }
        fs::rename(&temporary, path).map_err(|error| {
            io_error(
                format!("replace {} from {}", path.display(), temporary.display()),
                error,
            )
        })?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, TraceJournalError> {
    ensure_private_regular_file(path)?;
    let bytes =
        fs::read(path).map_err(|error| io_error(format!("read {}", path.display()), error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        TraceJournalError::CorruptRun(format!("parse {}: {error}", path.display()))
    })
}

fn run_directories(root: &Path) -> Result<Vec<PathBuf>, TraceJournalError> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)
        .map_err(|error| io_error(format!("read runs root {}", root.display()), error))?
    {
        let entry = entry.map_err(|error| io_error("read run directory entry", error))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| io_error("inspect run directory entry", error))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

fn paths_for_directory(directory: PathBuf) -> RunPaths {
    RunPaths {
        manifest: directory.join("manifest.json"),
        events: directory.join("events.jsonl"),
        summary: directory.join("summary.json"),
        source_digests: directory.join("source-digests.jsonl"),
        blobs: directory.join("blobs"),
        directory,
    }
}

fn run_directory_name(run_id: &str) -> String {
    hex_digest(run_id.as_bytes())
}

fn hex_digest(bytes: &[u8]) -> String {
    render_hex(&Sha256::digest(bytes))
}

fn render_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push(char::from(HEX[usize::from(byte >> 4)]));
        rendered.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    rendered
}

fn acknowledgement(run_id: &str, seq: u64) -> AgentActivityAcknowledgement {
    AgentActivityAcknowledgement {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        highest_contiguous_seq: seq,
    }
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn ensure_secure_directory(path: &Path) -> Result<(), TraceJournalError> {
    if path.exists() {
        return ensure_existing_secure_directory(path);
    }
    fs::create_dir_all(path).map_err(|error| {
        io_error(
            format!("create private directory {}", path.display()),
            error,
        )
    })?;
    set_directory_permissions(path)
}

fn ensure_existing_secure_directory(path: &Path) -> Result<(), TraceJournalError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error(format!("inspect directory {}", path.display()), error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TraceJournalError::Io {
            operation: format!("secure directory {}", path.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path is not a real directory",
            ),
        });
    }
    set_directory_permissions(path)
}

fn ensure_private_regular_file(path: &Path) -> Result<(), TraceJournalError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error(format!("inspect file {}", path.display()), error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TraceJournalError::Io {
            operation: format!("secure file {}", path.display()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path is not a regular file",
            ),
        });
    }
    set_file_permissions(path)
}

fn open_private_file(path: &Path, append: bool, create: bool) -> Result<File, TraceJournalError> {
    let mut options = private_open_options();
    options.read(true).write(true).append(append).create(create);
    let file = options
        .open(path)
        .map_err(|error| io_error(format!("open private file {}", path.display()), error))?;
    set_file_permissions(path)?;
    Ok(file)
}

fn private_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), TraceJournalError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error(format!("set directory mode on {}", path.display()), error))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<(), TraceJournalError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<(), TraceJournalError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| io_error(format!("set file mode on {}", path.display()), error))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> Result<(), TraceJournalError> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), TraceJournalError> {
    let directory = File::open(path)
        .map_err(|error| io_error(format!("open directory {} for sync", path.display()), error))?;
    directory
        .sync_all()
        .map_err(|error| io_error(format!("sync directory {}", path.display()), error))
}

fn io_error(operation: impl Into<String>, source: std::io::Error) -> TraceJournalError {
    TraceJournalError::Io {
        operation: operation.into(),
        source,
    }
}
