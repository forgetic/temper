use std::path::Path;

pub(super) struct RecoveryPlanRequest<'a> {
    pub failure_phase: &'a str,
    pub checkout_path: &'a Path,
    pub quarantine_path: &'a Path,
    pub expected_branch: &'a str,
    pub git_user: &'a str,
    pub git_email: &'a str,
    pub target_sha: Option<&'a str>,
    pub recovery_refs: &'a [String],
    pub stash_ref: Option<&'a str>,
    pub replay_commits: &'a [String],
}

pub(super) struct RecoveryPlan {
    pub commands: Vec<String>,
    pub notes: Vec<String>,
}

pub(super) fn render(request: RecoveryPlanRequest<'_>) -> RecoveryPlan {
    let quarantine = shell_quote(&request.quarantine_path.display().to_string());
    let mut commands = inspection_commands(&quarantine, request.recovery_refs);
    let mut notes = vec![
        "Preserved recovery refs are immutable evidence; this plan never deletes them.".to_string(),
    ];

    match request.failure_phase {
        "replay-commits" | "restore-worktree" => {
            let Some(target_sha) = request.target_sha else {
                notes.push(
                    "The target commit is unknown, so recovery is inspection-only; do not mutate the quarantine or apply the preserved stash."
                        .to_string(),
                );
                return RecoveryPlan { commands, notes };
            };
            commands.push(format!(
                "git -C {quarantine} checkout -B {} {}",
                shell_quote(request.expected_branch),
                shell_quote(target_sha)
            ));
            commands.push(format!(
                "git -C {quarantine} reset --hard {}",
                shell_quote(target_sha)
            ));
            commands.extend(request.replay_commits.iter().map(|commit| {
                format!(
                    "git -C {quarantine} -c {} -c {} cherry-pick {}",
                    shell_quote(&format!("user.name={}", request.git_user)),
                    shell_quote(&format!("user.email={}", request.git_email)),
                    shell_quote(commit)
                )
            }));
            if let Some(stash_ref) = request.stash_ref {
                commands.push(format!(
                    "git -C {quarantine} stash apply --index {}",
                    shell_quote(stash_ref)
                ));
            }
            notes.push(format!(
                "Reset `{}` to exact target `{target_sha}`, then replay the recorded commits in manifest order before restoring the preserved worktree.",
                request.expected_branch
            ));
            if request.failure_phase == "replay-commits" {
                notes.push(
                    "The worker aborted the failed cherry-pick and verified a clean, operation-free index before quarantine. If replay conflicts again, stop and inspect it deliberately."
                        .to_string(),
                );
            } else {
                notes.push(
                    "The worker reset the failed stash apply to its captured pre-apply HEAD, removed only non-ignored untracked paths represented by the preserved stash, and verified the normalized state."
                        .to_string(),
                );
            }
            notes.push(conflict_note());
            add_completion(&mut commands, &mut notes, &request);
        }
        "verify" => {
            notes.push(
                "Local work is already present because verification failed; inspect it in place and do not apply the preserved stash again."
                    .to_string(),
            );
            notes.push(conflict_note());
            add_completion(&mut commands, &mut notes, &request);
        }
        "preserve" => notes.push(
            "Preservation was not proven complete, so recovery is inspection-only; do not apply a stash or run automated mutations."
                .to_string(),
        ),
        "inspect-operation" => notes.push(
            "An unresolved Git operation was present before quarantine, so recovery is inspection-only; inspect and resolve that operation without applying a stash blindly."
                .to_string(),
        ),
        "replay-commits-ambiguous" => notes.push(
            "Cherry-pick abort or normalization verification failed; the current Git operation/index state is ambiguous and recovery is inspection-only."
                .to_string(),
        ),
        "restore-worktree-ambiguous" => notes.push(
            "Reset, scoped untracked-path cleanup, or normalization verification failed; the current Git state is ambiguous and recovery is inspection-only."
                .to_string(),
        ),
        "inspect-branch" | "inspect-read-only" | "read-only-local-commits"
        | "checkout-anchor" => notes.push(format!(
            "Phase `{}` is not safe for automatic mutation; inspect the recorded branch, target, status, and preserved refs without applying a stash blindly.",
            request.failure_phase
        )),
        unknown => notes.push(format!(
            "Unknown recovery phase `{unknown}` fails closed: inspect only and do not apply a stash or run completion mutations."
        )),
    }

    RecoveryPlan { commands, notes }
}

