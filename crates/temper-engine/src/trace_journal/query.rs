impl AgentTraceJournal {
    /// Returns one fully revalidated run, including canonical events.
    ///
    /// Blob paths, file types, digests, and the stream's sequence invariants
    /// are checked while the journal lock is held.
    pub fn run(&self, run_id: &str) -> Result<Option<AgentTraceRun>, TraceJournalError> {
        self.with_store_lock(|journal| {
            let paths = journal.paths(run_id);
            if !paths.manifest.exists() {
                return Ok(None);
            }
            let recovered = journal.recover_run_locked(&paths, true)?;
            Ok(Some(AgentTraceRun {
                manifest: recovered.manifest,
                summary: recovered.summary,
                events: recovered.events,
                attachments: recovered.attachments,
            }))
        })
    }

    /// Returns every fully revalidated run. The caller supplies presentation
    /// ordering; content-addressed directory order is intentionally internal.
    pub fn runs(&self) -> Result<Vec<AgentTraceRun>, TraceJournalError> {
        self.with_store_lock(|journal| {
            let mut runs = Vec::new();
            for directory in run_directories(&journal.inner.runs_root)? {
                let recovered =
                    journal.recover_run_locked(&paths_for_directory(directory), true)?;
                runs.push(AgentTraceRun {
                    manifest: recovered.manifest,
                    summary: recovered.summary,
                    events: recovered.events,
                    attachments: recovered.attachments,
                });
            }
            Ok(runs)
        })
    }
}
