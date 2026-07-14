// MODEL + UPDATE — pure data-to-data. No DOM, no globals, no `new EventSource`.
// This is the layer that carries most of the test weight (Appendix B, Layer 1).
//
// Everything is exported so tests import it directly; nothing is closure-captured
// (Appendix B.3, seam 2).

export type Lane = "triage" | "implement" | "review" | "ci" | "done";
export type Role = "code" | "review" | "pm";
export type Sev = "warn" | "bad";

export interface Activity {
  kind: "think" | "tool";
  text: string;
}

// A single line in the drill-down's live activity stream (UX §5). Maps to the
// agent's AgentEvent/StreamDelta: thinking / text / tool start / tool end. `k`
// is an optional short label (e.g. the tool name `bash`); when absent the view
// falls back to the kind. Held in a bounded ring buffer, dropped on reload.
export interface StreamEvent {
  kind: "think" | "text" | "tool";
  k?: string;
  v: string;
}

// Cap for the per-card live activity ring buffer (UX §5: "Capped ring buffer
// client-side; not persisted"). Drop-oldest once full.
export const STREAM_CAP = 100;
/// Drawer history is durable in the engine, but the browser projection remains
/// bounded. Reopening the drawer rebuilds this window from the journal.
export const TRACE_EVENT_CAP = 5_000;

export interface TraceUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

export interface TraceRunSummary {
  version: number;
  run_id: string;
  identity: {
    artifact_ref: string;
    role: string;
    action: string;
    correlation_key: string;
    repository: string;
    job_id: string;
    worker_id: string;
    assignment_id: string;
    agent_session_id?: string;
  };
  status: "active" | "succeeded" | "cancelled" | "failed";
  started_at?: string;
  completed_at?: string;
  duration_ms?: number;
  counts: {
    events: number;
    scopes: number;
    turns: number;
    model_calls: number;
    tool_calls: number;
    retries: number;
  };
  usage: TraceUsage;
  capture_mode: "off" | "metadata" | "transcript" | "diagnostic";
  has_truncated_content: boolean;
  has_trace_gaps: boolean;
  dropped_events: number;
  first_seq?: number;
  last_seq: number;
}

export interface AgentRunEvent {
  version: number;
  run_id: string;
  seq: number;
  occurred_at: string;
  elapsed_ms: number;
  assignment: {
    artifact_ref: string;
    role: string;
    action: string;
    correlation_key: string;
    [key: string]: unknown;
  };
  agent_session_id?: string;
  scope: { id: string; kind: "main" | "sub_agent"; parent_id?: string };
  turn?: number;
  event: { type: string; data: Record<string, unknown> };
}

export interface RunView {
  cardId: string;
  runs: TraceRunSummary[];
  selectedRun: string | null;
  events: AgentRunEvent[];
  loading: boolean;
  error?: string;
}

export interface Card {
  id: string;
  lane: Lane;
  ref: string;
  role: Role;
  title: string;
  enteredAt: number; // epoch ms
  steps?: { done: number; total: number };
  activity?: Activity;
  ci?: "running" | "failed";
  merged?: boolean;
  // ephemeral live-activity ring buffer (drill-down stream, UX §5). Bounded by
  // STREAM_CAP, drop-oldest. Not part of the durable snapshot contract.
  stream?: StreamEvent[];
}

// Per-lane state the board header renders. `sat` = work waiting with no free
// capacity (the saturation badge, UX §4.3 / §4.1 "N waiting · 0 slots").
export interface LaneState {
  id: Lane;
  title: string;
  sat: number;
}

export interface Problem {
  sev: Sev;
  msg: string;
  card: string;
  since: number;
}

export interface State {
  lanes: LaneState[];
  cards: Record<string, Card>;
  problems: Record<string, Problem>;
  workers: { healthy: number; total: number };
  lastEventAt: number;
  pipe: "live" | "stale" | "dead";
  openCard: string | null;
  now: number;
  cursor: number; // last applied feed sequence (snapshot+resume, Appendix B.5)
  // Drawer-only durable projection. It is populated from same-origin trace APIs
  // on open and discarded on close; it is never part of a board snapshot.
  runView: RunView | null;
}

// The pipeline columns, in flow order (UX §4.1). The ordered, mutually-exclusive
// lifecycle the read-model projects artifacts onto (UX Appendix A.4).
export const LANES: { id: Lane; title: string }[] = [
  { id: "triage", title: "Triage" },
  { id: "implement", title: "Implement" },
  { id: "review", title: "Review" },
  { id: "ci", title: "CI / Gates" },
  { id: "done", title: "Done" },
];

