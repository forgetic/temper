// Drawer trace lifecycle + durable timeline rendering.
// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import { screen, waitFor, within } from "@testing-library/dom";
import userEvent from "@testing-library/user-event";
import { createApp, type App } from "../src/app.js";
import type { AgentRunEvent, Card, TraceRunSummary } from "../src/model.js";
import { FakeEventSource } from "./fake-event-source.js";

const T0 = 1_000_000;
const card: Card = {
  id: "c1", lane: "implement", ref: "ai/temper#312", role: "code", title: "traces", enteredAt: T0,
};

const summary: TraceRunSummary = {
  version: 1,
  run_id: "run-312",
  identity: {
    worker_id: "w", assignment_id: "a", job_id: "j", repository: "ai/temper",
    artifact_ref: "ai/temper#312", role: "code", action: "open_pr", correlation_key: "work",
  },
  status: "active",
  started_at: "2026-01-01T00:00:00Z",
  counts: { events: 3, scopes: 2, turns: 1, model_calls: 1, tool_calls: 1, retries: 1 },
  usage: { input_tokens: 10, output_tokens: 5, cache_read_tokens: 2, cache_write_tokens: 1 },
  capture_mode: "transcript",
  has_truncated_content: false,
  has_trace_gaps: true,
  dropped_events: 4,
  first_seq: 1,
  last_seq: 3,
};

function traceEvent(seq: number, type: string, data: Record<string, unknown>, scope = "main"): AgentRunEvent {
  return {
    version: 1, run_id: "run-312", seq, occurred_at: "2026-01-01T00:00:00Z", elapsed_ms: seq * 10,
    assignment: { artifact_ref: "ai/temper#312", role: "code", action: "open_pr", correlation_key: "work" },
    scope: scope === "main"
      ? { id: "main", kind: "main" }
      : { id: scope, kind: "sub_agent", parent_id: "main" },
    turn: 1,
    event: { type, data },
  };
}

let root: HTMLElement;
let board: FakeEventSource;
let sources: FakeEventSource[];
let requests: string[];
let app: App;

beforeEach(() => {
  document.body.innerHTML = `<div id="root"></div>`;
  root = document.getElementById("root")!;
  sources = [];
  requests = [];
  const eventSource = (url: string) => {
    const source = new FakeEventSource(url);
    sources.push(source);
    return source;
  };
  const events = [
    traceEvent(1, "tool.started", {
      call_id: "tool-1", name: "bash", arguments: { storage: "inline", text: "<img src=x onerror=boom>", truncated: false },
    }, "investigate-1"),
    traceEvent(2, "tool.finished", {
      call_id: "tool-1", name: "bash", status: "failed", duration_ms: 1250,
      result: { storage: "inline", text: "failed <script>alert(1)</script>", truncated: false },
    }, "investigate-1"),
    traceEvent(3, "trace.gap", { dropped_events: 4, dropped_bytes: 100, kinds: ["text_delta"] }),
    traceEvent(4, "model.call.finished", {
      call_id: "model-timeout", attempt: 0, status: "failed", duration_ms: 5000,
      failure: {
        provider: "openai", model: "gpt-test", category: "timeout", retryable: true,
        message: "Model request timed out.", detail_redacted: false,
      },
    }),
    traceEvent(5, "model.call.finished", {
      call_id: "model-rate", attempt: 1, status: "failed", duration_ms: 250,
      failure: {
        provider: "openai", model: "gpt-test", category: "rate_limit", retryable: true,
        http_status: 429, provider_request_id: "req_rate_532", provider_error_code: "rate_limit",
        message: "Rate limit exceeded; retry later.", detail_redacted: false,
      },
    }),
    traceEvent(6, "model.call.finished", {
      call_id: "model-response", attempt: 0, status: "failed", duration_ms: 90,
      failure: {
        provider: "anthropic", model: "claude-test", category: "response", retryable: false,
        http_status: 502, message: "Provider returned a malformed stream.", detail_redacted: false,
      },
    }),
    traceEvent(7, "model.call.finished", {
      call_id: "model-redacted", attempt: 0, status: "failed", duration_ms: 10,
      failure: {
        provider: "unknown", model: "unknown", category: "redacted_unknown", retryable: false,
        message: "Provider failure details were redacted.", detail_redacted: true,
        raw_response: "WEB-SECRET-SENTINEL-532",
      },
    }),
  ];
  const fetchJson = async (url: string): Promise<unknown> => {
    requests.push(url);
    if (url.startsWith("/api/agent-runs?")) {
      if (!url.includes("cursor=")) {
        return {
          runs: [{ ...summary, run_id: "run-old", started_at: "2025-12-01T00:00:00Z" }],
          next_cursor: "newer-runs",
        };
      }
      return { runs: [summary] };
    }
    if (url.includes("/events?")) {
      return { run_id: "run-312", events, next_after_seq: 7, has_more: false };
    }
    throw new Error(`unexpected request ${url}`);
  };
  app = createApp({ root, eventSource, fetchJson, now: () => T0 });
  board = sources[0]!;
  board.push({ t: "snapshot", seq: 1, state: { cards: { c1: card } } });
});

