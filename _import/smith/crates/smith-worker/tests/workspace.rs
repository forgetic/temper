use std::fs;
use std::path::Path;
use std::process::Command;

use smith_worker::{RoleGitIdentity, Workspace, WorkspaceConfig};
use tempfile::tempdir;

#[tokio::test]
async fn workspace_prepares_commits_pushes_and_reuses_local_git_checkout() {
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
    let workspace = Workspace::new(&config, "ai/smith", "engineer", identity, path_str(&origin))
        .expect("workspace config is valid");
    assert_eq!(workspace.path(), config.root.join("ai__smith/engineer"));

    workspace
        .prepare("smith-worker/work")
        .await
        .expect("prepare workspace");
    assert!(workspace.path().exists());
    assert_eq!(
        git_output(["-C", path_str(workspace.path()), "branch", "--show-current"]),
        "smith-worker/work"
    );
    let head = workspace.head_sha().await.expect("workspace head sha");
    let origin_main = git_output(["-C", path_str(workspace.path()), "rev-parse", "origin/main"]);
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
}

#[tokio::test]
async fn prepare_pull_request_head_checks_out_the_pull_ref() {
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
    let workspace = Workspace::new(&config, "ai/smith", "reviewer", identity, path_str(&origin))
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
