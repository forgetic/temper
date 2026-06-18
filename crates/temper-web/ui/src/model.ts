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
}

export interface Problem {
  sev: Sev;
  msg: string;
  card: string;
  since: number;
}

export interface State {
  cards: Record<string, Card>;
  problems: Record<string, Problem>;
  workers: { healthy: number; total: number };
  lastEventAt: number;
  pipe: "live" | "stale" | "dead";
  openCard: string | null;
  now: number;
  cursor: number; // last applied feed sequence (snapshot+resume, Appendix B.5)
}

// Discriminated union mirroring the SSE/feed events. In prod these arrive as the
// `data:` payload of an SSE message; in tests we push them through the same path.
export type Event =
  | { t: "tick"; now: number }
  | { t: "snapshot"; seq: number; state: Partial<State> & { cards: Record<string, Card> } }
  | { t: "card.move"; seq: number; id: string; lane: Lane; now: number }
  | { t: "card.activity"; seq: number; id: string; activity: Activity }
  | { t: "card.step"; seq: number; id: string; steps: { done: number; total: number } }
  | { t: "problem.add"; seq: number; id: string; problem: Problem }
  | { t: "problem.clear"; seq: number; id: string }
  | { t: "pipe"; pipe: State["pipe"] }
  // user actions funnel through the same reducer (unidirectional)
  | { t: "open"; id: string }
  | { t: "close" };

export const MIN = 60_000;
export const STUCK_MIN = 15;
export const WARN_MIN = 8;
export const STALE_MS = 5_000; // no event for this long => pipe goes stale

export function initialState(now: number): State {
  return {
    cards: {},
    problems: {},
    workers: { healthy: 0, total: 0 },
    lastEventAt: now,
    pipe: "live",
    openCard: null,
    now,
    cursor: 0,
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
      const pipe = ev.now - state.lastEventAt > STALE_MS ? "stale" : state.pipe === "dead" ? "dead" : "live";
      return { ...base, now: ev.now, pipe };
    }
    case "snapshot":
      return {
        ...base,
        ...ev.state,
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
    case "problem.add":
      return { ...base, problems: { ...state.problems, [ev.id]: ev.problem }, cursor: ev.seq };
    case "problem.clear": {
      const next = { ...state.problems };
      delete next[ev.id];
      return { ...base, problems: next, cursor: ev.seq };
    }
    case "pipe":
      return { ...base, pipe: ev.pipe };
    case "open":
      return { ...state, openCard: ev.id };
    case "close":
      return { ...state, openCard: null };
  }
}
