//! Durable assignment claim, heartbeat, release, and recovery mutations.

use super::*;

impl<F: Forge + ?Sized> LeaseManager<'_, F> {
    /// Atomically persists an exact assignment, lease, and lifecycle mutation.
    ///
    /// The body and Forge labels/assignees share one conditional update, so an
    /// observer can never see the assignment publication without the matching
    /// lifecycle projection (or vice versa).
    pub async fn claim_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        mut request: AssignmentClaimRequest,
        now: DateTime<Utc>,
    ) -> Result<DurableAssignment, LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        if let Some(current) = loaded.metadata().assignment.as_ref() {
            if assignment_identity_matches(current, &request.assignment) {
                return Ok(current.clone());
            }
            return Err(LeaseError::AssignmentConflict {
                job_id: current.job_id.clone().unwrap_or_default(),
            });
        }

        let role =
            request
                .assignment
                .role
                .clone()
                .ok_or_else(|| LeaseError::MalformedMetadata {
                    reason: "assignment role is required".to_string(),
                })?;
        let lease_owner = request
            .assignment
            .daemon_boot_id
            .clone()
            .or_else(|| request.assignment.worker_id.clone())
            .ok_or_else(|| LeaseError::MalformedMetadata {
                reason: "assignment daemon_boot_id or worker_id is required".to_string(),
            })?;
        let lease =
            self.planner
                .acquire(loaded.metadata().lease.as_ref(), role, lease_owner, now)?;

        request.assignment.pre_claim_labels = loaded.labels().to_vec();
        request.assignment.pre_claim_assignees = loaded
            .assignees()
            .iter()
            .map(|user| user.as_str().to_string())
            .collect();
        request.assignment.assigned_at = Some(now);
        request.assignment.expires_at = Some(lease.expires_at);
        let metadata = loaded.metadata_with_diagnostic_job(&request.assignment, now)?;
        self.write_assignment_with_metadata(
            &loaded,
            metadata,
            Some(request.assignment.clone()),
            Some(lease),
            request.mutation,
            target,
        )
        .await?;
        Ok(request.assignment)
    }

    /// Validates that fresh Forge metadata names this exact assignment.
    pub async fn validate_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<bool, LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        Ok(loaded
            .metadata()
            .assignment
            .as_ref()
            .is_some_and(|current| assignment_identity_matches(current, expected)))
    }

    /// Clears an exact durable assignment and its lease. A mismatched caller
    /// cannot release another worker or daemon boot's claim.
    pub async fn release_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<(), LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        let Some(current) = loaded.metadata().assignment.as_ref() else {
            return Ok(());
        };
        if !assignment_identity_matches(current, expected) {
            return Err(LeaseError::AssignmentConflict {
                job_id: current.job_id.clone().unwrap_or_default(),
            });
        }
        self.write_assignment(&loaded, None, None, AssignmentMutation::default(), target)
            .await
    }

    /// Clears a successful exact assignment and, when it is the provider-recovery
    /// due attempt, clears that marker in the same Forge CAS.
    pub async fn release_successful_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<(), LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        let Some(current) = loaded.metadata().assignment.as_ref() else {
            return Ok(());
        };
        if !assignment_identity_matches(current, expected) {
            return Err(LeaseError::AssignmentConflict {
                job_id: current.job_id.clone().unwrap_or_default(),
            });
        }
        let mut metadata = loaded.metadata().clone();
        if let Some(recovery) = metadata.provider_recovery.as_ref() {
            recovery
                .validate()
                .map_err(|reason| LeaseError::MalformedMetadata {
                    reason: format!("corrupt provider recovery state: {reason}"),
                })?;
            if expected.coordination_key.as_deref() != Some(recovery.workstream_id.as_str()) {
                return Err(LeaseError::AssignmentConflict {
                    job_id: "provider recovery workstream does not match successful assignment"
                        .to_string(),
                });
            }
            if !recovery.authorizes_attempt(expected.attempt_id.as_deref()) {
                return Err(LeaseError::AssignmentConflict {
                    job_id: "provider recovery does not authorize successful attempt".to_string(),
                });
            }
            metadata.provider_recovery = None;
        }
        self.write_assignment_with_metadata(
            &loaded,
            metadata,
            None,
            None,
            AssignmentMutation::default(),
            target,
        )
        .await
    }

    /// Rolls an unpublished assignment back to its captured pre-claim
    /// lifecycle state while clearing the assignment and lease in one CAS.
    pub async fn rollback_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<(), LeaseError> {
        self.rollback_assignment_matching(repo_id, target, expected, false)
            .await
    }

    /// Rolls back only when the complete durable assignment snapshot still
    /// matches. Unlike [`rollback_assignment`](Self::rollback_assignment), a
    /// renewed expiry is stale and is never cleared by recovery.
    pub async fn rollback_assignment_snapshot(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<(), LeaseError> {
        self.rollback_assignment_matching(repo_id, target, expected, true)
            .await
    }

    async fn rollback_assignment_matching(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        snapshot_match: bool,
    ) -> Result<(), LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        let Some(current) = loaded.metadata().assignment.as_ref() else {
            return Ok(());
        };
        let matches = if snapshot_match {
            current == expected
        } else {
            assignment_identity_matches(current, expected)
        };
        if !matches {
            return Err(LeaseError::AssignmentConflict {
                job_id: current.job_id.clone().unwrap_or_default(),
            });
        }
        let add_labels = current
            .pre_claim_labels
            .iter()
            .filter(|label| !loaded.labels().contains(label))
            .cloned()
            .collect();
        let remove_labels = loaded
            .labels()
            .iter()
            .filter(|label| !current.pre_claim_labels.contains(label))
            .cloned()
            .collect();
        let pre_assignees = current
            .pre_claim_assignees
            .iter()
            .map(|user| UserId::new(user.clone()))
            .collect::<Vec<_>>();
        let add_assignees = pre_assignees
            .iter()
            .filter(|user| !loaded.assignees().contains(user))
            .cloned()
            .collect();
        let remove_assignees = loaded
            .assignees()
            .iter()
            .filter(|user| !pre_assignees.contains(user))
            .cloned()
            .collect();
        // Only the immediate unpublished-claim rollback may reopen an
        // interrupted-CI diagnostic publication boundary. Snapshot rollback is
        // abandoned-assignment convergence: the assignment may already have
        // reached a worker before its lease expired, so retain the durable job
        // id and let recovery park rather than dispatching it again.
        let metadata = if snapshot_match {
            loaded.metadata_with_released_provider_attempt(expected)
        } else {
            loaded.metadata_with_rolled_back_diagnostic(expected)
        };
        self.write_assignment_with_metadata(
            &loaded,
            metadata,
            None,
            None,
            AssignmentMutation {
                add_labels,
                remove_labels,
                add_assignees,
                remove_assignees,
            },
            target,
        )
        .await
    }

    /// Converges an abandoned issue assignment from fresh Forge state.
    ///
    /// Unlike [`rollback_assignment`](Self::rollback_assignment), this does not
    /// restore an assignment-time label snapshot. It preserves unrelated labels
    /// added while the worker ran and projects the issue to `blocked` when fresh
    /// dependency reads remain unresolved, or back to the assignment's queue
    /// labels when they are resolved. Assignment metadata and lease are cleared
    /// in the same conditional update.
    pub async fn converge_issue_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        queue_labels: &[String],
        claim_labels: &[String],
        dependencies_unresolved: bool,
    ) -> Result<(), LeaseError> {
        self.converge_issue_assignment_matching(
            repo_id,
            target,
            expected,
            queue_labels,
            claim_labels,
            dependencies_unresolved,
            false,
        )
        .await
    }

    /// Converges an issue only if the complete assignment snapshot still
    /// matches, fencing a heartbeat or newer assignment observed after scan.
    pub async fn converge_issue_assignment_snapshot(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        queue_labels: &[String],
        claim_labels: &[String],
        dependencies_unresolved: bool,
    ) -> Result<(), LeaseError> {
        self.converge_issue_assignment_matching(
            repo_id,
            target,
            expected,
            queue_labels,
            claim_labels,
            dependencies_unresolved,
            true,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn converge_issue_assignment_matching(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        queue_labels: &[String],
        claim_labels: &[String],
        dependencies_unresolved: bool,
        snapshot_match: bool,
    ) -> Result<(), LeaseError> {
        if !matches!(target, ArtifactSource::Issue { .. }) {
            return if snapshot_match {
                self.rollback_assignment_snapshot(repo_id, target, expected)
                    .await
            } else {
                self.rollback_assignment(repo_id, target, expected).await
            };
        }
        let loaded = self.load(repo_id, target).await?;
        let Some(current) = loaded.metadata().assignment.as_ref() else {
            return Ok(());
        };
        let matches = if snapshot_match {
            current == expected
        } else {
            assignment_identity_matches(current, expected)
        };
        if !matches {
            return Err(LeaseError::AssignmentConflict {
                job_id: current.job_id.clone().unwrap_or_default(),
            });
        }

        let mut add_labels = Vec::new();
        let mut remove_labels = loaded
            .labels()
            .iter()
            .filter(|label| {
                label.as_str() == "in-progress" || claim_labels.iter().any(|added| added == *label)
            })
            .cloned()
            .collect::<Vec<_>>();
        if dependencies_unresolved {
            if !loaded.labels().iter().any(|label| label == "blocked") {
                add_labels.push("blocked".to_string());
            }
            for label in queue_labels {
                if loaded.labels().contains(label) && !remove_labels.contains(label) {
                    remove_labels.push(label.clone());
                }
            }
        } else {
            if loaded.labels().iter().any(|label| label == "blocked") {
                remove_labels.push("blocked".to_string());
            }
            for label in queue_labels {
                if !loaded.labels().contains(label) && !add_labels.contains(label) {
                    add_labels.push(label.clone());
                }
            }
        }

        let pre_assignees = current
            .pre_claim_assignees
            .iter()
            .map(|user| UserId::new(user.clone()))
            .collect::<Vec<_>>();
        let add_assignees = pre_assignees
            .iter()
            .filter(|user| !loaded.assignees().contains(user))
            .cloned()
            .collect();
        let remove_assignees = loaded
            .assignees()
            .iter()
            .filter(|user| !pre_assignees.contains(user))
            .cloned()
            .collect();
        self.write_assignment(
            &loaded,
            None,
            None,
            AssignmentMutation {
                add_labels,
                remove_labels,
                add_assignees,
                remove_assignees,
            },
            target,
        )
        .await
    }

    /// Conditionally refreshes the lease for an exact durable assignment.
    ///
    /// This is the restart-reattachment path. The complete core attempt fence
    /// and all recovered optional identity fields must still match fresh Forge
    /// metadata, as must the lease role and owner. Definitive durable-state
    /// changes are distinguished from temporary backend failures. A lost CAS
    /// is verified with one fresh read and never revokes ownership by itself.
    pub async fn heartbeat_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        now: DateTime<Utc>,
    ) -> RecoveredHeartbeatOutcome {
        let loaded = match self.load(repo_id, target).await {
            Ok(loaded) => loaded,
            Err(error) => return ownership_loss_from_error(error),
        };
        if let Err(reason) = assignment_ownership(&loaded, expected) {
            return RecoveredHeartbeatOutcome::OwnershipLost { reason };
        }

        let lease = loaded
            .metadata()
            .lease
            .as_ref()
            .expect("ownership validation requires a lease");
        let refreshed = match self.planner.heartbeat(Some(lease), &lease.worker, now) {
            Ok(refreshed) => refreshed,
            Err(error) => return ownership_loss_from_error(error.into()),
        };
        let mut assignment = loaded
            .metadata()
            .assignment
            .as_ref()
            .expect("ownership validation requires an assignment")
            .clone();
        assignment.expires_at = Some(refreshed.expires_at);
        match self
            .write_assignment(
                &loaded,
                Some(assignment),
                Some(refreshed),
                AssignmentMutation::default(),
                target,
            )
            .await
        {
            Ok(()) => RecoveredHeartbeatOutcome::Owned,
            Err(LeaseError::Contended { .. }) => match self.load(repo_id, target).await {
                Ok(fresh) => match assignment_ownership(&fresh, expected) {
                    Ok(()) => RecoveredHeartbeatOutcome::TransientlyUnavailable {
                        reason: "lease heartbeat lost a compare-and-swap race, but fresh durable ownership still matches"
                            .to_string(),
                    },
                    Err(reason) => RecoveredHeartbeatOutcome::OwnershipLost { reason },
                },
                Err(error) => ownership_loss_from_error(error),
            },
            Err(error) => ownership_loss_from_error(error),
        }
    }

    /// Clears an impossible durable claim and leaves one idempotent attention
    /// marker instead of guessing a ready/blocked state.
    pub async fn quarantine_assignment(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<(), LeaseError> {
        self.quarantine_assignment_matching(repo_id, target, expected, false)
            .await
    }

    /// Quarantines only the complete assignment snapshot captured by a recovery
    /// finding. A renewed or replaced assignment is a stale conflict.
    pub async fn quarantine_assignment_snapshot(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
    ) -> Result<(), LeaseError> {
        self.quarantine_assignment_matching(repo_id, target, expected, true)
            .await
    }

    async fn quarantine_assignment_matching(
        &self,
        repo_id: &RepositoryId,
        target: ArtifactSource,
        expected: &DurableAssignment,
        snapshot_match: bool,
    ) -> Result<(), LeaseError> {
        let loaded = self.load(repo_id, target).await?;
        let Some(current) = loaded.metadata().assignment.as_ref() else {
            return Ok(());
        };
        let matches = if snapshot_match {
            current == expected
        } else {
            assignment_identity_matches(current, expected)
        };
        if !matches {
            return Err(LeaseError::AssignmentConflict {
                job_id: current.job_id.clone().unwrap_or_default(),
            });
        }
        let pre_assignees = current
            .pre_claim_assignees
            .iter()
            .map(|user| UserId::new(user.clone()))
            .collect::<Vec<_>>();
        let add_labels = (!requires_human_attention(loaded.labels()))
            .then(|| NEEDS_HUMAN_LABEL.to_string())
            .into_iter()
            .collect();
        let remove_labels = loaded
            .labels()
            .iter()
            .filter(|label| label.as_str() == "in-progress")
            .cloned()
            .collect();
        let add_assignees = pre_assignees
            .iter()
            .filter(|user| !loaded.assignees().contains(user))
            .cloned()
            .collect();
        let remove_assignees = loaded
            .assignees()
            .iter()
            .filter(|user| !pre_assignees.contains(user))
            .cloned()
            .collect();
        self.write_assignment(
            &loaded,
            None,
            None,
            AssignmentMutation {
                add_labels,
                remove_labels,
                add_assignees,
                remove_assignees,
            },
            target,
        )
        .await
    }
}
