#!/usr/bin/env python3
"""Hermetic MCP fixture for the controlled codebase-memory benchmark."""

import json
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else "enabled"

INDEX_PROPERTIES = {
    "repo_path": {"type": "string"},
    "name": {"type": "string"},
}
TOOLS = [
    {
        "name": "search_code",
        "description": "Search indexed source and return containing symbols",
        "inputSchema": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "project": {"type": "string"},
            },
            "required": ["pattern"],
        },
    },
    {
        "name": "search_graph",
        "description": "Search indexed symbols and relationships",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "project": {"type": "string"},
            },
        },
    },
    {
        "name": "get_architecture",
        "description": "Summarize indexed architecture",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
        },
    },
    {
        "name": "list_projects",
        "description": "List indexed projects",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "index_status",
        "description": "Return targeted index status",
        "inputSchema": {
            "type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"],
        },
    },
    {
        "name": "index_repository",
        "description": "Upsert a stable repository index",
        "inputSchema": {
            "type": "object",
            "properties": INDEX_PROPERTIES,
            "required": ["repo_path", "name"],
        },
    },
]


def send(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def tool_result(request_id, payload, is_error=False):
    send(
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "content": [{"type": "text", "text": payload}],
                "isError": is_error,
            },
        }
    )


for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    request_id = request["id"]
    method = request.get("method")
    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": "codebase-memory-mcp",
                        "version": "0.9.0",
                    },
                    "capabilities": {"tools": {}},
                },
            }
        )
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name", "")
        arguments = params.get("arguments") or {}
        if name == "index_status":
            tool_result(
                request_id,
                json.dumps(
                    {
                        "project": arguments.get("project", ""),
                        "status": "fresh",
                    }
                ),
            )
        elif name == "list_projects":
            tool_result(
                request_id,
                json.dumps(
                    {
                        "projects": [
                            {
                                "name": "temper-benchmark/codebase-memory-routing-repair",
                                "path": "fixture/repo",
                            }
                        ]
                    }
                ),
            )
        elif name == "index_repository":
            tool_result(
                request_id,
                json.dumps(
                    {
                        "project": arguments.get("name", ""),
                        "status": "fresh",
                    }
                ),
            )
        elif MODE == "unavailable":
            # The wrapper must classify and replace this provider text. It must
            # never appear in model-visible output or retained safe diagnostics.
            tool_result(
                request_id,
                "fixture backend unavailable Authorization: Bearer MCP-FIXTURE-SECRET",
                True,
            )
        else:
            tool_result(
                request_id,
                json.dumps(
                    {
                        "implementation": "src/route.rs::worker_slot",
                        "caller": "src/delivery.rs::DeliveryRouter::worker_for",
                        "focused_test": "tests/alias_retry.rs::alias_retries_stay_on_the_original_ordered_worker",
                    },
                    sort_keys=True,
                ),
            )
    else:
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "unknown method"},
            }
        )
