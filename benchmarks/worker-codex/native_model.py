"""Verify the client model actually invoked, independently of requested config."""

from collections import Counter
import json
from pathlib import Path


def model_evidence(journal_root, expected_attempts):
    requests, errors = Counter(), []
    for path in sorted(Path(journal_root).rglob("events.jsonl")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            try:
                event = json.loads(line)["event"]
                if event["type"] != "model.call.started":
                    continue
                data = event["data"]
                provider, model = data.get("provider"), data.get("model")
                if not isinstance(provider, str) or not isinstance(model, str):
                    raise ValueError("model request identity is missing")
                requests[(provider, model)] += 1
            except (ValueError, KeyError, TypeError) as error:
                errors.append({"trace": str(path), "line": number, "error": str(error)})
    count = sum(requests.values())
    complete = bool(count) and count == expected_attempts and not errors
    return {"source": "native_model_call_started_client_events", "observed_attempts": count,
            "expected_attempts": expected_attempts, "complete": complete,
            "matches_requested_model": complete and set(requests) == {("openai-codex", "gpt-6-astra")},
            "requests": [{"provider": provider, "model": model, "attempts": number}
                         for (provider, model), number in sorted(requests.items())],
            "errors": errors,
            "provider_reported_model": None,
            "reasoning_effort": "xhigh",
            "reasoning_evidence": "native ChatGptOAuth coding_thinking_level; not in trace events"}
