use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One environment mutation carried inside [`ContainmentCommand`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentChange {
    Set(OsString, OsString),
    Remove(OsString),
}

/// A complete spawn request moved into a prepared backend.
///
/// Backends must not run the payload and attach it later. Implementations must
/// use a pre-exec placement, a suspended process, a supervisor hand-off, or an
/// equivalent primitive that establishes ownership before the first payload
/// instruction. The API intentionally does not accept an already-spawned
/// [`std::process::Child`].
pub struct ContainmentCommand {
    program: OsString,
    arguments: Vec<OsString>,
    environment_changes: Vec<EnvironmentChange>,
    clear_environment: bool,
    cwd: Option<PathBuf>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl ContainmentCommand {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            environment_changes: Vec::new(),
            clear_environment: false,
            cwd: None,
            stdin: Stdio::inherit(),
            stdout: Stdio::inherit(),
            stderr: Stdio::inherit(),
        }
    }

    pub fn arg(&mut self, argument: impl Into<OsString>) -> &mut Self {
        self.arguments.push(argument.into());
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    pub fn env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> &mut Self {
        self.environment_changes
            .push(EnvironmentChange::Set(key.into(), value.into()));
        self
    }

    pub fn env_remove(&mut self, key: impl Into<OsString>) -> &mut Self {
        self.environment_changes
            .push(EnvironmentChange::Remove(key.into()));
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.clear_environment = true;
        self
    }

    pub fn current_dir(&mut self, cwd: impl Into<PathBuf>) -> &mut Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn stdin(&mut self, stdin: Stdio) -> &mut Self {
        self.stdin = stdin;
        self
    }

    pub fn stdout(&mut self, stdout: Stdio) -> &mut Self {
        self.stdout = stdout;
        self
    }

    pub fn stderr(&mut self, stderr: Stdio) -> &mut Self {
        self.stderr = stderr;
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    pub fn environment_changes(&self) -> &[EnvironmentChange] {
        &self.environment_changes
    }

    pub fn clears_environment(&self) -> bool {
        self.clear_environment
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Converts this request to `std::process::Command` inside a backend's
    /// race-free spawn implementation.
    pub fn into_std_command(self) -> Command {
        let program = self.program.clone();
        let arguments = self.arguments.clone();
        let mut command = Command::new(program);
        command.args(arguments);
        self.apply_spawn_properties(&mut command);
        command
    }

    /// Wraps the payload in the Linux supervisor helper while preserving the
    /// payload's argv, environment, cwd, and stdio exactly. The helper inherits
    /// those launch properties, spawns the payload with inherited stdio, and
    /// closes its own copies after the spawn handshake.
    #[cfg(target_os = "linux")]
    pub(crate) fn into_linux_supervisor_command(
        self,
        helper: &OsStr,
        mut helper_arguments: Vec<OsString>,
    ) -> Command {
        helper_arguments.push(OsString::from("--"));
        helper_arguments.push(self.program.clone());
        helper_arguments.extend(self.arguments.iter().cloned());

        let mut command = Command::new(helper);
        command.args(helper_arguments);
        self.apply_spawn_properties(&mut command);
        command
    }

    fn apply_spawn_properties(self, command: &mut Command) {
        if self.clear_environment {
            command.env_clear();
        }
        for change in self.environment_changes {
            match change {
                EnvironmentChange::Set(key, value) => {
                    command.env(key, value);
                }
                EnvironmentChange::Remove(key) => {
                    command.env_remove(key);
                }
            }
        }
        if let Some(cwd) = self.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(self.stdin)
            .stdout(self.stdout)
            .stderr(self.stderr);
    }
}
