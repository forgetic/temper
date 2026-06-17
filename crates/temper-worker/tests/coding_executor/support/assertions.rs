use std::process::Command;

use super::*;

pub fn expect_success(outcome: JobOutcome) -> (String, String, Option<String>) {
    match outcome {
        JobOutcome::Success {
            repos,
            summary,
            details: _,
        } => {
            assert_eq!(
                repos.len(),
                1,
                "coding executor produces exactly one per-repo outcome"
            );
            let branch = repos.into_iter().next().expect("one repo outcome").branch;
            (branch.name, branch.head_sha, summary)
        }
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => {
            panic!("expected success, got verdict {verdict:?} {body:?} {summary:?} {children:?}")
        }
        JobOutcome::Failure { class, message } => {
            panic!("expected success, got {class:?}: {message}")
        }
    }
}

pub fn expect_verdict(
    outcome: JobOutcome,
) -> (String, Option<String>, Option<String>, Vec<JobChild>) {
    match outcome {
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => (verdict, body, summary, children),
        JobOutcome::Success {
            repos,
            summary,
            details: _,
        } => panic!("expected verdict, got success {repos:?} {summary:?}"),
        JobOutcome::Failure { class, message } => {
            panic!("expected verdict, got {class:?}: {message}")
        }
    }
}

pub fn expect_failure_class(outcome: JobOutcome, expected: FailureClass) -> String {
    match outcome {
        JobOutcome::Failure { class, message } => {
            assert_eq!(class, expected, "unexpected failure message: {message}");
            message
        }
        JobOutcome::Success {
            repos,
            summary,
            details: _,
        } => panic!("expected {expected:?} failure, got success {repos:?} {summary:?}"),
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => {
            panic!(
                "expected {expected:?} failure, got verdict {verdict:?} {body:?} {summary:?} {children:?}"
            )
        }
    }
}

pub fn assert_no_origin_branch(fixture: &Fixture, branch_name: &str) {
    let output = Command::new("git")
        .args([
            "-C",
            path_str(&fixture.origin),
            "show-ref",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ])
        .output()
        .expect("run git show-ref");
    assert!(
        !output.status.success(),
        "origin unexpectedly has branch {branch_name}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

pub fn assert_no_extra_origin_head_branches(fixture: &Fixture, expected: &[&str]) {
    let output = git_output([
        "-C",
        path_str(&fixture.origin),
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
    ]);
    let mut branches = if output.is_empty() {
        Vec::new()
    } else {
        output.lines().map(str::to_string).collect::<Vec<_>>()
    };
    branches.sort();

    let mut expected = expected
        .iter()
        .map(|branch| (*branch).to_string())
        .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(branches, expected);
}

pub fn assert_workspace_clean(fixture: &Fixture, role: &str) {
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.workspace_root.join(role).join("service")),
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ]),
        ""
    );
}

pub fn git<const N: usize>(args: [&str; N]) {
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

pub fn git_output<const N: usize>(args: [&str; N]) -> String {
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
        .trim_end_matches('\n')
        .to_string()
}

pub fn path_str(path: &Path) -> &str {
    path.as_os_str()
        .to_str()
        .unwrap_or_else(|| panic!("non-utf8 path: {:?}", path.as_os_str()))
}

pub fn assert_is_sha(value: &str) {
    assert_eq!(value.len(), 40, "not a full SHA: {value}");
    assert!(
        value.chars().all(|ch| ch.is_ascii_hexdigit()),
        "not hex: {value}"
    );
}
