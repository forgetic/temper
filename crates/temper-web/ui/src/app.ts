// CONTROLLER — wires board SSE + drawer-scoped durable trace reads through the
// pure reducer. Network seams are injected, so lifecycle tests use fakes.

import {
  apply,
  initialState,
  TRACE_EVENT_CAP,
  type AgentRunEvent,
  type Event,
  type State,
  type TraceRunSummary,
} from "./model.js";
import { render } from "./view.js";

export interface EventSourceLike {
  onmessage: ((ev: { data: string }) => void) | null;
  onerror: ((ev?: unknown) => void) | null;
  close(): void;
}

export interface AppDeps {
  root: HTMLElement;
  eventSource: (url: string) => EventSourceLike;
  now: () => number;
  // Same-origin JSON facade. When absent (legacy/isolated tests), the board and
  // ephemeral stream continue to work without starting drawer trace work.
  fetchJson?: (url: string) => Promise<unknown>;
}

export interface App {
  state: State;
  dispatch: (ev: Event) => void;
  stop: () => void;
}

interface RunPage {
  runs: TraceRunSummary[];
  next_cursor?: string;
}

interface EventPage {
  run_id: string;
  events: AgentRunEvent[];
  next_after_seq: number;
  has_more: boolean;
}

export function createApp(deps: AppDeps): App {
  const app: App = {
    state: initialState(deps.now()),
    dispatch,
    stop,
  };
  let detailSource: EventSourceLike | null = null;
  let traceGeneration = 0;

  function dispatch(ev: Event): void {
    app.state = apply(app.state, ev);
    render(deps.root, app.state);

    if (ev.t === "open") void openCardTrace(ev.id);
    if (ev.t === "close") stopCardTrace();
    if (ev.t === "run.select") void selectRun(ev.cardId, ev.runId);
  }

  const boardSource = deps.eventSource("/events");
  boardSource.onmessage = (message) => dispatch(JSON.parse(message.data) as Event);
  boardSource.onerror = () => dispatch({ t: "pipe", pipe: "dead" });

  deps.root.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-close]") || target.closest('[data-region="scrim"]')) {
      dispatch({ t: "close" });
      return;
    }
    const run = target.closest("[data-run]") as HTMLElement | null;
    if (run && app.state.openCard) {
      dispatch({ t: "run.select", cardId: app.state.openCard, runId: run.dataset.run! });
      return;
    }
    const card = target.closest("[data-card]") as HTMLElement | null;
    if (card) dispatch({ t: "open", id: card.dataset.card! });
  });
  deps.root.ownerDocument.addEventListener("keydown", (event) => {
    if ((event as KeyboardEvent).key === "Escape") dispatch({ t: "close" });
  });

  const themeButton = deps.root.ownerDocument.getElementById("theme");
  themeButton?.addEventListener("click", () => {
    const root = deps.root.ownerDocument.documentElement;
    root.dataset.theme = root.dataset.theme === "light" ? "dark" : "light";
  });

  async function openCardTrace(cardId: string): Promise<void> {
    stopDetailSource();
    const generation = ++traceGeneration;
    const fetchJson = deps.fetchJson;
    const card = app.state.cards[cardId];
    if (!fetchJson || !card) return;

    try {
      const runs: TraceRunSummary[] = [];
      const seenCursors = new Set<string>();
      let cursor: string | undefined;
      do {
        const query = new URLSearchParams({ artifact_ref: card.ref, limit: "50" });
        if (cursor) query.set("cursor", cursor);
        const page = asRunPage(await fetchJson(`/api/agent-runs?${query.toString()}`));
        if (generation !== traceGeneration || app.state.openCard !== cardId) return;
        runs.push(...page.runs);
        if (runs.length > 200) runs.splice(0, runs.length - 200);
        cursor = page.next_cursor;
        if (cursor && seenCursors.has(cursor)) throw new Error("trace run cursor did not advance");
        if (cursor) seenCursors.add(cursor);
      } while (cursor);

      // The engine's documented ordering is stable ascending start time/run id;
      // keeping the final bounded window therefore preserves the latest run.
      const selectedRun = runs.at(-1)?.run_id ?? null;
      dispatch({ t: "runs.loaded", cardId, runs, selectedRun });
      if (selectedRun) await loadRun(cardId, selectedRun, generation);
    } catch {
      if (generation === traceGeneration && app.state.openCard === cardId) {
        dispatch({ t: "run.error", cardId, message: "durable run history is temporarily unavailable" });
      }
    }
  }

  async function selectRun(cardId: string, runId: string): Promise<void> {
    stopDetailSource();
    const generation = ++traceGeneration;
    if (!deps.fetchJson) return;
    try {
      await loadRun(cardId, runId, generation);
    } catch {
      if (generation === traceGeneration && app.state.openCard === cardId) {
        dispatch({ t: "run.error", cardId, message: "durable run history is temporarily unavailable" });
      }
    }
  }

  async function loadRun(cardId: string, runId: string, generation: number): Promise<void> {
    const fetchJson = deps.fetchJson;
    if (!fetchJson) return;
    const events: AgentRunEvent[] = [];
    let afterSeq = 0;
    let hasMore = true;
    while (hasMore && events.length < TRACE_EVENT_CAP) {
      const page = asEventPage(
        await fetchJson(
          `/api/agent-runs/${encodeURIComponent(runId)}/events?after_seq=${afterSeq}&limit=500`,
        ),
      );
      if (
        generation !== traceGeneration ||
        app.state.openCard !== cardId ||
        app.state.runView?.selectedRun !== runId
      ) return;
      events.push(...page.events.filter((event) => event.seq > afterSeq));
      const next = Math.max(afterSeq, page.next_after_seq);
      if (page.has_more && next === afterSeq && page.events.length === 0) {
        throw new Error("trace event cursor did not advance");
      }
      afterSeq = next;
      hasMore = page.has_more;
    }

    dispatch({ t: "run.history", cardId, runId, events });
    if (
      generation !== traceGeneration ||
      app.state.openCard !== cardId ||
      app.state.runView?.selectedRun !== runId
    ) return;

    const source = deps.eventSource(
      `/api/agent-runs/${encodeURIComponent(runId)}/stream?after_seq=${afterSeq}`,
    );
    detailSource = source;
    source.onmessage = (message) => {
      if (generation !== traceGeneration || app.state.openCard !== cardId) return;
      try {
        const event = asAgentRunEvent(JSON.parse(message.data));
        if (event.run_id !== runId || event.seq <= 0) return;
        dispatch({ t: "run.event", cardId, runId, event });
      } catch {
        // One malformed frame is ignored. The cursor remains at the last valid
        // event and native EventSource reconnect semantics stay intact.
      }
    };
    source.onerror = () => {
      // Native EventSource reconnects and sends Last-Event-ID. Keep this source
      // alive; a later valid event clears the temporary message.
      if (generation === traceGeneration && app.state.openCard === cardId) {
        dispatch({ t: "run.error", cardId, message: "live run details reconnecting…" });
      }
    };
  }

  function stopDetailSource(): void {
    detailSource?.close();
    detailSource = null;
  }

  function stopCardTrace(): void {
    traceGeneration += 1;
    stopDetailSource();
  }

  function stop(): void {
    stopCardTrace();
    boardSource.close();
  }

  render(deps.root, app.state);
  return app;
}

function asRunPage(value: unknown): RunPage {
  if (!isRecord(value) || !Array.isArray(value.runs)) throw new Error("invalid run page");
  return value as unknown as RunPage;
}

function asEventPage(value: unknown): EventPage {
  if (
    !isRecord(value) ||
    typeof value.run_id !== "string" ||
    !Array.isArray(value.events) ||
    typeof value.next_after_seq !== "number" ||
    typeof value.has_more !== "boolean"
  ) throw new Error("invalid event page");
  return value as unknown as EventPage;
}

function asAgentRunEvent(value: unknown): AgentRunEvent {
  if (
    !isRecord(value) ||
    typeof value.run_id !== "string" ||
    typeof value.seq !== "number" ||
    !isRecord(value.event) ||
    typeof value.event.type !== "string"
  ) throw new Error("invalid run event");
  return value as unknown as AgentRunEvent;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
