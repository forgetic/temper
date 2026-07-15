// DRAWER — ephemeral board hints plus the durable engine-journal projection.
// All captured values pass through esc() and previews are bounded before they
// reach markup.

import {
  ledger,
  type AgentRunEvent,
  type Card,
  type RunView,
  type State,
  type TraceRunSummary,
} from "./model.js";

const PREVIEW_CAP = 512;
const esc = (value: string) =>
  value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[character]!);
const STEP_LABELS = ["write failing test", "implement", "run tests", "open PR"];

export function renderDrawer(state: State): string {
  const card = state.openCard ? state.cards[state.openCard] : null;
  const open = !!card;
  return `
    <div data-region="scrim" data-open="${open}" class="scrim ${open ? "open" : ""}"></div>
    <aside data-region="drawer" data-open="${open}"
           role="dialog" aria-modal="true" aria-label="agent activity"
           aria-hidden="${open ? "false" : "true"}"
           class="drawer ${open ? "open" : ""}">
      ${card ? drawerBody(card, state.runView) : ""}
    </aside>`;
}

function drawerBody(card: Card, runView: RunView | null): string {
  return `
    <div class="drawer-head">
      <div style="flex:1">
        <span class="ref">${esc(card.ref)}</span>
        <div class="sub">role:${esc(card.role)} · ${card.activity ? "● streaming" : "idle"}</div>
      </div>
      <button class="iconbtn" data-close aria-label="close">✕</button>
    </div>
    <div class="drawer-body">
      <p class="section-label">Runs</p>
      ${runListView(card, runView)}
      ${selectedRunView(runView)}
      <p class="section-label">Board activity</p>
      <div class="stream" aria-label="live activity">${streamView(card)}</div>
      <p class="section-label">Steps</p>
      <div class="ledger" aria-label="steps">${ledgerView(card)}</div>
    </div>`;
}

function runListView(card: Card, view: RunView | null): string {
  if (!view || view.cardId !== card.id || (view.loading && view.runs.length === 0)) {
    return `<div class="run-list quiet" aria-label="runs">loading durable history…</div>`;
  }
  if (view.runs.length === 0) {
    return `<div class="run-list quiet" aria-label="runs">no journalled runs</div>`;
  }
  return `<div class="run-list" aria-label="runs">${view.runs.map((run) => `
    <button data-run="${esc(run.run_id)}" aria-pressed="${run.run_id === view.selectedRun}"
            class="run-choice ${run.run_id === view.selectedRun ? "selected" : ""}">
      <span>${esc(shortRunId(run.run_id))}</span>
      <span class="run-status ${esc(run.status)}">${esc(run.status)}</span>
    </button>`).join("")}</div>`;
}

function selectedRunView(view: RunView | null): string {
  if (!view?.selectedRun) {
    return view?.error ? `<p class="trace-error" role="status">${esc(view.error)}</p>` : "";
  }
  const summary = view.runs.find((run) => run.run_id === view.selectedRun);
  return `
    ${summary ? summaryView(summary) : ""}
    ${view.error ? `<p class="trace-error" role="status">${esc(view.error)}</p>` : ""}
    ${view.loading ? `<p class="trace-loading" role="status">loading run timeline…</p>` : ""}
    <p class="section-label">Scopes</p>
    <div class="scope-tree" aria-label="scope tree">${scopeTreeView(view.events)}</div>
    <p class="section-label">Timeline</p>
    <div class="timeline" aria-label="run timeline">${timelineView(view.events)}</div>`;
}

function summaryView(run: TraceRunSummary): string {
  const usage = run.usage;
  const inputTokens = number(usage.input_tokens);
  const outputTokens = number(usage.output_tokens);
  const cacheReadTokens = number(usage.cache_read_tokens);
  const cacheWriteTokens = number(usage.cache_write_tokens);
  const tokens = inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens;
  const gap = run.has_trace_gaps ? ` · ${number(run.dropped_events)} dropped` : "";
  return `<section class="run-summary" aria-label="run summary">
    <div><b>${esc(run.status)}</b> · ${formatDuration(run.duration_ms)}</div>
    <div>${number(run.counts.turns)} turns · ${number(run.counts.tool_calls)} tools · ${number(run.counts.retries)} retries</div>
    <div>${tokens} tokens (${inputTokens} in / ${outputTokens} out)${gap}</div>
    <div>capture: ${esc(run.capture_mode)}${run.has_truncated_content ? " · previews truncated" : ""}</div>
  </section>`;
}

