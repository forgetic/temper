impl AgentTraceJournal {
    /// Opens, recovers, summarizes, and performs startup retention cleanup.
    pub fn open(config: TraceJournalConfig) -> Result<Self, TraceJournalError> {
        Self::open_with_clock_and_protection(
            config,
            system_clock(),
            &RetentionProtection::default(),
        )
    }

    /// Opens with an injected wall clock for deterministic quota/retention tests.
    pub fn open_with_clock(
        config: TraceJournalConfig,
        clock: WallClock,
    ) -> Result<Self, TraceJournalError> {
        Self::open_with_clock_and_protection(config, clock, &RetentionProtection::default())
    }

    /// Opens with startup retention protection for assignments recovered before
    /// the trace subsystem is initialized.
    pub fn open_with_clock_and_protection(
        config: TraceJournalConfig,
        clock: WallClock,
        protection: &RetentionProtection,
    ) -> Result<Self, TraceJournalError> {
        config.policy.validate()?;
        ensure_secure_directory(&config.root)?;
        let runs_root = config.root.join("runs");
        ensure_secure_directory(&runs_root)?;
        let lock_path = config.root.join(".journal.lock");
        let lock_file = open_private_file(&lock_path, false, true)?;
        lock_file
            .lock_exclusive()
            .map_err(|error| io_error("lock trace journal for key initialization", error))?;
        let key_result = load_or_create_source_digest_key(&config.root);
        let unlock_result = FileExt::unlock(&lock_file)
            .map_err(|error| io_error("unlock trace journal after key initialization", error));
        let source_digest_key = match (key_result, unlock_result) {
            (Err(error), _) => return Err(error),
            (Ok(_), Err(error)) => return Err(error),
            (Ok(key), Ok(())) => key,
        };
        let journal = Self {
            inner: Arc::new(JournalInner {
                root: config.root,
                runs_root,
                policy: config.policy,
                clock,
                process_lock: Mutex::new(()),
                lock_file,
                source_digest_key,
            }),
        };

        let report = journal.with_store_lock(|journal| journal.recover_all_locked())?;
        for failure in &report.failures {
            tracing::warn!(
                target: "temper::engine",
                run_directory = %failure.run_directory,
                error = %failure.error,
                "agent trace recovery skipped a corrupt run"
            );
        }
        let retention =
            journal.with_store_lock(|journal| journal.cleanup_retention_locked(protection))?;
        for failure in &retention.failures {
            tracing::warn!(
                target: "temper::engine",
                run_directory = %failure.run_directory,
                error = %failure.error,
                "agent trace startup retention skipped a run"
            );
        }
        Ok(journal)
    }

    /// Builds the optional journal directly from the engine subsystem config.
    pub fn from_engine_config(
        config: &EngineAgentTraceConfig,
    ) -> Result<Option<Self>, TraceJournalError> {
        Self::from_engine_config_with_clock_and_protection(
            config,
            system_clock(),
            &RetentionProtection::default(),
        )
    }

    /// Variant used by engine startup after durable assignments have been
    /// recovered, so retention cannot race those in-flight jobs.
    pub fn from_engine_config_with_clock_and_protection(
        config: &EngineAgentTraceConfig,
        clock: WallClock,
        protection: &RetentionProtection,
    ) -> Result<Option<Self>, TraceJournalError> {
        let Some(root) = &config.journal_root else {
            return Ok(None);
        };
        Self::open_with_clock_and_protection(
            TraceJournalConfig {
                root: root.clone(),
                policy: config.policy.clone(),
            },
            clock,
            protection,
        )
        .map(Some)
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Stable, traversal-safe directory for a run. The external run ID remains
    /// in the immutable manifest; the path component is its SHA-256 digest.
    pub fn run_directory(&self, run_id: &str) -> PathBuf {
        self.paths(run_id).directory
    }

    /// Authenticated internal ingestion API. Transport wiring belongs to the
    /// worker-forwarding feature, not this component.
    pub fn ingest(
        &self,
        binding: &AuthenticatedWorkerBinding,
        batch: &AgentActivityBatch,
    ) -> Result<AgentActivityAcknowledgement, TraceJournalError> {
        self.with_store_lock(|journal| journal.ingest_locked(binding, batch))
    }

    /// Rebuilds every readable summary and truncates only malformed, unterminated
    /// final JSON fragments.
    pub fn recover(&self) -> Result<TraceRecoveryReport, TraceJournalError> {
        self.with_store_lock(|journal| journal.recover_all_locked())
    }

    /// Callable periodic retention pass.
    pub fn cleanup_retention(
        &self,
        protection: &RetentionProtection,
    ) -> Result<RetentionReport, TraceJournalError> {
        self.with_store_lock(|journal| journal.cleanup_retention_locked(protection))
    }

    /// Read helpers retained for ingestion/recovery tests and narrow callers.
    pub fn manifest(&self, run_id: &str) -> Result<Option<AgentTraceManifest>, TraceJournalError> {
        self.with_store_lock(|journal| {
            let paths = journal.paths(run_id);
            if !paths.manifest.exists() {
                return Ok(None);
            }
            let manifest: AgentTraceManifest = read_json(&paths.manifest)?;
            if manifest.run_id != run_id {
                return Err(TraceJournalError::CorruptRun(
                    "manifest run ID does not match its content-addressed directory".to_string(),
                ));
            }
            Ok(Some(manifest))
        })
    }

    pub fn summary(&self, run_id: &str) -> Result<Option<AgentTraceSummary>, TraceJournalError> {
        self.with_store_lock(|journal| {
            let paths = journal.paths(run_id);
            if !paths.manifest.exists() {
                return Ok(None);
            }
            let recovered = journal.recover_run_locked(&paths, true)?;
            Ok(Some(recovered.summary))
        })
    }

    pub fn events(&self, run_id: &str) -> Result<Vec<AgentRunEventV1>, TraceJournalError> {
        self.with_store_lock(|journal| {
            let paths = journal.paths(run_id);
            if !paths.manifest.exists() {
                return Ok(Vec::new());
            }
            Ok(journal.recover_run_locked(&paths, true)?.events)
        })
    }

    pub fn audit_records(&self) -> Result<Vec<TraceAuditRecord>, TraceJournalError> {
        self.with_store_lock(|journal| {
            let path = journal.inner.root.join("audit.jsonl");
            if !path.exists() {
                return Ok(Vec::new());
            }
            let bytes = fs::read(&path).map_err(|error| io_error("read trace audit log", error))?;
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                TraceJournalError::CorruptRun(format!("audit log is not UTF-8: {error}"))
            })?;
            text.lines()
                .map(|line| {
                    serde_json::from_str(line).map_err(|error| {
                        TraceJournalError::CorruptRun(format!(
                            "invalid trace audit record: {error}"
                        ))
                    })
                })
                .collect()
        })
    }

    fn ingest_locked(
        &self,
        binding: &AuthenticatedWorkerBinding,
        batch: &AgentActivityBatch,
    ) -> Result<AgentActivityAcknowledgement, TraceJournalError> {
        if self.inner.policy.capture == CaptureModeV1::Off {
            return Err(TraceJournalError::Disabled);
        }
        validate_binding(binding)?;
        if batch.events.len() > MAX_BATCH_EVENTS {
            return Err(TraceJournalError::PolicyViolation(format!(
                "batch contains more than {MAX_BATCH_EVENTS} events"
            )));
        }
        if batch.blobs.len() > MAX_BATCH_BLOBS {
            return Err(TraceJournalError::PolicyViolation(format!(
                "batch contains more than {MAX_BATCH_BLOBS} blobs"
            )));
        }
        let absolute_encoded_blob = MAX_BLOB_ATTACHMENT_BYTES.div_ceil(3) as u64 * 4;
        let encoded_blob_limit = self
            .inner
            .policy
            .max_run_bytes
            .saturating_mul(2)
            .max(absolute_encoded_blob)
            .min(MAX_BATCH_ENCODED_BLOB_BYTES);
        let encoded_blob_bytes = batch.blobs.iter().try_fold(0u64, |total, blob| {
            total.checked_add(blob.data_base64.len() as u64)
        });
        if encoded_blob_bytes.is_none_or(|bytes| bytes > encoded_blob_limit) {
            return Err(TraceJournalError::PolicyViolation(format!(
                "batch blob payload exceeds the {encoded_blob_limit}-byte ingestion bound"
            )));
        }
        // Sanitize the untrusted representation before canonical validation;
        // producer and worker validation may both have been bypassed.
        let mut validation_batch = batch.clone();
        for event in &mut validation_batch.events {
            event.event.sanitize_retry_failure_message();
        }
        validation_batch.validate()?;
        for event in &batch.events {
            if event.assignment != binding.assignment
                || event.agent_session_id != binding.agent_session_id
            {
                return Err(TraceJournalError::BindingMismatch);
            }
        }

        let paths = self.paths(&batch.run_id);
        let existing = if paths.manifest.exists() {
            let recovered = self.recover_run_locked(&paths, false)?;
            if !manifest_matches_binding(&recovered.manifest, binding) {
                return Err(TraceJournalError::BindingMismatch);
            }
            Some(recovered)
        } else {
            if binding.capture_policy != self.inner.policy {
                return Err(TraceJournalError::PolicyViolation(
                    "worker capture policy does not match the engine policy".to_string(),
                ));
            }
            None
        };
        let durable_seq = existing
            .as_ref()
            .map_or(0, |run| run.summary.last_accepted_seq);
        let expected = durable_seq.saturating_add(1);
        if batch.first_seq > expected {
            if durable_seq == 0 {
                return Err(TraceJournalError::SequenceGap {
                    expected,
                    received: batch.first_seq,
                });
            }
            return Ok(acknowledgement(&batch.run_id, durable_seq));
        }

        let capture = binding.capture_policy.capture;
        let attachments = decode_attachments(&batch.blobs, capture, &batch.events)?;
        let mut durable_events = existing
            .as_ref()
            .map_or_else(Vec::new, |run| run.events.clone());
        let source_digests = existing
            .as_ref()
            .map_or_else(BTreeMap::new, |run| run.source_digests.clone());

        let mut first_new_index = batch.events.len();
        for (index, incoming) in batch.events.iter().enumerate() {
            let sanitized = sanitize_for_policy(incoming.clone(), &binding.capture_policy);
            if incoming.seq <= durable_seq {
                let source_digest = source_event_digest(&self.inner.source_digest_key, incoming)?;
                if let Some(durable_digest) = source_digests.get(&incoming.seq) {
                    if durable_digest != &source_digest {
                        self.audit_conflict(binding, &batch.run_id, incoming.seq)?;
                        return Err(TraceJournalError::ConflictingRetransmit { seq: incoming.seq });
                    }
                    continue;
                }
                let durable = durable_events
                    .get((incoming.seq - 1) as usize)
                    .ok_or_else(|| {
                        TraceJournalError::CorruptRun(
                            "durable sequence index is missing".to_string(),
                        )
                    })?;
                if durable != &sanitized && durable != &strip_optional_content(sanitized.clone()) {
                    self.audit_conflict(binding, &batch.run_id, incoming.seq)?;
                    return Err(TraceJournalError::ConflictingRetransmit { seq: incoming.seq });
                }
            } else {
                first_new_index = index;
                break;
            }
        }

        let manifest = existing.as_ref().map_or_else(
            || AgentTraceManifest {
                format_version: JOURNAL_FORMAT_VERSION,
                run_id: batch.run_id.clone(),
                worker_id: binding.worker_id.clone(),
                assignment_id: binding.assignment_id.clone(),
                assignment: binding.assignment.clone(),
                agent_session_id: binding.agent_session_id.clone(),
                capture_policy: binding.capture_policy.clone(),
                created_at: timestamp((self.inner.clock)()),
            },
            |run| run.manifest.clone(),
        );

        let mut projected_bytes = existing.as_ref().map_or(0, |run| run.summary.stored_bytes);
        let mut known_blobs = existing_blob_digests(&paths.blobs)?;
        let mut new_blob_digests = BTreeSet::new();
        let mut new_events = Vec::new();
        let mut new_source_digests = Vec::new();
        for incoming in batch.events.iter().skip(first_new_index) {
            let source_digest = source_event_digest(&self.inner.source_digest_key, incoming)?;
            if let Some(durable_digest) = source_digests.get(&incoming.seq) {
                if durable_digest != &source_digest {
                    self.audit_conflict(binding, &batch.run_id, incoming.seq)?;
                    return Err(TraceJournalError::ConflictingRetransmit { seq: incoming.seq });
                }
            } else {
                new_source_digests.push(SourceDigestRecord {
                    seq: incoming.seq,
                    digest: source_digest,
                });
            }
            let mut event = sanitize_for_policy(incoming.clone(), &binding.capture_policy);
            let mut line = event_line(&event)?;
            let mut blob_increment =
                referenced_new_blob_bytes(&event, &known_blobs, &new_blob_digests, &attachments)?;
            let mut increment = (line.len() as u64).saturating_add(blob_increment);
            if projected_bytes.saturating_add(increment) > binding.capture_policy.max_run_bytes {
                event = strip_optional_content(event);
                line = event_line(&event)?;
                blob_increment = referenced_new_blob_bytes(
                    &event,
                    &known_blobs,
                    &new_blob_digests,
                    &attachments,
                )?;
                increment = (line.len() as u64).saturating_add(blob_increment);
            }
            projected_bytes = projected_bytes.saturating_add(increment);
            for reference in content_references(&event) {
                if !known_blobs.contains(&reference.digest) {
                    new_blob_digests.insert(reference.digest.clone());
                }
            }
            new_events.push(event);
        }

        durable_events.extend(new_events.iter().cloned());
        validate_stream_for_manifest(&durable_events, &manifest)?;

        if existing.is_none() {
            create_manifest(&paths, &manifest)?;
        }
        for digest in &new_blob_digests {
            let attachment = attachments.get(digest).ok_or_else(|| {
                TraceJournalError::PolicyViolation(format!("missing decoded attachment {digest}"))
            })?;
            store_blob(&paths.blobs, attachment)?;
            known_blobs.insert(digest.clone());
        }
        // Source digests are synced before their events. If the process dies in
        // between, the pending digest makes a conflicting retry detectable;
        // the identical retry can safely complete the append.
        if !new_source_digests.is_empty() {
            append_source_digests(&paths.source_digests, &new_source_digests)?;
        }
        if !new_events.is_empty() {
            append_events(&paths.events, &new_events)?;
        }

        // Re-read after the durable append so lost acknowledgements deduplicate
        // against synced records instead of appending another sequence.
        let recovered = self.recover_run_locked(&paths, true)?;
        // OTel is a best-effort projection of the durable authority. Replaying
        // the complete run lets a restarted engine rebuild open span state;
        // the projector deduplicates by canonical sequence and has no failure
        // channel that could alter this durable acknowledgement.
        temper_log::activity::project_agent_activity(&recovered.events);
        Ok(acknowledgement(
            &batch.run_id,
            recovered.summary.last_accepted_seq,
        ))
    }

    fn recover_all_locked(&self) -> Result<TraceRecoveryReport, TraceJournalError> {
        let mut report = TraceRecoveryReport::default();
        for directory in run_directories(&self.inner.runs_root)? {
            let paths = paths_for_directory(directory.clone());
            match self.recover_run_locked_detailed(&paths, true) {
                Ok((_, truncated)) => {
                    report.recovered_runs += 1;
                    report.truncated_final_fragments += u64::from(truncated);
                }
                Err(error) => report.failures.push(TraceRecoveryFailure {
                    run_directory: directory.display().to_string(),
                    error: error.to_string(),
                }),
            }
        }
        Ok(report)
    }

    fn recover_run_locked(
        &self,
        paths: &RunPaths,
        rewrite_summary: bool,
    ) -> Result<RecoveredRun, TraceJournalError> {
        self.recover_run_locked_detailed(paths, rewrite_summary)
            .map(|(run, _)| run)
    }

    fn recover_run_locked_detailed(
        &self,
        paths: &RunPaths,
        rewrite_summary: bool,
    ) -> Result<(RecoveredRun, bool), TraceJournalError> {
        ensure_existing_secure_directory(&paths.directory)?;
        ensure_existing_secure_directory(&paths.blobs)?;
        ensure_private_regular_file(&paths.manifest)?;
        let manifest: AgentTraceManifest = read_json(&paths.manifest)?;
        validate_manifest(&manifest)?;
        if self.paths(&manifest.run_id).directory != paths.directory {
            return Err(TraceJournalError::CorruptRun(
                "manifest is stored under the wrong content-addressed directory".to_string(),
            ));
        }

        let (events, truncated) = read_events_recovering_final_fragment(&paths.events)?;
        let source_digests = read_source_digests(&paths.source_digests)?;
        if paths.source_digests.exists()
            && events
                .iter()
                .any(|event| !source_digests.contains_key(&event.seq))
        {
            return Err(TraceJournalError::CorruptRun(
                "source digest index is missing a durable event".to_string(),
            ));
        }
        validate_stream_for_manifest(&events, &manifest)?;
        let attachments = load_referenced_blobs(&paths.blobs, &events)?;
        let summary = build_summary(paths, &manifest, &events)?;
        if rewrite_summary {
            write_atomic_json(&paths.summary, &summary)?;
        }
        Ok((
            RecoveredRun {
                manifest,
                events,
                attachments,
                source_digests,
                summary,
            },
            truncated,
        ))
    }

    fn cleanup_retention_locked(
        &self,
        protection: &RetentionProtection,
    ) -> Result<RetentionReport, TraceJournalError> {
        let mut report = RetentionReport::default();
        let now = (self.inner.clock)();
        for directory in run_directories(&self.inner.runs_root)? {
            report.examined += 1;
            let paths = paths_for_directory(directory.clone());
            let recovered = match self.recover_run_locked(&paths, true) {
                Ok(run) => run,
                Err(error) => {
                    report.failures.push(TraceRecoveryFailure {
                        run_directory: directory.display().to_string(),
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            if recovered.summary.status == AgentTraceRunStatus::Active {
                report.preserved_incomplete += 1;
                continue;
            }
            if protection.run_ids.contains(&recovered.manifest.run_id)
                || protection
                    .assignment_ids
                    .contains(&recovered.manifest.assignment_id)
                || protection
                    .job_ids
                    .contains(&recovered.manifest.assignment.job_id)
            {
                report.preserved_in_flight += 1;
                continue;
            }
            let Some(completed_at) = recovered.summary.completed_at.as_deref() else {
                report.preserved_incomplete += 1;
                continue;
            };
            let completed_at = match DateTime::parse_from_rfc3339(completed_at) {
                Ok(completed_at) => completed_at.with_timezone(&Utc),
                Err(error) => {
                    report.failures.push(TraceRecoveryFailure {
                        run_directory: directory.display().to_string(),
                        error: format!("invalid summary completion time: {error}"),
                    });
                    continue;
                }
            };
            let age = now.signed_duration_since(completed_at);
            if age
                < chrono::Duration::days(i64::from(
                    recovered.manifest.capture_policy.retention_days,
                ))
            {
                continue;
            }
            let metadata = match fs::symlink_metadata(&directory) {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.failures.push(TraceRecoveryFailure {
                        run_directory: directory.display().to_string(),
                        error: format!("inspect retention target: {error}"),
                    });
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                report.failures.push(TraceRecoveryFailure {
                    run_directory: directory.display().to_string(),
                    error: "retention target is not a regular run directory".to_string(),
                });
                continue;
            }
            if let Err(error) = fs::remove_dir_all(&directory) {
                report.failures.push(TraceRecoveryFailure {
                    run_directory: directory.display().to_string(),
                    error: format!("remove retained run: {error}"),
                });
                continue;
            }
            if let Err(error) = sync_directory(&self.inner.runs_root) {
                report.failures.push(TraceRecoveryFailure {
                    run_directory: directory.display().to_string(),
                    error: format!("sync retained runs root: {error}"),
                });
                continue;
            }
            report.removed += 1;
        }
        Ok(report)
    }

    fn audit_conflict(
        &self,
        binding: &AuthenticatedWorkerBinding,
        run_id: &str,
        seq: u64,
    ) -> Result<(), TraceJournalError> {
        let record = TraceAuditRecord {
            format_version: JOURNAL_FORMAT_VERSION,
            occurred_at: timestamp((self.inner.clock)()),
            kind: "conflicting_retransmit".to_string(),
            run_id: run_id.to_string(),
            worker_id: binding.worker_id.clone(),
            assignment_id: binding.assignment_id.clone(),
            seq,
        };
        let mut line = serde_json::to_vec(&record)
            .map_err(|error| TraceJournalError::Serialization(error.to_string()))?;
        line.push(b'\n');
        let path = self.inner.root.join("audit.jsonl");
        let mut file = open_private_file(&path, true, true)?;
        file.write_all(&line)
            .map_err(|error| io_error("append trace audit record", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync trace audit log", error))?;
        Ok(())
    }

    fn paths(&self, run_id: &str) -> RunPaths {
        paths_for_directory(self.inner.runs_root.join(run_directory_name(run_id)))
    }

    fn with_store_lock<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, TraceJournalError>,
    ) -> Result<T, TraceJournalError> {
        let _process_guard = self
            .inner
            .process_lock
            .lock()
            .map_err(|_| TraceJournalError::LockPoisoned)?;
        self.inner
            .lock_file
            .lock_exclusive()
            .map_err(|error| io_error("lock trace journal", error))?;
        let result = operation(self);
        let unlock = FileExt::unlock(&self.inner.lock_file)
            .map_err(|error| io_error("unlock trace journal", error));
        match (result, unlock) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }
}
