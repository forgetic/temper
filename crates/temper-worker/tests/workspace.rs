use std::fs;
use std::path::Path;
use std::process::Command;

use temper_worker::{
    PreparationOutcome, RecoveryContext, RoleGitIdentity, Workspace, WorkspaceConfig,
};
use tempfile::tempdir;

#[path = "workspace/recovery_plan.rs"]
mod recovery_plan;

#[test]
fn workspace_prepares_commits_pushes_and_reuses_local_git_checkout() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        let _ = seed_origin(&origin, temp.path());

        let identity = RoleGitIdentity {
            user: "Smith Engineer".to_string(),
            email: "smith-engineer@example.test".to_string(),
            token: "test-token".to_string(),
        };
        let config = WorkspaceConfig {
            root: temp.path().join("workspaces"),
            base_branch: "main".to_string(),
        };
        let workspace =
            Workspace::new(&config, "ai/smith", "engineer", identity, path_str(&origin))
                .expect("workspace config is valid");
        assert_eq!(workspace.path(), config.root.join("ai__smith/engineer"));

        let initial = workspace
            .prepare("smith-worker/work")
            .await
            .expect("prepare workspace");
        assert!(matches!(initial, PreparationOutcome::CleanReuse { .. }));
        assert!(workspace.path().exists());
        assert_eq!(
            git_output(["-C", path_str(workspace.path()), "branch", "--show-current"]),
            "smith-worker/work"
        );
        let head = workspace.head_sha().await.expect("workspace head sha");
        let origin_main =
            git_output(["-C", path_str(workspace.path()), "rev-parse", "origin/main"]);
        assert_eq!(head, origin_main);
        assert!(
            !workspace
                .has_changes()
                .await
                .expect("prepared workspace has no changes"),
            "freshly prepared workspace should be clean"
        );

        fs::write(
            workspace.path().join("worker.txt"),
            "persistent workspace\n",
        )
        .expect("write workspace file");
        assert!(
            workspace
                .has_changes()
                .await
                .expect("workspace detects untracked file"),
            "untracked file should count as a workspace change"
        );
        let commit_sha = workspace
            .commit_all("add file")
            .await
            .expect("commit all changes");
        assert_is_sha(&commit_sha);
        assert!(
            !workspace
                .has_changes()
                .await
                .expect("committed workspace has no changes"),
            "committed workspace should be clean"
        );

        assert_eq!(
            git_output([
                "-C",
                path_str(workspace.path()),
                "log",
                "-1",
                "--format=%an <%ae>|%cn <%ce>",
            ]),
            "Smith Engineer <smith-engineer@example.test>|Smith Engineer <smith-engineer@example.test>"
        );

        let pushed_sha = workspace
            .push_branch("agent/pr-for-code-999")
            .await
            .expect("push branch");
        assert_is_sha(&pushed_sha);
        assert_eq!(pushed_sha, commit_sha);
        assert_eq!(
            git_output([
                "-C",
                path_str(&origin),
                "rev-parse",
                "refs/heads/agent/pr-for-code-999",
            ]),
            commit_sha
        );

        let sentinel = workspace.path().join(".git").join("smith-sentinel");
        fs::write(&sentinel, "keep git object cache").expect("write sentinel under .git");
        workspace
            .prepare("smith-worker/work")
            .await
            .expect("reuse existing workspace");
        assert!(
            sentinel.exists(),
            "prepare must not recreate or wipe checkout"
        );
    });
}

