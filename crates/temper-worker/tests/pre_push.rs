use std::fs;
use std::path::Path;
use std::process::Command;

use temper_worker::{PrePushStatus, run_pre_push_checks};
use tempfile::tempdir;

#[test]
fn missing_config_is_a_successful_noop() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let report = run_pre_push_checks(temp.path())
            .await
            .expect("missing config is ok");

        assert_eq!(report.status, PrePushStatus::NotConfigured);
        assert!(report.passed());
        assert!(!report.required);
        assert!(report.commands.is_empty());
        assert_eq!(
            report.config_path,
            temp.path().join(".temper/pre-push.toml")
        );
    });
}

#[test]
fn passing_config_runs_commands_in_repo_cwd() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        write_config(
            temp.path(),
            r#"
version = 1

[pre_push]
required = true
cwd = "repo"

[[pre_push.commands]]
id = "fmt"
argv = ["sh", "-c", "printf pass; printf warn >&2"]
timeout_secs = 5
"#,
        );

        let report = run_pre_push_checks(temp.path()).await.expect("run checks");

        assert_eq!(report.status, PrePushStatus::Passed);
        assert!(report.passed());
        assert!(report.required);
        assert_eq!(report.commands.len(), 1);
        let command = &report.commands[0];
        assert_eq!(command.id, "fmt");
        assert_eq!(command.argv[0], "sh");
        assert_eq!(command.cwd, temp.path());
        assert_eq!(command.exit_code, Some(0));
        assert!(!command.timed_out);
        assert_eq!(command.stdout_tail, "pass");
        assert_eq!(command.stderr_tail, "warn");
    });
}

#[test]
fn failing_command_stops_the_sequence() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        write_config(
            temp.path(),
            r#"
version = 1

[pre_push]
required = true
cwd = "repo"

[[pre_push.commands]]
id = "fail"
argv = ["sh", "-c", "printf out; printf err >&2; exit 42"]
timeout_secs = 5

[[pre_push.commands]]
id = "never"
argv = ["sh", "-c", "printf never"]
timeout_secs = 5
"#,
        );

        let report = run_pre_push_checks(temp.path()).await.expect("run checks");

        assert_eq!(report.status, PrePushStatus::Failed);
        assert!(!report.passed());
        assert_eq!(report.commands.len(), 1);
        let command = &report.commands[0];
        assert_eq!(command.id, "fail");
        assert_eq!(command.exit_code, Some(42));
        assert!(!command.timed_out);
        assert_eq!(command.stdout_tail, "out");
        assert_eq!(command.stderr_tail, "err");
    });
}

#[test]
fn timed_out_command_is_reported_and_stops_the_sequence() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        write_config(
            temp.path(),
            r#"
version = 1

[pre_push]
required = true
cwd = "repo"

[[pre_push.commands]]
id = "slow"
argv = ["sh", "-c", "printf before; sleep 5; printf after"]
timeout_secs = 1

[[pre_push.commands]]
id = "never"
argv = ["sh", "-c", "printf never"]
timeout_secs = 5
"#,
        );

        let report = run_pre_push_checks(temp.path()).await.expect("run checks");

        assert_eq!(report.status, PrePushStatus::Failed);
        assert!(!report.passed());
        assert_eq!(report.commands.len(), 1);
        let command = &report.commands[0];
        assert_eq!(command.id, "slow");
        assert!(command.timed_out, "command should time out: {command:?}");
        assert!(
            command.elapsed_ms >= 900,
            "elapsed_ms: {}",
            command.elapsed_ms
        );
        assert!(
            command.elapsed_ms < 3_000,
            "timeout should stop promptly, elapsed_ms: {}",
            command.elapsed_ms
        );
        assert!(
            command.stdout_tail.contains("before"),
            "stdout tail: {:?}",
            command.stdout_tail
        );
        assert!(
            !command.stdout_tail.contains("after"),
            "stdout tail: {:?}",
            command.stdout_tail
        );
    });
}

#[test]
fn config_is_read_from_the_current_checkout_branch() {
    temper_worker_io::block_on(async {
        let temp = tempdir().expect("create temp dir");
        let repo = temp.path().join("repo");
        git(["init", "-b", "main", path_str(&repo)]);
        write_config(&repo, config_printing("main-config"));
        git(["-C", path_str(&repo), "add", ".temper/pre-push.toml"]);
        git_commit(&repo, "main config");

        git(["-C", path_str(&repo), "checkout", "-b", "feature"]);
        write_config(&repo, config_printing("feature-config"));
        git(["-C", path_str(&repo), "add", ".temper/pre-push.toml"]);
        git_commit(&repo, "feature config");

        git(["-C", path_str(&repo), "checkout", "main"]);
        let main_report = run_pre_push_checks(&repo).await.expect("main checks");
        assert_eq!(main_report.status, PrePushStatus::Passed);
        assert_eq!(main_report.commands[0].stdout_tail, "main-config");

        git(["-C", path_str(&repo), "checkout", "feature"]);
        let feature_report = run_pre_push_checks(&repo).await.expect("feature checks");
        assert_eq!(feature_report.status, PrePushStatus::Passed);
        assert_eq!(feature_report.commands[0].stdout_tail, "feature-config");
    });
}

fn config_printing(text: &str) -> String {
    format!(
        r#"
version = 1

[pre_push]
required = true
cwd = "repo"

[[pre_push.commands]]
id = "branch"
argv = ["sh", "-c", "printf {text}"]
timeout_secs = 5
"#
    )
}

fn write_config(repo: &Path, contents: impl AsRef<str>) {
    let temper_dir = repo.join(".temper");
    fs::create_dir_all(&temper_dir).expect("create .temper dir");
    fs::write(temper_dir.join("pre-push.toml"), contents.as_ref()).expect("write pre-push config");
}

fn git<const N: usize>(args: [&str; N]) {
    let output = Command::new("git").args(args).output().expect("run git");
    assert!(
        output.status.success(),
        "git failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_commit(repo: &Path, message: &str) {
    git([
        "-C",
        path_str(repo),
        "-c",
        "user.name=Test User",
        "-c",
        "user.email=test@example.test",
        "commit",
        "-m",
        message,
    ]);
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("test path is utf-8")
}
