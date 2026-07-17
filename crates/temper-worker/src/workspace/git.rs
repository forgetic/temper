use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

use crate::managed_effect::ManagedCommand;

use super::{Workspace, WorkspaceError};

impl Workspace {
    /// Persists this workspace's git author identity and push credential into the
    /// repo's **local** `.git/config`, so a spawned agent (which holds no token)
    /// can `git commit`/`git push` against the prepared repo checkout. The push token
    /// goes into `http.extraheader`; it lives only in the worker-owned checkout's
    /// config, never on the agent's argv or env.
    ///
    /// Idempotent: re-running overwrites the same keys. Called after the repo checkout
    /// is prepared and before the agent is spawned.
    pub async fn configure_local_identity(&self) -> Result<(), WorkspaceError> {
        for (key, value) in [
            ("user.name".to_string(), self.identity.user.clone()),
            ("user.email".to_string(), self.identity.email.clone()),
            (
                "http.extraheader".to_string(),
                format!("AUTHORIZATION: token {}", self.identity.token),
            ),
        ] {
            // `--local` writes the repo's own `.git/config`; the label is
            // token-free (the value is passed as a separate arg, so neither the
            // command label nor git's stderr carries the token).
            self.run_local_config(&key, value).await?;
        }
        Ok(())
    }

    /// Runs `git config --local <key> <value>` in this checkout, returning a
    /// token-free error label on failure.
    async fn run_local_config(&self, key: &str, value: String) -> Result<(), WorkspaceError> {
        self.run_workspace_git(
            false,
            format!("git config --local {key} <value>"),
            vec![
                OsString::from("config"),
                OsString::from("--local"),
                OsString::from(key),
                OsString::from(value),
            ],
        )
        .await
        .map(|_| ())
    }

    pub(super) async fn run_workspace_git(
        &self,
        include_remote_header: bool,
        command: String,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        self.run_git(Some(&self.path), include_remote_header, command, args)
            .await
    }

    pub(super) async fn run_git(
        &self,
        current_dir: Option<&Path>,
        include_remote_header: bool,
        command: String,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        let output = self
            .run_git_unchecked(current_dir, include_remote_header, args)
            .await?;
        if output.status.success() {
            Ok(output)
        } else {
            // The labeled `command` is a hand-written, token-free string and the
            // remote URL never embeds the token (it is passed via a separate
            // `-c http.extraheader` arg), so neither the command nor git's
            // stderr carries the push token.
            Err(WorkspaceError::Git {
                command,
                status: status_string(output.status),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    pub(super) async fn run_workspace_git_unchecked(
        &self,
        include_remote_header: bool,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        self.run_git_unchecked(Some(&self.path), include_remote_header, args)
            .await
    }

    async fn run_git_unchecked(
        &self,
        current_dir: Option<&Path>,
        include_remote_header: bool,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        // Each invocation has a dedicated process-tree owner. Dropping this
        // future (the watchdog path) kills git and any credential/remote helper,
        // joins its waiter and output readers, and only then lets the executor
        // report quiescence.
        let full_args =
            git_invocation_args(&self.identity, current_dir, include_remote_header, args);
        let mut git = Command::new("git");
        git.env("GIT_TERMINAL_PROMPT", "0").args(&full_args);
        self.cancellation
            .run(ManagedCommand::spawn(git, self.cancellation.clone()))
            .await
            .ok_or(WorkspaceError::Cancelled)?
            .map_err(WorkspaceError::Io)
    }
}

fn git_invocation_args(
    identity: &super::RoleGitIdentity,
    current_dir: Option<&Path>,
    include_remote_header: bool,
    args: Vec<OsString>,
) -> Vec<OsString> {
    let mut full_args: Vec<OsString> = vec![
        OsString::from("-c"),
        OsString::from(format!("user.name={}", identity.user)),
        OsString::from("-c"),
        OsString::from(format!("user.email={}", identity.email)),
    ];
    if include_remote_header {
        // A prepared writable checkout persists `http.extraheader` locally so
        // the live agent can use git. Git accumulates extra headers across
        // config scopes, so reset inherited/local values before adding the
        // worker-owned header for this remote operation. Otherwise Forgejo sees
        // duplicate Authorization headers and rejects the request.
        full_args.push(OsString::from("-c"));
        full_args.push(OsString::from("http.extraheader="));
        full_args.push(OsString::from("-c"));
        full_args.push(OsString::from(format!(
            "http.extraheader=AUTHORIZATION: token {}",
            identity.token
        )));
    }
    if let Some(current_dir) = current_dir {
        full_args.push(OsString::from("-C"));
        full_args.push(current_dir.as_os_str().to_os_string());
    }
    full_args.extend(args);
    full_args
}

fn status_string(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| status.to_string(), |code| code.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn remote_git_args_reset_persisted_extra_headers_before_auth() {
        let args = git_invocation_args(
            &identity(),
            Some(Path::new("/workspace/repo")),
            true,
            vec![OsString::from("fetch")],
        );
        let args = strings(args);

        let reset = find_pair(&args, "-c", "http.extraheader=")
            .expect("empty extraheader reset is present");
        let auth = find_pair(&args, "-c", "http.extraheader=AUTHORIZATION: token token-1")
            .expect("auth extraheader is present");

        assert!(reset < auth, "reset must precede auth header: {args:?}");
    }

    #[test]
    fn local_git_args_do_not_add_remote_auth_header() {
        let args = git_invocation_args(
            &identity(),
            Some(Path::new("/workspace/repo")),
            false,
            vec![OsString::from("status")],
        );
        let args = strings(args);

        assert!(!args.iter().any(|arg| arg.contains("http.extraheader")));
    }

    fn identity() -> super::super::RoleGitIdentity {
        super::super::RoleGitIdentity {
            user: "engineer".to_string(),
            email: "engineer@example.invalid".to_string(),
            token: "token-1".to_string(),
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn find_pair(args: &[String], key: &str, value: &str) -> Option<usize> {
        args.windows(2)
            .position(|window| window[0] == key && window[1] == value)
    }
}