fn inspection_commands(quarantine: &str, refs: &[String]) -> Vec<String> {
    let mut commands = vec![
        format!("git -C {quarantine} status --short --branch"),
        format!("git -C {quarantine} diff --cc"),
        format!("git -C {quarantine} diff --cached"),
    ];
    commands.extend(
        refs.iter()
            .map(|reference| format!("git -C {quarantine} show {}", shell_quote(reference))),
    );
    commands
}

fn add_completion(
    commands: &mut Vec<String>,
    notes: &mut Vec<String>,
    request: &RecoveryPlanRequest<'_>,
) {
    commands.push(completion_command(
        request.quarantine_path,
        request.checkout_path,
    ));
    notes.push(
        "After all conflicts are resolved and staged deliberately, run the guarded completion command. It refuses unmerged paths, active Git operations, and an existing canonical checkout; it removes temper-recovery.json only after the quarantine move succeeds."
            .to_string(),
    );
    notes.push(
        "Only after canonical-path restoration succeeds, requeue the Forge artifact by removing `needs-human` and restoring its queue label; the worker intentionally emits no Forge mutation command."
            .to_string(),
    );
}

fn conflict_note() -> String {
    "If replay or stash application conflicts, inspect `git status --short --branch`, `git diff --cc`, and `git diff --cached`; resolve each path deliberately and stage it with `git add`. For a cherry-pick conflict, run `git cherry-pick --continue` only after that deliberate resolution, then resume the recorded command order. Never choose ours or theirs automatically."
        .to_string()
}

