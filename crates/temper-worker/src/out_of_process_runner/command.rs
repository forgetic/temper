//! Agent subprocess command and per-run configuration files.

use super::*;

impl OutOfProcessRunner {
    pub(super) fn write_tool_config(
        &self,
        directory: &Path,
        context: &WorkspaceContext,
    ) -> Result<Option<PathBuf>, AgentRunError> {
        let Some(tool_config) = self
            .tool_config
            .as_ref()
            .filter(|config| config.enabled_for_role(&context.work_item.role))
        else {
            return Ok(None);
        };
        let path = directory.join("tool-config.json");
        let bytes = serde_json::to_vec_pretty(tool_config).map_err(|error| {
            AgentRunError::transient(format!("serialize agent tool config: {error}"))
        })?;
        std::fs::write(&path, bytes).map_err(|error| {
            AgentRunError::transient(format!("write agent tool config file: {error}"))
        })?;
        Ok(Some(path))
    }

    pub(super) fn write_trace_policy(&self, directory: &Path, job_id: &str) -> Option<PathBuf> {
        let policy = self.trace_policy.as_ref()?;
        let path = directory.join("trace-policy.json");
        let bytes = match serde_json::to_vec_pretty(policy) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.policy_serialize_failed",
                    job_id,
                    %error,
                    "worker could not serialize agent trace policy; continuing without child activity"
                );
                return None;
            }
        };
        if let Err(error) = std::fs::write(&path, bytes) {
            tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.policy_write_failed",
                job_id,
                path = %path.display(),
                %error,
                "worker could not write agent trace policy; continuing without child activity"
            );
            None
        } else {
            Some(path)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn child_command(
        &self,
        program: &str,
        args: &[String],
        cwd: &Path,
        context_path: &Path,
        result_path: &Path,
        tool_config_path: Option<&Path>,
        runtime_limits_path: Option<&Path>,
        terminal_output_path: Option<&Path>,
        trace_policy_path: Option<&Path>,
        operator_transcript_path: Option<&Path>,
        lifecycle_address: Option<&str>,
        activity_address: Option<&str>,
        submit_address: Option<&str>,
        forge_address: Option<&str>,
    ) -> temper_process_containment::ContainmentCommand {
        let mut command = temper_process_containment::ContainmentCommand::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .arg("--context")
            .arg(context_path)
            .arg("--result")
            .arg(result_path)
            .arg("--workspace")
            .arg(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(path) = tool_config_path {
            command.arg(TOOL_CONFIG_FLAG).arg(path);
        }
        if let Some(path) = runtime_limits_path {
            command.arg(RUNTIME_LIMITS_FLAG).arg(path);
        }
        if let Some(path) = terminal_output_path {
            command.arg(TERMINAL_OUTPUT_FLAG).arg(path);
        }
        if let Some(path) = trace_policy_path {
            command.arg(TRACE_POLICY_FLAG).arg(path);
        }
        if let Some(path) = operator_transcript_path {
            command.arg(OPERATOR_TRANSCRIPT_FLAG).arg(path);
        }
        if let Some(address) = lifecycle_address {
            command.arg(AGENT_LIFECYCLE_ADDRESS_FLAG).arg(address);
        }
        if let Some(address) = activity_address {
            command.arg(ACTIVITY_ADDRESS_FLAG).arg(address);
        }
        if let Some(address) = submit_address {
            command.arg(SUBMIT_FOR_PR_ADDRESS_FLAG).arg(address);
        }
        if let Some(address) = forge_address {
            command.arg(FORGE_CONTEXT_ADDRESS_FLAG).arg(address);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}
