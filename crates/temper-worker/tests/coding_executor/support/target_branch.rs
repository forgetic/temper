use std::process::Command;

use super::*;

pub fn single_repo_assign(
    correlation_key: &str,
    branch_hint: &str,
    default_branch: &str,
    base_branch: &str,
) -> Assign {
    assign_with_repos(
        correlation_key,
        vec![writable_repo(
            "acme/service",
            "service",
            default_branch,
            base_branch,
            branch_hint,
        )],
    )
}

pub fn coordinated_assign(
    correlation_key: &str,
    branch_hint: &str,
    target_branch: &str,
    lib_writable: bool,
) -> Assign {
    let lib_repo = if lib_writable {
        writable_repo("acme/lib", "lib", "main", target_branch, branch_hint)
    } else {
        json!({
            "repo": "acme/lib",
            "dir": "lib",
            "access": "read_only",
            "default_branch": "main",
            "base_branch": "main"
        })
    };
    assign_with_repos(
        correlation_key,
        vec![
            writable_repo(
                "acme/service",
                "service",
                "main",
                target_branch,
                branch_hint,
            ),
            lib_repo,
        ],
    )
}

pub fn seed_repo_from_service_main(fixture: &Fixture, repo: &str) {
    let origin = origin_path(fixture, repo);
    if let Some(parent) = origin.parent() {
        fs::create_dir_all(parent).expect("create git repo parent");
    }
    git(["init", "--bare", path_str(&origin)]);
    let seed = temp_root(fixture).join(format!("seed-{}", repo.replace('/', "-")));
    git(["clone", path_str(&fixture.origin), path_str(&seed)]);
    git(["-C", path_str(&seed), "checkout", "main"]);
    git([
        "-C",
        path_str(&seed),
        "remote",
        "set-url",
        "origin",
        path_str(&origin),
    ]);
    git(["-C", path_str(&seed), "push", "origin", "main"]);
}

pub fn seed_feature_branch(fixture: &Fixture, repo: &str, branch: &str) -> String {
    let seed = temp_root(fixture).join(format!(
        "seed-{}-{}",
        repo.replace('/', "-"),
        branch.replace('/', "-")
    ));
    git([
        "clone",
        path_str(&origin_path(fixture, repo)),
        path_str(&seed),
    ]);
    git(["-C", path_str(&seed), "checkout", "main"]);
    git(["-C", path_str(&seed), "checkout", "-b", branch]);
    fs::write(seed.join("feature-marker.txt"), "feature branch marker\n")
        .expect("write feature marker");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "feature-marker.txt",
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
        "seed feature branch",
    ]);
    let head = git_output(["-C", path_str(&seed), "rev-parse", "HEAD"]);
    git([
        "-C",
        path_str(&seed),
        "push",
        "origin",
        &format!("HEAD:refs/heads/{branch}"),
    ]);
    head
}

pub fn branch_head(fixture: &Fixture, repo: &str, branch: &str) -> String {
    git_output([
        "-C",
        path_str(&origin_path(fixture, repo)),
        "rev-parse",
        &format!("refs/heads/{branch}"),
    ])
}

pub fn assert_no_branch(fixture: &Fixture, repo: &str, branch: &str) {
    assert!(
        !ref_exists(fixture, repo, &format!("refs/heads/{branch}")),
        "origin {repo} unexpectedly has branch {branch}"
    );
}

fn assign_with_repos(correlation_key: &str, repos: Vec<Value>) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: format!("acme/service/issue-7/engineer/{correlation_key}"),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: json!({
            "role": "engineer",
            "repo": "acme/service",
            "queue": "code_ready",
            "artifact_kind": "code",
            "artifact": {
                "number": 7,
                "title": "Implement the thing",
                "body": "Detailed issue body",
                "labels": ["code", "ready"],
                "state": "Open"
            },
            "workspace": {
                "coordination_key": correlation_key,
                "repos": repos
            },
            "action": "open_pr",
            "checkout_capability": "writable",
            "allowed_verdicts": []
        }),
    }
}

fn writable_repo(
    repo: &str,
    dir: &str,
    default_branch: &str,
    base_branch: &str,
    branch_hint: &str,
) -> Value {
    json!({
        "repo": repo,
        "dir": dir,
        "access": "writable",
        "default_branch": default_branch,
        "base_branch": base_branch,
        "branch_hint": branch_hint
    })
}

fn ref_exists(fixture: &Fixture, repo: &str, refname: &str) -> bool {
    Command::new("git")
        .args([
            "-C",
            path_str(&origin_path(fixture, repo)),
            "show-ref",
            "--verify",
            refname,
        ])
        .output()
        .expect("run git show-ref")
        .status
        .success()
}

fn origin_path(fixture: &Fixture, repo: &str) -> PathBuf {
    temp_root(fixture).join("git").join(format!("{repo}.git"))
}

fn temp_root(fixture: &Fixture) -> &Path {
    fixture
        .workspace_root
        .parent()
        .expect("workspace root has temp parent")
}