#[test]
fn prepare_pull_request_head_checks_out_the_pull_ref() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        let pull_request_head_sha = seed_origin(&origin, temp.path());

        let identity = RoleGitIdentity {
            user: "Smith Reviewer".to_string(),
            email: "smith-reviewer@example.test".to_string(),
            token: "test-token".to_string(),
        };
        let config = WorkspaceConfig {
            root: temp.path().join("workspaces"),
            base_branch: "main".to_string(),
        };
        let workspace =
            Workspace::new(&config, "ai/smith", "reviewer", identity, path_str(&origin))
                .expect("workspace config is valid");

        workspace
            .prepare_pull_request_head(7, "smith-worker/review-7")
            .await
            .expect("prepare PR-head workspace");
        assert_eq!(
            git_output(["-C", path_str(workspace.path()), "branch", "--show-current"]),
            "smith-worker/review-7"
        );
        assert_eq!(
            workspace.head_sha().await.expect("workspace head sha"),
            pull_request_head_sha
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(workspace.path()),
                "rev-parse",
                "refs/temper/pr/7/head",
            ]),
            pull_request_head_sha
        );

        let sentinel = workspace.path().join(".git").join("smith-pr-sentinel");
        fs::write(&sentinel, "keep git object cache").expect("write sentinel under .git");
        workspace
            .prepare_pull_request_head(7, "smith-worker/review-7")
            .await
            .expect("reuse existing PR-head workspace");
        assert!(
            sentinel.exists(),
            "prepare_pull_request_head must not recreate or wipe checkout"
        );
        assert_eq!(
            workspace.head_sha().await.expect("workspace head sha"),
            pull_request_head_sha
        );
    });
}

#[test]
fn dirty_staged_unstaged_and_untracked_work_is_restored_with_immutable_refs() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "dirty-recovery");

        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");
        fs::write(workspace.path().join("README.md"), "staged version\n").expect("tracked edit");
        git(["-C", path_str(workspace.path()), "add", "README.md"]);
        fs::write(workspace.path().join("README.md"), "unstaged version\n").expect("unstaged edit");
        fs::write(
            workspace.path().join("untracked.txt"),
            "untracked payload\n",
        )
        .expect("untracked edit");

        let outcome = workspace
            .prepare("agent/work")
            .await
            .expect("recover workspace");
        let refs = match outcome {
            PreparationOutcome::RecoveredLocalWork { recovery_refs, .. } => recovery_refs,
            other => panic!("expected recovered local work, got {other:?}"),
        };
        assert_eq!(
            fs::read_to_string(workspace.path().join("README.md")).unwrap(),
            "unstaged version\n"
        );
        assert_eq!(
            fs::read_to_string(workspace.path().join("untracked.txt")).unwrap(),
            "untracked payload\n"
        );
        assert_eq!(
            git_output(["-C", path_str(workspace.path()), "show", ":README.md"]),
            "staged version"
        );
        assert_eq!(refs.len(), 2, "HEAD and worktree refs are retained");
        for reference in refs {
            assert_is_sha(&git_output([
                "-C",
                path_str(workspace.path()),
                "rev-parse",
                reference.as_str(),
            ]));
        }
    });
}

#[test]
fn local_commits_replay_onto_an_advanced_base() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "advanced-base");
        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");
        fs::write(workspace.path().join("local.txt"), "local commit\n").unwrap();
        workspace.commit_all("local-only commit").await.unwrap();

        let advanced = advance_remote_branch(
            &origin,
            temp.path(),
            "main",
            "base-advanced.txt",
            "advanced base\n",
        );
        let outcome = workspace
            .prepare("agent/work")
            .await
            .expect("replay local commit");
        assert!(matches!(
            outcome,
            PreparationOutcome::RecoveredLocalWork { .. }
        ));
        assert!(workspace.path().join("local.txt").exists());
        assert!(workspace.path().join("base-advanced.txt").exists());
        assert_eq!(
            git_output([
                "-C",
                path_str(workspace.path()),
                "merge-base",
                "--is-ancestor",
                advanced.as_str(),
                "HEAD",
            ]),
            ""
        );
    });
}

#[test]
fn local_commits_replay_onto_an_advanced_remote_work_branch() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "advanced-work-branch");
        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");
        fs::write(workspace.path().join("local.txt"), "local commit\n").unwrap();
        workspace
            .commit_all("interrupted local commit")
            .await
            .unwrap();

        let remote_head = advance_remote_branch(
            &origin,
            temp.path(),
            "agent/work",
            "remote-work.txt",
            "remote work\n",
        );
        let outcome = workspace
            .prepare("agent/work")
            .await
            .expect("replay over remote work");
        assert!(matches!(
            outcome,
            PreparationOutcome::RecoveredLocalWork { .. }
        ));
        assert!(workspace.path().join("local.txt").exists());
        assert!(workspace.path().join("remote-work.txt").exists());
        git([
            "-C",
            path_str(workspace.path()),
            "merge-base",
            "--is-ancestor",
            remote_head.as_str(),
            "HEAD",
        ]);
    });
}

