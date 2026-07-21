use super::*;

#[test]
fn normalized_stash_conflict_plan_restores_canonical_checkout_and_all_local_work() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "executable-stash-plan");
        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");

        fs::write(
            workspace.path().join("local-one.txt"),
            "first local commit\n",
        )
        .unwrap();
        let first_commit = workspace.commit_all("first local commit").await.unwrap();
        fs::write(workspace.path().join("tracked.txt"), "tracked base\n").unwrap();
        fs::write(
            workspace.path().join("local-two.txt"),
            "second local commit\n",
        )
        .unwrap();
        let second_commit = workspace.commit_all("second local commit").await.unwrap();

        fs::write(workspace.path().join("README.md"), "local README work\n").unwrap();
        fs::write(
            workspace.path().join("tracked.txt"),
            "unstaged tracked work\n",
        )
        .unwrap();
        fs::write(workspace.path().join("staged.txt"), "staged local work\n").unwrap();
        git(["-C", path_str(workspace.path()), "add", "staged.txt"]);
        fs::write(
            workspace.path().join("untracked.txt"),
            "untracked local work\n",
        )
        .unwrap();

        let target = advance_remote_branch(
            &origin,
            temp.path(),
            "main",
            "README.md",
            "advanced remote README\n",
        );
        let outcome = workspace
            .prepare("agent/work")
            .await
            .expect("stash conflict is quarantined");
        let manifest = match outcome {
            PreparationOutcome::Quarantined(manifest) => manifest,
            other => panic!("expected quarantine, got {other:?}"),
        };

        assert_eq!(manifest.failure_phase, "restore-worktree");
        assert_eq!(manifest.target_sha.as_deref(), Some(target.as_str()));
        assert_eq!(manifest.expected_branch, "agent/work");
        assert_eq!(manifest.replay_commits, [first_commit, second_commit]);
        assert!(
            manifest
                .recovery_notes
                .iter()
                .any(|note| note.contains("Never choose ours or theirs automatically"))
        );
        let quarantine = Path::new(&manifest.quarantine_path);
        assert_eq!(
            git_output(["-C", path_str(quarantine), "status", "--porcelain=v1",]),
            "?? temper-recovery.json",
            "normalization may leave only the subsequently written recovery manifest"
        );
        git([
            "-C",
            path_str(quarantine),
            "merge-base",
            "--is-ancestor",
            target.as_str(),
            "HEAD",
        ]);
        let replay_range = format!("{target}..HEAD");
        assert_eq!(
            git_output([
                "-C",
                path_str(quarantine),
                "rev-list",
                "--count",
                replay_range.as_str(),
            ]),
            "2"
        );

        for command in manifest.recovery_commands.iter().filter(|command| {
            command.contains(" checkout -B ")
                || command.contains(" reset --hard ")
                || command.contains(" cherry-pick ")
        }) {
            assert_shell_success(command);
        }
        let apply = manifest
            .recovery_commands
            .iter()
            .find(|command| command.contains(" stash apply --index "))
            .expect("stash apply command");
        let apply_output = shell_output(apply);
        assert!(
            !apply_output.status.success(),
            "fixture must recreate a real stash conflict"
        );
        assert!(
            !git_output([
                "-C",
                path_str(quarantine),
                "diff",
                "--name-only",
                "--diff-filter=U",
            ])
            .is_empty()
        );

        let completion = manifest
            .recovery_commands
            .iter()
            .find(|command| command.contains("mv --"))
            .expect("guarded completion command");
        assert!(
            !shell_output(completion).status.success(),
            "completion must reject the unmerged index"
        );
        assert!(!workspace.path().exists());
        assert!(quarantine.join("temper-recovery.json").exists());

        fs::write(
            quarantine.join("README.md"),
            "advanced remote README\nresolved local README work\n",
        )
        .unwrap();
        git(["-C", path_str(quarantine), "add", "README.md"]);
        assert_shell_success(completion);

        assert!(workspace.path().exists());
        assert!(!quarantine.exists());
        assert!(
            !workspace.path().join("temper-recovery.json").exists(),
            "manifest is removed only from the successfully restored checkout"
        );
        assert_local_recovery_contents(workspace.path());
        for reference in &manifest.recovery_refs {
            assert_is_sha(&git_output([
                "-C",
                path_str(workspace.path()),
                "rev-parse",
                reference,
            ]));
        }

        let prepared = workspace
            .prepare("agent/work")
            .await
            .expect("restored checkout prepares again");
        assert!(matches!(
            prepared,
            PreparationOutcome::RecoveredLocalWork { .. }
        ));
        assert_local_recovery_contents(workspace.path());
        for reference in &manifest.recovery_refs {
            assert_is_sha(&git_output([
                "-C",
                path_str(workspace.path()),
                "rev-parse",
                reference,
            ]));
        }
        assert!(!quarantine.exists());
    });
}

fn assert_local_recovery_contents(checkout: &Path) {
    assert_eq!(
        fs::read_to_string(checkout.join("README.md")).unwrap(),
        "advanced remote README\nresolved local README work\n"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("local-one.txt")).unwrap(),
        "first local commit\n"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("local-two.txt")).unwrap(),
        "second local commit\n"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("tracked.txt")).unwrap(),
        "unstaged tracked work\n"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("staged.txt")).unwrap(),
        "staged local work\n"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("untracked.txt")).unwrap(),
        "untracked local work\n"
    );
}