// Discriminated union mirroring the SSE/feed events. In prod these arrive as the
// `data:` payload of an SSE message; in tests we push them through the same path.
export type Event =
  | { t: "tick"; now: number }
  | { t: "snapshot"; seq: number; state: Partial<State> & { cards: Record<string, Card> } }
  | { t: "card.move"; seq: number; id: string; lane: Lane; now: number }
  | { t: "card.activity"; seq: number; id: string; activity: Activity }
  | { t: "card.step"; seq: number; id: string; steps: { done: number; total: number } }
  // push a line into a card's live activity ring buffer (drop-oldest, UX §5)
  | { t: "card.stream"; seq: number; id: string; event: StreamEvent }
  | { t: "problem.add"; seq: number; id: string; problem: Problem }
  | { t: "problem.clear"; seq: number; id: string }
  // a role/lane became saturated: N items waiting, 0 free slots (UX §4.3)
  | { t: "role.saturated"; seq: number; lane: Lane; waiting: number }
  | { t: "pipe"; pipe: State["pipe"] }
  // user actions funnel through the same reducer (unidirectional)
  | { t: "open"; id: string }
  | { t: "close" }
  | { t: "runs.loaded"; cardId: string; runs: TraceRunSummary[]; selectedRun: string | null }
  | { t: "run.select"; cardId: string; runId: string }
  | { t: "run.history"; cardId: string; runId: string; events: AgentRunEvent[] }
  | { t: "run.event"; cardId: string; runId: string; event: AgentRunEvent }
  | { t: "run.error"; cardId: string; message: string };

export const MIN = 60_000;
export const STUCK_MIN = 15;
export const WARN_MIN = 8;
export const STALE_MS = 5_000; // no event for this long => pipe goes stale

export function initialState(now: number): State {
  return {
    lanes: LANES.map((l) => ({ ...l, sat: 0 })),
    cards: {},
    problems: {},
    workers: { healthy: 0, total: 0 },
    lastEventAt: now,
    pipe: "live",
    openCard: null,
    now,
    cursor: 0,
    runView: null,
  };
}

// ── pure derivations (tested directly, no DOM) ────────────────────────────────
export function cardState(c: Card, now: number): "ok" | "work" | "warn" | "bad" {
  if (c.merged) return "ok";
  if (c.ci === "failed") return "bad";
  const ageMin = (now - c.enteredAt) / MIN;
  if (ageMin > STUCK_MIN && c.lane !== "done") return "bad";
  if (ageMin > WARN_MIN && c.lane !== "done") return "warn";
  if (c.activity || c.ci === "running") return "work";
  return "ok";
}

export function isStuck(c: Card, now: number): boolean {
  return cardState(c, now) === "bad" && !c.ci && !c.merged;
}

export function laneCards(state: State, lane: Lane): Card[] {
  return Object.values(state.cards)
    .filter((c) => c.lane === lane)
    .sort(
      (a, b) =>
        Number(isStuck(b, state.now)) - Number(isStuck(a, state.now)) ||
        b.enteredAt - a.enteredAt,
    );
}

export function stuckCount(state: State): number {
  return Object.values(state.cards).filter((c) => isStuck(c, state.now)).length;
}

// ── durable step ledger (UX §5) ──────────────────────────────────────────────
// Derive the checklist the drawer renders from a card's `steps {done,total}`:
// the first `done` rows are complete, the next is in progress, the rest pending.
// This is the authoritative, reload-surviving view (vs. the ephemeral stream).
export type LedgerStatus = "done" | "now" | "todo";
export interface LedgerStep {
  n: number; // 1-based
  status: LedgerStatus;
}

export function ledger(card: Card): LedgerStep[] {
  const total = card.steps?.total ?? 0;
  const done = card.steps?.done ?? 0;
  return Array.from({ length: total }, (_, i) => {
    const n = i + 1;
    const status: LedgerStatus = n <= done ? "done" : n === done + 1 ? "now" : "todo";
    return { n, status };
  });
}

