use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use temper_io_engine::process::{ProcessCall, ProcessCallError, run_process};

use crate::{ConversationReply, ConversationRequest, InteractionError, InteractiveResponder};

const STDERR_PREVIEW_LIMIT: usize = 4096;

/// Configuration for invoking an out-of-process interactive responder.
///
/// The adapter sends one serialized [`ConversationRequest`] to the process on
/// stdin and expects exactly one serialized [`ConversationReply`] on stdout. The
/// process receives only the allow-listed environment variables configured here;
/// Forge tokens, Forge handles, workflow tools, and other ambient environment
/// are not inherited by default.
///
/// `Debug` redacts forwarded environment values (which may be secrets such as
/// API keys or Forge tokens) while keeping their names visible.
#[derive(Clone, Eq, PartialEq)]
pub struct ProcessResponderConfig {
    /// Program to execute.
    pub program: PathBuf,
    /// Arguments supplied to the program.
    pub args: Vec<String>,
    /// Optional working directory for the process.
    pub working_dir: Option<PathBuf>,
    /// Environment variables forwarded to the subprocess as resolved
    /// name→value pairs. Values are resolved once at the config/arg boundary;
    /// this adapter never reads ambient process environment at spawn time.
    pub env: Vec<(String, String)>,
    /// Maximum wall-clock duration for one responder turn.
    pub timeout: Duration,
}

impl fmt::Debug for ProcessResponderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env_redacted: Vec<(&str, &str)> = self
            .env
            .iter()
            .map(|(name, _value)| (name.as_str(), "<redacted>"))
            .collect();
        formatter
            .debug_struct("ProcessResponderConfig")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("working_dir", &self.working_dir)
            .field("env", &env_redacted)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ProcessResponderConfig {
    /// Default one-turn timeout for a process responder.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Builds process-responder configuration with no inherited environment and
    /// the default one-turn timeout.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
            env: Vec::new(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Replaces the command arguments.
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the working directory.
    pub fn with_working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    /// Replaces the forwarded environment with resolved name→value pairs.
    ///
    /// Values must be resolved by the caller at the config/arg boundary (where
    /// process environment is read once); this adapter forwards them verbatim
    /// and never reads ambient environment itself.
    pub fn with_env<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env = pairs
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    /// Sets the one-turn timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Provider-neutral adapter for an external interactive responder process.
///
/// The process protocol is deliberately narrow: JSON request on stdin, JSON
/// reply on stdout, no Forge handles and no workflow tools. The adapter validates
/// proposal ids and kinds through the generic interaction types before returning
/// the reply to the session runtime.
#[derive(Debug)]
pub struct ProcessResponder {
    /// Clock/deadline capability of the engine task this responder runs
    /// under, injected at construction — process timeouts compute against it.
    cx: temper_io_engine::Cx,
    config: ProcessResponderConfig,
}

impl ProcessResponder {
    /// Builds a process responder after validating static configuration.
    pub fn new(
        cx: temper_io_engine::Cx,
        config: ProcessResponderConfig,
    ) -> Result<Self, InteractionError> {
        validate_config(&config)?;
        Ok(Self { cx, config })
    }

    /// Returns the process configuration.
    pub fn config(&self) -> &ProcessResponderConfig {
        &self.config
    }
}

#[async_trait]
impl InteractiveResponder for ProcessResponder {
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        let mut request_json = serde_json::to_vec(request)?;
        request_json.push(b'\n');

        // One responder turn is one `<io-event-request>`: the call below is
        // data describing the spawn (program, args, allow-listed env, stdin
        // payload, deadline); the engine performs it and hands back the
        // buffered output for this pure code to interpret.
        let mut call = ProcessCall::new(self.config.program.to_string_lossy().into_owned());
        call.args = self.config.args.clone();
        call.clear_env = true;
        call.current_dir = self.config.working_dir.clone();
        call.stdin = Some(request_json);
        call.timeout = Some(self.config.timeout);
        for (name, value) in &self.config.env {
            call.env.insert(name.clone(), value.clone());
        }

        let output = run_process(&self.cx, call)
            .await
            .map_err(|error| match error {
                ProcessCallError::Spawn(message) => InteractionError::ProcessResponderIo {
                    operation: "spawn",
                    source: std::io::Error::other(message),
                },
                ProcessCallError::Io { operation, message } => {
                    InteractionError::ProcessResponderIo {
                        operation,
                        source: std::io::Error::other(message),
                    }
                }
                ProcessCallError::TimedOut => InteractionError::ProcessResponderTimeout {
                    timeout: self.config.timeout,
                },
            })?;
        if !output.success {
            return Err(InteractionError::ProcessResponderExit {
                status: output.status_display,
                stderr: preview_lossy(&output.stderr),
            });
        }
        let reply: ConversationReply = serde_json::from_slice(&output.stdout)
            .map_err(|source| InteractionError::ProcessResponderMalformedJson { source })?;
        reply.validate()?;
        Ok(reply)
    }
}

fn validate_config(config: &ProcessResponderConfig) -> Result<(), InteractionError> {
    if config.program.as_os_str().is_empty() {
        return Err(InteractionError::InvalidConfig {
            field: "process_responder.program",
            message: "must not be empty".into(),
        });
    }
    if config.timeout.is_zero() {
        return Err(InteractionError::InvalidConfig {
            field: "process_responder.timeout",
            message: "must be greater than zero".into(),
        });
    }
    for (name, _value) in &config.env {
        validate_env_name(name)?;
    }
    Ok(())
}

fn validate_env_name(name: &str) -> Result<(), InteractionError> {
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return Err(InteractionError::InvalidConfig {
            field: "process_responder.env_allowlist",
            message: format!("invalid environment variable name `{name}`"),
        });
    }
    Ok(())
}

fn preview_lossy(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if text.len() > STDERR_PREVIEW_LIMIT {
        let end = text
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= STDERR_PREVIEW_LIMIT)
            .last()
            .unwrap_or(0);
        text.truncate(end);
        text.push_str("...");
    }
    text
}