#[test]
fn replay_conflict_quarantines_and_manifest_recovers_original_commit() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "conflict");
        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");
        fs::write(workspace.path().join("README.md"), "local side\n").unwrap();
        let original = workspace.commit_all("local conflict").await.unwrap();
        advance_remote_branch(&origin, temp.path(), "main", "README.md", "remote side\n");

        let first = workspace
            .prepare("agent/work")
            .await
            .expect("quarantine conflict");
        let manifest = match first {
            PreparationOutcome::Quarantined(manifest) => manifest,
            other => panic!("expected quarantine, got {other:?}"),
        };
        assert_eq!(manifest.failure_phase, "replay-commits");
        assert_eq!(manifest.original_head.as_deref(), Some(original.as_str()));
        let quarantine = Path::new(&manifest.quarantine_path);
        assert!(quarantine.join("temper-recovery.json").exists());
        assert_eq!(
            git_output([
                "-C",
                path_str(quarantine),
                "show",
                &format!("{}:README.md", manifest.recovery_refs[0]),
            ]),
            "local side"
        );

        let second = workspace
            .prepare("agent/work")
            .await
            .expect("idempotent quarantine");
        assert_eq!(second, PreparationOutcome::Quarantined(manifest));
        assert!(
            !workspace.path().exists(),
            "retry must not clone another checkout"
        );
    });
}

#[test]
fn unexpected_branch_and_read_only_edits_are_quarantined() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "unexpected-branch");
        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");
        git([
            "-C",
            path_str(workspace.path()),
            "checkout",
            "-b",
            "surprise",
        ]);
        fs::write(workspace.path().join("keep.txt"), "keep me\n").unwrap();

        let outcome = workspace
            .prepare("agent/work")
            .await
            .expect("quarantine branch");
        let manifest = match outcome {
            PreparationOutcome::Quarantined(manifest) => manifest,
            other => panic!("expected quarantine, got {other:?}"),
        };
        assert_eq!(manifest.failure_phase, "inspect-branch");
        let quarantine = Path::new(&manifest.quarantine_path);
        let worktree_ref = manifest
            .recovery_refs
            .iter()
            .find(|reference| reference.contains("/worktree-"))
            .expect("stable worktree ref");
        git([
            "-C",
            path_str(quarantine),
            "stash",
            "apply",
            "--index",
            worktree_ref,
        ]);
        assert!(quarantine.join("keep.txt").exists());

        let read_only = recovery_workspace(temp.path(), &origin, "read-only-dirty");
        read_only
            .prepare_read_only()
            .await
            .expect("initial read-only prepare");
        fs::write(read_only.path().join("review-note.txt"), "do not discard\n").unwrap();
        let outcome = read_only
            .prepare_read_only()
            .await
            .expect("quarantine read-only");
        let manifest = match outcome {
            PreparationOutcome::Quarantined(manifest) => manifest,
            other => panic!("expected read-only quarantine, got {other:?}"),
        };
        assert_eq!(manifest.failure_phase, "inspect-read-only");
        let quarantine = Path::new(&manifest.quarantine_path);
        let worktree_ref = manifest
            .recovery_refs
            .iter()
            .find(|reference| reference.contains("/worktree-"))
            .expect("stable worktree ref");
        git([
            "-C",
            path_str(quarantine),
            "stash",
            "apply",
            "--index",
            worktree_ref,
        ]);
        assert!(quarantine.join("review-note.txt").exists());
    });
}

#[test]
fn unresolved_operation_is_quarantined_before_checkout() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let origin = temp.path().join("origin.git");
        git(["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, temp.path());
        let workspace = recovery_workspace(temp.path(), &origin, "merge-state");
        workspace
            .prepare("agent/work")
            .await
            .expect("initial prepare");
        let head = workspace.head_sha().await.unwrap();
        fs::write(
            workspace.path().join(".git/MERGE_HEAD"),
            format!("{head}\n"),
        )
        .unwrap();

        let outcome = workspace
            .prepare("agent/work")
            .await
            .expect("quarantine merge");
        let manifest = match outcome {
            PreparationOutcome::Quarantined(manifest) => manifest,
            other => panic!("expected quarantine, got {other:?}"),
        };
        assert_eq!(manifest.failure_phase, "inspect-operation");
        assert!(manifest.failure.contains("merge"));
        assert_eq!(manifest.recovery_refs.len(), 1);
    });
}