// ── UPDATE: the single mutation path. Returns a NEW state (pure). ─────────────
// Out-of-order / duplicate feed events are ignored via the seq cursor, so
// snapshot-then-stream produces no gap and no dup (Appendix B.5).
export function apply(state: State, ev: Event): State {
  // feed events carry a seq; drop stale/dup ones
  if ("seq" in ev) {
    if (ev.seq <= state.cursor) return state; // already applied — no-op
  }
  const touched = "seq" in ev || ev.t === "tick";
  const base: State = touched ? { ...state, lastEventAt: state.now } : state;

  switch (ev.t) {
    case "tick": {
      // a dead pipe (connection lost) stays dead until a real event revives it —
      // check it first so a long idle gap can't downgrade "dead" to "stale".
      const pipe =
        state.pipe === "dead" ? "dead" : ev.now - state.lastEventAt > STALE_MS ? "stale" : "live";
      return { ...base, now: ev.now, pipe };
    }
    case "snapshot":
      return {
        ...base,
        ...ev.state,
        // keep the fixed lane order/titles; only adopt sat values if the
        // snapshot supplies them (it usually doesn't — sat is a live signal).
        lanes: ev.state.lanes ?? base.lanes,
        cards: { ...ev.state.cards },
        cursor: ev.seq,
        pipe: "live",
      };
    case "card.move": {
      const c = state.cards[ev.id];
      if (!c) return { ...base, cursor: ev.seq }; // unknown id => no-op (but advance cursor)
      return {
        ...base,
        cards: { ...state.cards, [ev.id]: { ...c, lane: ev.lane, enteredAt: ev.now } },
        cursor: ev.seq,
      };
    }
    case "card.activity": {
      const c = state.cards[ev.id];
      if (!c) return { ...base, cursor: ev.seq };
      return { ...base, cards: { ...state.cards, [ev.id]: { ...c, activity: ev.activity } }, cursor: ev.seq };
    }
    case "card.step": {
      const c = state.cards[ev.id];
      if (!c) return { ...base, cursor: ev.seq };
      return { ...base, cards: { ...state.cards, [ev.id]: { ...c, steps: ev.steps } }, cursor: ev.seq };
    }
    case "card.stream": {
      const c = state.cards[ev.id];
      if (!c) return { ...base, cursor: ev.seq };
      // append then drop-oldest past the cap (bounded ring buffer, UX §5)
      const stream = [...(c.stream ?? []), ev.event];
      if (stream.length > STREAM_CAP) stream.splice(0, stream.length - STREAM_CAP);
      return { ...base, cards: { ...state.cards, [ev.id]: { ...c, stream } }, cursor: ev.seq };
    }
    case "problem.add":
      return { ...base, problems: { ...state.problems, [ev.id]: ev.problem }, cursor: ev.seq };
    case "problem.clear": {
      const next = { ...state.problems };
      delete next[ev.id];
      return { ...base, problems: next, cursor: ev.seq };
    }
    case "role.saturated": {
      // set the lane's saturation count (0 clears the badge). Unknown lanes are
      // a no-op but still advance the cursor.
      const lanes = state.lanes.map((l) => (l.id === ev.lane ? { ...l, sat: ev.waiting } : l));
      return { ...base, lanes, cursor: ev.seq };
    }
    case "pipe":
      return { ...base, pipe: ev.pipe };
    case "open":
      return {
        ...state,
        openCard: ev.id,
        runView: { cardId: ev.id, runs: [], selectedRun: null, events: [], loading: true },
      };
    case "close":
      return { ...state, openCard: null, runView: null };
    case "runs.loaded": {
      if (state.openCard !== ev.cardId || state.runView?.cardId !== ev.cardId) return state;
      return {
        ...state,
        runView: {
          ...state.runView,
          runs: ev.runs,
          selectedRun: ev.selectedRun,
          events: [],
          loading: ev.selectedRun !== null,
          error: undefined,
        },
      };
    }
    case "run.select": {
      if (state.openCard !== ev.cardId || state.runView?.cardId !== ev.cardId) return state;
      return {
        ...state,
        runView: { ...state.runView, selectedRun: ev.runId, events: [], loading: true, error: undefined },
      };
    }
    case "run.history": {
      const view = state.runView;
      if (state.openCard !== ev.cardId || view?.selectedRun !== ev.runId) return state;
      const events = dedupeTraceEvents(ev.events).slice(-TRACE_EVENT_CAP);
      return { ...state, runView: { ...view, events, loading: false, error: undefined } };
    }
    case "run.event": {
      const view = state.runView;
      if (state.openCard !== ev.cardId || view?.selectedRun !== ev.runId) return state;
      if (view.events.some((event) => event.seq === ev.event.seq)) return state;
      const events = [...view.events, ev.event]
        .sort((left, right) => left.seq - right.seq)
        .slice(-TRACE_EVENT_CAP);
      return { ...state, runView: { ...view, events, loading: false, error: undefined } };
    }
    case "run.error": {
      if (state.openCard !== ev.cardId || state.runView?.cardId !== ev.cardId) return state;
      return { ...state, runView: { ...state.runView, loading: false, error: ev.message } };
    }
  }
}

function dedupeTraceEvents(events: AgentRunEvent[]): AgentRunEvent[] {
  const bySeq = new Map<number, AgentRunEvent>();
  for (const event of events) bySeq.set(event.seq, event);
  return [...bySeq.values()].sort((left, right) => left.seq - right.seq);
}
