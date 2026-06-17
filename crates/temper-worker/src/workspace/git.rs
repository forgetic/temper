use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

use super::{Workspace, WorkspaceError};

impl Workspace {
    /// Persists this workspace's git author identity and push credential into the
    /// repo's **local** `.git/config`, so a spawned agent (which holds no token)
    /// can `git commit`/`git push` against the prepared checkout. The push token
    /// goes into `http.extraheader`; it lives only in the worker-owned checkout's
    /// config, never on the agent's argv or env.
    ///
    /// Idempotent: re-running overwrites the same keys. Called after the checkout
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
        // Assemble the full argument vector up front so the actual `git`
        // invocation can run on the blocking pool. The worker runs on the
        // skein runtime (single-threaded, no tokio reactor), so git -- a
        // blocking subprocess -- must go through `spawn_blocking`.
        let mut full_args: Vec<OsString> = vec![
            OsString::from("-c"),
            OsString::from(format!("user.name={}", self.identity.user)),
            OsString::from("-c"),
            OsString::from(format!("user.email={}", self.identity.email)),
        ];
        if include_remote_header {
            full_args.push(OsString::from("-c"));
            full_args.push(OsString::from(format!(
                "http.extraheader=AUTHORIZATION: token {}",
                self.identity.token
            )));
        }
        if let Some(current_dir) = current_dir {
            full_args.push(OsString::from("-C"));
            full_args.push(current_dir.as_os_str().to_os_string());
        }
        full_args.extend(args);

        let output = skein::runtime::spawn_blocking(move || {
            Command::new("git")
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(&full_args)
                .output()
        })
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
}

fn status_string(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| status.to_string(), |code| code.to_string())
}
