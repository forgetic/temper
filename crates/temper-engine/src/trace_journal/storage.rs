fn decode_attachments(
    attachments: &[BlobAttachmentV1],
) -> Result<BTreeMap<String, (BlobReferenceV1, Vec<u8>)>, TraceJournalError> {
    attachments
        .iter()
        .map(|attachment| {
            let bytes = attachment.decode()?;
            Ok((
                attachment.blob.digest.clone(),
                (attachment.blob.clone(), bytes),
            ))
        })
        .collect()
}

fn referenced_new_blob_bytes(
    event: &AgentRunEventV1,
    existing: &BTreeSet<String>,
    planned: &BTreeSet<String>,
    attachments: &BTreeMap<String, (BlobReferenceV1, Vec<u8>)>,
) -> Result<u64, TraceJournalError> {
    let mut added = BTreeSet::new();
    let mut bytes = 0u64;
    for reference in content_references(event) {
        if existing.contains(&reference.digest)
            || planned.contains(&reference.digest)
            || !added.insert(reference.digest.as_str())
        {
            continue;
        }
        let (attached_reference, content) =
            attachments.get(&reference.digest).ok_or_else(|| {
                TraceJournalError::PolicyViolation(format!(
                    "event references absent attachment {}",
                    reference.digest
                ))
            })?;
        if attached_reference != reference || content.len() as u64 != reference.bytes {
            return Err(TraceJournalError::PolicyViolation(format!(
                "attachment {} differs from its event reference",
                reference.digest
            )));
        }
        bytes = bytes.saturating_add(reference.bytes);
    }
    Ok(bytes)
}

fn create_manifest(
    paths: &RunPaths,
    manifest: &AgentTraceManifest,
) -> Result<(), TraceJournalError> {
    ensure_secure_directory(&paths.directory)?;
    ensure_secure_directory(&paths.blobs)?;
    let mut content = serde_json::to_vec_pretty(manifest)
        .map_err(|error| TraceJournalError::Serialization(error.to_string()))?;
    content.push(b'\n');
    write_atomic_bytes(&paths.manifest, &content, false)
}

fn append_source_digests(
    path: &Path,
    records: &[SourceDigestRecord],
) -> Result<(), TraceJournalError> {
    let mut bytes = Vec::new();
    for record in records {
        bytes.extend(
            serde_json::to_vec(record)
                .map_err(|error| TraceJournalError::Serialization(error.to_string()))?,
        );
        bytes.push(b'\n');
    }
    let mut file = open_private_file(path, true, true)?;
    file.write_all(&bytes)
        .map_err(|error| io_error(format!("append {}", path.display()), error))?;
    file.sync_all()
        .map_err(|error| io_error(format!("sync {}", path.display()), error))
}

fn read_source_digests(path: &Path) -> Result<BTreeMap<u64, String>, TraceJournalError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
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
    let mut records = BTreeMap::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let newline = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| start + offset);
        let (end, terminated) = newline.map_or((bytes.len(), false), |end| (end, true));
        let line = &bytes[start..end];
        let record = match serde_json::from_slice::<SourceDigestRecord>(line) {
            Ok(record) => record,
            Err(_error) if !terminated && end == bytes.len() => {
                file.set_len(start as u64)
                    .map_err(|error| io_error("truncate incomplete source digest", error))?;
                file.sync_all()
                    .map_err(|error| io_error("sync recovered source digests", error))?;
                break;
            }
            Err(error) => {
                return Err(TraceJournalError::CorruptRun(format!(
                    "invalid source digest record at byte {start}: {error}"
                )));
            }
        };
        if record.seq == 0
            || record.digest.len() != 71
            || !record.digest.starts_with("sha256:")
            || !record.digest[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || records.insert(record.seq, record.digest).is_some()
        {
            return Err(TraceJournalError::CorruptRun(
                "source digest index contains an invalid or duplicate sequence".to_string(),
            ));
        }
        start = if terminated { end + 1 } else { end };
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") && start == bytes.len() {
        file.seek(SeekFrom::End(0))
            .map_err(|error| io_error("seek recovered source digest index", error))?;
        file.write_all(b"\n")
            .map_err(|error| io_error("terminate recovered source digest", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync terminated source digest", error))?;
    }
    Ok(records)
}

fn source_event_digest(
    key: &[u8; 32],
    event: &AgentRunEventV1,
) -> Result<String, TraceJournalError> {
    let bytes = serde_json::to_vec(event)
        .map_err(|error| TraceJournalError::Serialization(error.to_string()))?;
    let mut digest = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|error| TraceJournalError::Serialization(error.to_string()))?;
    digest.update(&bytes);
    Ok(format!(
        "sha256:{}",
        render_hex(&digest.finalize().into_bytes())
    ))
}

