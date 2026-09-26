"""Compare effective settings while retaining raw configuration-file evidence."""

import copy
import hashlib
import json
from pathlib import Path


def codex_config_fingerprint(config, output: Path, pairs: int, *, layout: str = "paired") -> str:
    root = output.resolve()
    if layout == "paired":
        checkouts = [root / f"pairs/{pair:03}/codex/repo" for pair in range(1, pairs + 1)]
    elif layout == "single":
        checkouts = [root / "codex/repo"]
    elif layout == "none":
        checkouts = []
    else:
        raise ValueError("Codex checkout layout must be paired, single, or none")
    normalized = copy.deepcopy(config)
    projects = normalized.get("projects")
    if isinstance(projects, dict):
        for checkout in checkouts:
            key = str(checkout)
            if projects.get(key) == {"trust_level": "trusted"}:
                del projects[key]
        if not projects:
            normalized.pop("projects")
    serialized = json.dumps(normalized, sort_keys=True, separators=(",", ":"), allow_nan=False)
    return hashlib.sha256(serialized.encode()).hexdigest()


def configurations_match(observed, frozen) -> bool:
    if not all(isinstance(value.get("codex_effective_config_sha256"), str)
               for value in (observed, frozen)):
        return observed == frozen
    # Raw file hashes remain in both evidence records. Every effective setting,
    # other fingerprint and binary still participates in the comparison.
    return ({key: value for key, value in observed.items() if key != "codex_config_sha256"}
            == {key: value for key, value in frozen.items() if key != "codex_config_sha256"})