function scopeTreeView(events: AgentRunEvent[]): string {
  const scopes = new Map<string, AgentRunEvent["scope"]>();
  for (const event of events) scopes.set(event.scope.id, event.scope);
  if (scopes.size === 0) return quiet("no scopes recorded");
  const depth = (scope: AgentRunEvent["scope"]): number => {
    let result = 0;
    let parent = scope.parent_id;
    const seen = new Set<string>([scope.id]);
    while (parent && scopes.has(parent) && !seen.has(parent) && result < 20) {
      seen.add(parent);
      result += 1;
      parent = scopes.get(parent)!.parent_id;
    }
    return result;
  };
  return [...scopes.values()]
    .sort((left, right) => depth(left) - depth(right) || left.id.localeCompare(right.id))
    .map((scope) => `<div class="scope-node" style="--depth:${depth(scope)}" data-scope="${esc(scope.id)}">
      <span>${esc(scope.id)}</span><small>${esc(scope.kind)}${scope.parent_id ? ` ← ${esc(scope.parent_id)}` : ""}</small>
    </div>`)
    .join("");
}

function timelineView(events: AgentRunEvent[]): string {
  if (events.length === 0) return quiet("no durable events yet");
  const scopes = new Map<string, AgentRunEvent["scope"]>();
  for (const event of events) scopes.set(event.scope.id, event.scope);
  const depthFor = (scope: AgentRunEvent["scope"]): number => {
    let depth = 0;
    let parent = scope.parent_id;
    const seen = new Set<string>();
    while (parent && scopes.has(parent) && !seen.has(parent) && depth < 20) {
      seen.add(parent);
      depth += 1;
      parent = scopes.get(parent)!.parent_id;
    }
    return depth;
  };
  return [...events]
    .sort((left, right) => left.seq - right.seq)
    .map((event) => {
      const details = eventDetails(event);
      const data = isRecord(event.event.data) ? event.event.data : {};
      const classes = event.event.type.includes("failed") || event.event.type === "run.failed" || data.status === "failed"
        ? " failure"
        : event.event.type === "trace.gap" ? " gap" : "";
      return `<div class="trace-event${classes}" data-seq="${event.seq}" data-type="${esc(event.event.type)}"
                   style="--depth:${depthFor(event.scope)}">
        <span class="trace-time">${formatDuration(event.elapsed_ms)}</span>
        <span class="trace-kind">${esc(event.event.type)}</span>
        <span class="trace-detail">${esc(details)}</span>
      </div>`;
    })
    .join("");
}

