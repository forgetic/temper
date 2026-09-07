"""Explicit native model, credentials, discovery, and journal configuration."""

from pathlib import Path
import shutil
import tomllib

from stack_support import write_toml

MODEL = "gpt-6-astra"


def configure(bundle, root, temper_bin, auth_file, codebase_memory=None):
    config_path = bundle / "config.toml"
    config = tomllib.loads(config_path.read_text())
    config.setdefault("paths", {})["state_dir"] = str(root / "state")
    config["engine"].update(poll_cadence_secs=2, ci_poll_cadence_secs=2,
                            mechanical_cadence_secs=2)
    profiles = config["agent"]["profiles"]
    for profile in profiles.values():
        profile.update(command=[str(temper_bin), "agent"], provider="chatgpt",
                       model=MODEL, investigate_model=MODEL, subagents=False,
                       max_iterations=100)
        profile.pop("credential", None)
    config.setdefault("observability", {})["agent_traces"] = {
        "capture": "diagnostic", "capture_thinking": False,
        "retention_days": 7, "max_run_bytes": 268435456,
    }
    if codebase_memory is None:
        command = shutil.which("codebase-memory-mcp")
        if not command:
            raise RuntimeError("codebase-memory-mcp is required for the benchmark")
        codebase_memory = {"mode": "required", "command": command, "args": [],
                           "roles": ["engineer"], "index": "background",
                           "startup_timeout_secs": 30, "index_timeout_secs": 60,
                           "retention": {"enabled": False}}
    config["agent"].setdefault("tools", {})["codebase_memory"] = codebase_memory
    write_toml(config_path, config)
    credentials_path = bundle / "credentials.toml"
    credentials = tomllib.loads(credentials_path.read_text())
    credentials.setdefault("agent", {}).setdefault("providers", {})["chatgpt"] = {
        "type": "oauth", "auth_file": str(Path(auth_file).resolve()),
    }
    write_toml(credentials_path, credentials)
    return {"provider": "chatgpt", "model": MODEL, "reasoning_effort": "xhigh",
            "reasoning_source": "native ChatGptOAuth coding_thinking_level",
            "profiles": list(profiles), "subagents": False,
            "service_tier": "provider_default", "codebase_memory": codebase_memory}
