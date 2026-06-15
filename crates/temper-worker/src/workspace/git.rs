use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

use super::{Workspace, WorkspaceError};

impl Workspace {
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
            Err(WorkspaceError::Git {
                command: redact_secret(command, &self.identity.token),
                status: status_string(output.status),
                stderr: redact_secret(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                    &self.identity.token,
                ),
            })
        }
    }
}

fn status_string(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| status.to_string(), |code| code.to_string())
}

fn redact_secret(text: String, secret: &str) -> String {
    if secret.is_empty() {
        text
    } else {
        text.replace(secret, "<redacted>")
    }
}