function eventDetails(event: AgentRunEvent): string {
  const data = isRecord(event.event.data) ? event.event.data : {};
  const turn = event.turn === undefined ? "" : `turn ${event.turn}`;
  switch (event.event.type) {
    case "run.started": return `capture ${text(data.capture)}`;
    case "run.finished": return `${text(data.status)} · ${duration(data.duration_ms)}`;
    case "run.failed": return failure(data.failure);
    case "scope.started": return text(data.display_name) || event.scope.id;
    case "scope.finished": return `${text(data.status)} · ${duration(data.duration_ms)}`;
    case "turn.started": return turn;
    case "turn.finished": return `${turn} · ${text(data.stop_reason)} · ${duration(data.duration_ms)}`;
    case "model.call.started": return `${text(data.provider)}/${text(data.model)} · attempt ${number(data.attempt)}`;
    case "model.call.finished": return `${text(data.status)} · ${duration(data.duration_ms)}${optionalDuration(" · first token ", data.time_to_first_token_ms)}`;
    case "model.call.retrying": return `${failure(data.failure)} · retry ${number(data.next_attempt)} in ${duration(data.delay_ms)}`;
    case "tool.started": return `${text(data.name)}${contentSuffix(data.arguments)}`;
    case "tool.finished": return `${text(data.name)} · ${text(data.status)} · ${duration(data.duration_ms)}${contentSuffix(data.result)}`;
    case "assistant.message": return content(data.content);
    case "output.text.delta": return inlineText(data.delta);
    case "output.thinking.delta": return inlineText(data.delta);
    case "steering.applied": return `${text(data.source)}${contentSuffix(data.instruction)}`;
    case "usage": return `${number(data.input_tokens)} in / ${number(data.output_tokens)} out · ${number(data.cache_read_tokens)} cache read · ${number(data.cache_write_tokens)} cache write`;
    case "trace.gap": return `${number(data.dropped_events)} events / ${number(data.dropped_bytes)} bytes dropped · ${arrayText(data.kinds)}`;
    default: return bounded(JSON.stringify(data));
  }
}

function streamView(card: Card): string {
  const stream = card.stream ?? [];
  if (stream.length === 0) return quiet("no board activity yet");
  return stream.map((event) => `
    <div class="ev ${event.kind}">
      <span class="k">${esc(event.k ?? event.kind)}</span>
      <span class="v">${esc(event.v)}</span>
    </div>`).join("");
}

function ledgerView(card: Card): string {
  const steps = ledger(card);
  if (steps.length === 0) return `<div class="li quiet"><span class="lbl">no steps recorded</span></div>`;
  return steps.map((step) => {
    const box = step.status === "done" ? "[x]" : step.status === "now" ? "[>]" : "[ ]";
    const label = STEP_LABELS[step.n - 1] ?? "step";
    const cls = step.status === "todo" ? "" : step.status;
    return `<div class="li ${cls}" data-step="${step.n}" data-status="${step.status}">
      <span class="box">${box}</span><span class="lbl">${step.n} ${esc(label)}</span>
    </div>`;
  }).join("");
}

function failure(value: unknown): string {
  if (!isRecord(value)) return "failure";
  return `${text(value.code)}: ${bounded(text(value.message))}${value.retryable === true ? " (retryable)" : ""}`;
}

function contentSuffix(value: unknown): string {
  const preview = content(value);
  return preview ? ` · ${preview}` : "";
}

function content(value: unknown): string {
  if (!isRecord(value)) return "";
  if (value.storage === "inline" && isRecord(value.text)) return bounded(text(value.text.text));
  // serde's internally-tagged Inline variant flattens InlineContent fields.
  if (value.storage === "inline") return bounded(text(value.text));
  if (value.storage === "blob" && isRecord(value.blob)) {
    return `blob ${bounded(text(value.blob.digest), 80)} (${number(value.blob.bytes)} bytes)`;
  }
  return "";
}

function inlineText(value: unknown): string {
  return isRecord(value) ? bounded(text(value.text)) : "";
}

function optionalDuration(prefix: string, value: unknown): string {
  return typeof value === "number" ? `${prefix}${formatDuration(value)}` : "";
}

function duration(value: unknown): string {
  return typeof value === "number" ? formatDuration(value) : "";
}

function formatDuration(value?: number): string {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) return "in progress";
  return value < 1_000 ? `${value} ms` : `${(value / 1_000).toFixed(value < 10_000 ? 1 : 0)} s`;
}

function shortRunId(runId: string): string {
  return runId.length <= 14 ? runId : `${runId.slice(0, 11)}…`;
}

function bounded(value: string, limit = PREVIEW_CAP): string {
  return value.length <= limit ? value : `${value.slice(0, limit)}…`;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function number(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function arrayText(value: unknown): string {
  return Array.isArray(value) ? value.map(text).filter(Boolean).join(", ") : "";
}

function quiet(message: string): string {
  return `<div class="quiet"><span>${esc(message)}</span></div>`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
