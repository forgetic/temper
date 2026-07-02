use std::fs;
use std::path::Path;
use std::process::Command;

pub(super) fn initialise_git_repo(path: &Path) -> Result<(), String> {
    fs::write(path.join("README.md"), "seed repo\n")
        .map_err(|error| format!("write seed README: {error}"))?;
    run_git(path, &["init"])?;
    run_git(path, &["checkout", "-B", "main"])?;
    run_git(path, &["add", "README.md"])?;
    run_git(path, &commit_args())
}

fn commit_args() -> [&'static str; 7] {
    [
        "-c",
        "user.name=Temper Scenario",
        "-c",
        "user.email=scenario@example.invalid",
        "commit",
        "-m",
        "seed",
    ]
}

fn run_git(path: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|error| format!("run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