fn completion_command(quarantine_path: &Path, checkout_path: &Path) -> String {
    let quarantine = shell_quote(&quarantine_path.display().to_string());
    let checkout = shell_quote(&checkout_path.display().to_string());
    let manifest = shell_quote(
        &checkout_path
            .join("temper-recovery.json")
            .display()
            .to_string(),
    );
    format!(
        "git_dir=$(git -C {quarantine} rev-parse --absolute-git-dir) && test -z \"$(git -C {quarantine} diff --name-only --diff-filter=U)\" && test ! -e \"$git_dir/MERGE_HEAD\" && test ! -e \"$git_dir/rebase-merge\" && test ! -e \"$git_dir/rebase-apply\" && test ! -e \"$git_dir/CHERRY_PICK_HEAD\" && test ! -e \"$git_dir/REVERT_HEAD\" && test ! -e \"$git_dir/BISECT_LOG\" && test ! -e \"$git_dir/sequencer\" && test ! -e {checkout} && test ! -L {checkout} && mv -- {quarantine} {checkout} && rm -- {manifest}"
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::workspace::QuarantineManifest;

    const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
    const STASH: &str = "refs/temper/recovery/job/worktree-stash";

    #[test]
    fn every_phase_is_fail_closed_or_gets_only_its_safe_mutations() {
        let cases = [
            ("replay-commits", true, true),
            ("restore-worktree", true, true),
            ("verify", false, true),
            ("preserve", false, false),
            ("inspect-operation", false, false),
            ("inspect-branch", false, false),
            ("checkout-anchor", false, false),
            ("inspect-read-only", false, false),
            ("read-only-local-commits", false, false),
            ("replay-commits-ambiguous", false, false),
            ("restore-worktree-ambiguous", false, false),
            ("future-unknown-phase", false, false),
        ];

        for (phase, applies_stash, completes) in cases {
            let plan = plan(phase, Some(TARGET));
            assert_eq!(
                plan.commands
                    .iter()
                    .any(|command| command.contains("stash apply")),
                applies_stash,
                "stash policy for {phase}: {:?}",
                plan.commands
            );
            assert_eq!(
                plan.commands
                    .iter()
                    .any(|command| command.contains("mv --")),
                completes,
                "completion policy for {phase}: {:?}",
                plan.commands
            );
        }
    }

    #[test]
    fn normalized_target_plan_quotes_target_and_replays_exact_order() {
        let plan = render(RecoveryPlanRequest {
            failure_phase: "restore-worktree",
            checkout_path: Path::new("/tmp/canonical checkout"),
            quarantine_path: Path::new("/tmp/quarantine's checkout"),
            expected_branch: "feature/operator's-fix",
            git_user: "Recovery Operator",
            git_email: "recovery@example.test",
            target_sha: Some(TARGET),
            recovery_refs: &["refs/temper/recovery/ref's-head".to_string()],
            stash_ref: Some(STASH),
            replay_commits: &["commit-one".to_string(), "commit-two".to_string()],
        });
        let joined = plan.commands.join("\n");

        assert!(joined.contains("'/tmp/quarantine'\\''s checkout'"));
        assert!(joined.contains("'feature/operator'\\''s-fix'"));
        assert!(joined.contains(&shell_quote(TARGET)));
        let first = joined.find("cherry-pick 'commit-one'").unwrap();
        let second = joined.find("cherry-pick 'commit-two'").unwrap();
        let apply = joined.find("stash apply --index").unwrap();
        assert!(first < second && second < apply);
        let first_mutation = plan
            .commands
            .iter()
            .find(|command| is_mutation(command))
            .expect("mutation");
        assert!(first_mutation.contains("checkout -B"));
    }

    #[test]
    fn verify_never_reapplies_or_resets_local_work() {
        let plan = plan("verify", Some(TARGET));
        let joined = plan.commands.join("\n");
        assert!(!joined.contains("stash apply"));
        assert!(!joined.contains("reset --hard"));
        assert!(!joined.contains("cherry-pick"));
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("already present"))
        );
    }

    #[test]
    fn targetless_and_unknown_plans_are_inspection_only() {
        for (phase, target) in [("restore-worktree", None), ("unknown", Some(TARGET))] {
            let plan = plan(phase, target);
            assert!(plan.commands.iter().all(|command| !is_mutation(command)));
            assert!(
                !plan
                    .commands
                    .iter()
                    .any(|command| command.contains("stash apply"))
            );
        }
    }

    #[test]
    fn completion_is_guarded_and_never_deletes_recovery_refs() {
        let plan = plan("restore-worktree", Some(TARGET));
        let completion = plan
            .commands
            .iter()
            .find(|command| command.contains("mv --"))
            .expect("completion command");

        for guard in [
            "--diff-filter=U",
            "MERGE_HEAD",
            "rebase-merge",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "sequencer",
            "test ! -e '/tmp/canonical'",
            "test ! -L '/tmp/canonical'",
        ] {
            assert!(
                completion.contains(guard),
                "missing guard {guard}: {completion}"
            );
        }
        assert!(completion.find("mv --").unwrap() < completion.find("rm --").unwrap());
        for command in &plan.commands {
            assert!(!command.contains("update-ref -d"));
            assert!(!command.contains("branch -D"));
            assert!(!(command.contains("rm --") && command.contains(STASH)));
        }
    }

    #[test]
    fn old_manifest_without_new_fields_deserializes() {
        let manifest: QuarantineManifest = serde_json::from_str(
            r#"{
                "job_id":"job/1","correlation_key":"work","repository":"ai/temper",
                "checkout_path":"/tmp/canonical","quarantine_path":"/tmp/quarantine",
                "original_branch":"agent/work","expected_branch":"agent/work",
                "original_head":"0123","target_sha":"4567","original_status_paths":[],
                "recovery_refs":[],"failure_phase":"verify","failure":"old failure",
                "recovery_commands":[]
            }"#,
        )
        .expect("legacy manifest");

        assert!(manifest.replay_commits.is_empty());
        assert!(manifest.recovery_notes.is_empty());
    }

    fn plan(phase: &str, target_sha: Option<&str>) -> RecoveryPlan {
        render(RecoveryPlanRequest {
            failure_phase: phase,
            checkout_path: Path::new("/tmp/canonical"),
            quarantine_path: Path::new("/tmp/quarantine"),
            expected_branch: "agent/work",
            git_user: "Recovery Operator",
            git_email: "recovery@example.test",
            target_sha,
            recovery_refs: &["refs/temper/recovery/head".to_string(), STASH.to_string()],
            stash_ref: Some(STASH),
            replay_commits: &["one".to_string(), "two".to_string()],
        })
    }

    fn is_mutation(command: &str) -> bool {
        [
            " checkout ",
            " reset ",
            " cherry-pick ",
            " stash apply ",
            "mv --",
        ]
        .iter()
        .any(|needle| command.contains(needle))
    }
}