describe("durable run drawer", () => {
  it("fetches history, resumes SSE by cursor, and renders tools/gaps/scopes safely", async () => {
    await userEvent.click(screen.getByLabelText("ai/temper#312: traces"));

    await waitFor(() => expect(sources).toHaveLength(2));
    expect(requests[0]).toContain("artifact_ref=ai%2Ftemper%23312");
    expect(requests[1]).toContain("cursor=newer-runs");
    expect(requests[2]).toContain("after_seq=0");
    expect(sources[1]!.url).toBe("/api/agent-runs/run-312/stream?after_seq=7");

    const drawer = screen.getByLabelText("agent activity");
    expect(within(drawer).getByLabelText("run summary").textContent).toContain("1 tools");
    const scopeTree = within(drawer).getByLabelText("scope tree");
    expect(scopeTree.textContent).toContain("investigate-1");
    expect(scopeTree.textContent).toContain("← main");
    const timeline = within(drawer).getByLabelText("run timeline");
    expect(timeline.textContent).toContain("bash · failed · 1.3 s");
    expect(timeline.textContent).toContain("4 events / 100 bytes dropped");
    expect(timeline.textContent).toContain("openai/gpt-test · category=timeout · retryable=true");
    expect(timeline.textContent).toContain("category=rate_limit · retryable=true · HTTP 429 · request req_rate_532 · code rate_limit");
    expect(timeline.textContent).toContain("anthropic/claude-test · category=response · retryable=false · HTTP 502");
    expect(timeline.textContent).toContain("category=redacted_unknown · retryable=false · detail redacted · Provider failure details were redacted.");
    expect(timeline.textContent).not.toContain("WEB-SECRET-SENTINEL-532");
    expect(timeline.textContent).toContain("<script>alert(1)</script>");
    expect(timeline.querySelector("script")).toBeNull();
    expect(timeline.querySelector("img")).toBeNull();
  });

  it("deduplicates live resume events and closes detailed work with the drawer", async () => {
    await userEvent.click(screen.getByLabelText("ai/temper#312: traces"));
    await waitFor(() => expect(sources).toHaveLength(2));
    const detail = sources[1]!;
    const live = traceEvent(8, "turn.finished", { duration_ms: 50, stop_reason: "end_turn" });
    detail.onmessage?.({ data: JSON.stringify(live) });
    detail.onmessage?.({ data: JSON.stringify(live) });
    expect(app.state.runView!.events.filter((event) => event.seq === 8)).toHaveLength(1);

    await userEvent.click(within(screen.getByLabelText("agent activity")).getByLabelText("close"));
    expect(detail.closed).toBe(true);
    expect(app.state.runView).toBeNull();
  });
});