fn append_events(path: &Path, events: &[AgentRunEventV1]) -> Result<(), TraceJournalError> {
    let mut bytes = Vec::new();
    for event in events {
        bytes.extend(event_line(event)?);
    }
    let mut file = open_private_file(path, true, true)?;
    file.write_all(&bytes)
        .map_err(|error| io_error(format!("append {}", path.display()), error))?;
    file.sync_all()
        .map_err(|error| io_error(format!("sync {}", path.display()), error))
}

fn event_line(event: &AgentRunEventV1) -> Result<Vec<u8>, TraceJournalError> {
    let mut line = serde_json::to_vec(event)
        .map_err(|error| TraceJournalError::Serialization(error.to_string()))?;
    line.push(b'\n');
    Ok(line)
}

fn store_blob(
    blobs_directory: &Path,
    attachment: &(BlobReferenceV1, Vec<u8>),
) -> Result<(), TraceJournalError> {
    let (reference, bytes) = attachment;
    let path = blob_path(blobs_directory, reference)?;
    if path.exists() {
        ensure_private_regular_file(&path)?;
        let existing = fs::read(&path)
            .map_err(|error| io_error(format!("read blob {}", path.display()), error))?;
        let actual = BlobReferenceV1::for_bytes(reference.media_type, &existing);
        if actual != *reference {
            return Err(TraceJournalError::CorruptRun(format!(
                "existing blob {} does not match its digest",
                reference.digest
            )));
        }
        return Ok(());
    }
    write_atomic_bytes(&path, bytes, false)
}

fn verify_referenced_blobs(
    blobs_directory: &Path,
    events: &[AgentRunEventV1],
) -> Result<(), TraceJournalError> {
    let mut checked = BTreeSet::new();
    for event in events {
        for reference in content_references(event) {
            if !checked.insert(reference.digest.clone()) {
                continue;
            }
            let path = blob_path(blobs_directory, reference)?;
            ensure_private_regular_file(&path)?;
            let bytes = fs::read(&path)
                .map_err(|error| io_error(format!("read blob {}", path.display()), error))?;
            if BlobReferenceV1::for_bytes(reference.media_type, &bytes) != *reference {
                return Err(TraceJournalError::CorruptRun(format!(
                    "blob {} fails content-address validation",
                    reference.digest
                )));
            }
        }
    }
    Ok(())
}

fn blob_path(directory: &Path, reference: &BlobReferenceV1) -> Result<PathBuf, TraceJournalError> {
    let digest = reference.digest.strip_prefix("sha256:").ok_or_else(|| {
        TraceJournalError::CorruptRun("blob digest has no sha256 prefix".to_string())
    })?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(TraceJournalError::CorruptRun(
            "blob digest is not canonical lowercase SHA-256".to_string(),
        ));
    }
    Ok(directory.join(digest))
}

fn existing_blob_digests(directory: &Path) -> Result<BTreeSet<String>, TraceJournalError> {
    let mut digests = BTreeSet::new();
    if !directory.exists() {
        return Ok(digests);
    }
    for entry in fs::read_dir(directory).map_err(|error| {
        io_error(
            format!("read blob directory {}", directory.display()),
            error,
        )
    })? {
        let entry = entry.map_err(|error| io_error("read blob directory entry", error))?;
        let metadata = entry
            .metadata()
            .map_err(|error| io_error("inspect blob directory entry", error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if metadata.is_file()
            && name.len() == 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            digests.insert(format!("sha256:{name}"));
        }
    }
    Ok(digests)
}