fn recovery_workspace(temp: &Path, origin: &Path, correlation: &str) -> Workspace {
    let config = WorkspaceConfig {
        root: temp.join("workspaces").join(correlation),
        base_branch: "main".to_string(),
    };
    Workspace::new(
        &config,
        "ai/smith",
        "engineer",
        RoleGitIdentity {
            user: "Smith Engineer".to_string(),
            email: "smith-engineer@example.test".to_string(),
            token: "test-token".to_string(),
        },
        path_str(origin),
    )
    .expect("workspace")
    .with_recovery_context(RecoveryContext {
        job_id: format!("job/{correlation}"),
        correlation_key: correlation.to_string(),
        repository: "ai/smith".to_string(),
    })
}

fn advance_remote_branch(
    origin: &Path,
    temp: &Path,
    branch: &str,
    file: &str,
    contents: &str,
) -> String {
    let clone = temp.join(format!("advance-{}", file.replace('/', "-")));
    git(["clone", path_str(origin), path_str(&clone)]);
    git([
        "-C",
        path_str(&clone),
        "checkout",
        "-B",
        branch,
        "origin/main",
    ]);
    fs::write(clone.join(file), contents).expect("write remote change");
    git([
        "-C",
        path_str(&clone),
        "-c",
        "user.name=Remote User",
        "-c",
        "user.email=remote@example.test",
        "add",
        file,
    ]);
    git([
        "-C",
        path_str(&clone),
        "-c",
        "user.name=Remote User",
        "-c",
        "user.email=remote@example.test",
        "commit",
        "-m",
        "advance remote",
    ]);
    let sha = git_output(["-C", path_str(&clone), "rev-parse", "HEAD"]);
    git(["-C", path_str(&clone), "push", "origin", branch]);
    sha
}

fn seed_origin(origin: &Path, temp: &Path) -> String {
    let seed = temp.join("seed");
    git(["init", "-b", "main", path_str(&seed)]);
    fs::write(seed.join("README.md"), "# seed\n").expect("write seed file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "README.md",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial commit",
    ]);
    git([
        "-C",
        path_str(&seed),
        "remote",
        "add",
        "origin",
        path_str(origin),
    ]);
    git(["-C", path_str(&seed), "push", "origin", "main"]);

    git(["-C", path_str(&seed), "checkout", "-b", "review-head"]);
    fs::write(seed.join("pr-change.txt"), "pull request change\n").expect("write PR file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "pr-change.txt",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "pull request change",
    ]);
    let pull_request_head_sha = git_output(["-C", path_str(&seed), "rev-parse", "HEAD"]);
    git([
        "-C",
        path_str(&seed),
        "push",
        "origin",
        "HEAD:refs/temper/seed/pr-7",
    ]);
    git([
        "-C",
        path_str(origin),
        "update-ref",
        "refs/pull/7/head",
        pull_request_head_sha.as_str(),
    ]);
    git([
        "-C",
        path_str(origin),
        "update-ref",
        "-d",
        "refs/temper/seed/pr-7",
    ]);
    pull_request_head_sha
}

fn git<const N: usize>(args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .output()
        .expect("run git command");
    assert!(
        output.status.success(),
        "git command failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new("git")
        .args(args)
        .output()
        .expect("run git command");
    assert!(
        output.status.success(),
        "git command failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("git stdout is utf-8")
        .trim()
        .to_string()
}

fn shell_output(command: &str) -> std::process::Output {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .expect("run recovery shell command")
}

fn assert_shell_success(command: &str) {
    let output = shell_output(command);
    assert!(
        output.status.success(),
        "shell command failed with status {}\ncommand:\n{}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        command,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path_str(path: &Path) -> &str {
    path.as_os_str()
        .to_str()
        .unwrap_or_else(|| panic!("non-utf8 path: {:?}", path.as_os_str()))
}

fn assert_is_sha(value: &str) {
    assert_eq!(value.len(), 40, "not a full SHA: {value}");
    assert!(
        value.chars().all(|ch| ch.is_ascii_hexdigit()),
        "not hex: {value}"
    );
}
