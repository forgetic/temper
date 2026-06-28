use std::path::{Component, Path, PathBuf};
use std::process::Command;

use super::HermeticRepoSpec;

pub(crate) fn seed_origin(
    git_root: &Path,
    seed_root: &Path,
    repo: &HermeticRepoSpec,
) -> Result<PathBuf, String> {
    let owner_root = git_root.join(&repo.owner);
    std::fs::create_dir_all(&owner_root)
        .map_err(|error| format!("create git owner dir {}: {error}", owner_root.display()))?;
    let origin = owner_root.join(format!("{}.git", repo.name));
    git_output_trim(&["init", "--bare", path_str(&origin)?])?;

    let seed = seed_root.join(&repo.owner).join(&repo.name);
    std::fs::create_dir_all(&seed)
        .map_err(|error| format!("create git seed dir {}: {error}", seed.display()))?;
    git_output_trim(&["init", "-b", &repo.default_branch, path_str(&seed)?])?;

    let seed_files = if repo.seed_files.is_empty() {
        vec![(PathBuf::from("README.md"), format!("# {}\n", repo.name))]
    } else {
        repo.seed_files.clone()
    };
    for (relative, contents) in seed_files {
        validate_relative_file_path(&relative)?;
        let path = seed.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("create seed file parent {}: {error}", parent.display())
            })?;
        }
        std::fs::write(&path, contents)
            .map_err(|error| format!("write seed file {}: {error}", path.display()))?;
    }

    git_output_trim(&[
        "-C",
        path_str(&seed)?,
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "-A",
    ])?;
    git_output_trim(&[
        "-C",
        path_str(&seed)?,
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial commit",
    ])?;
    git_output_trim(&[
        "-C",
        path_str(&seed)?,
        "remote",
        "add",
        "origin",
        path_str(&origin)?,
    ])?;
    git_output_trim(&[
        "-C",
        path_str(&seed)?,
        "push",
        "origin",
        &repo.default_branch,
    ])?;
    Ok(origin)
}

pub(crate) fn git_output_trim(args: &[&str]) -> Result<String, String> {
    let output = git_output(args)?;
    Ok(String::from_utf8(output.stdout)
        .map_err(|error| format!("git stdout is not utf8: {error}"))?
        .trim_end_matches('\n')
        .to_string())
}

pub(crate) fn git_output_raw(args: &[&str]) -> Result<String, String> {
    let output = git_output(args)?;
    String::from_utf8(output.stdout).map_err(|error| format!("git stdout is not utf8: {error}"))
}

pub(crate) fn path_str(path: &Path) -> Result<&str, String> {
    path.as_os_str()
        .to_str()
        .ok_or_else(|| format!("path is not UTF-8: {}", path.display()))
}

fn validate_relative_file_path(path: &Path) -> Result<(), String> {
    let mut saw_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_component = true,
            _ => {
                return Err(format!(
                    "seed file path `{}` must contain only normal relative components",
                    path.display()
                ));
            }
        }
    }
    if saw_component {
        Ok(())
    } else {
        Err("seed file path must not be empty".to_string())
    }
}

fn git_output(args: &[&str]) -> Result<std::process::Output, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("run git {args:?}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "git {args:?} failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
