// LAYER 1 — model/reducer tests. Pure data-to-data. NO DOM, NO backend.
// Runs in the plain `node` environment (see vitest.config.ts). This is where
// most coverage lives.

import { describe, it, expect } from "vitest";
import {
  apply,
  initialState,
  cardState,
  isStuck,
  laneCards,
  stuckCount,
  MIN,
  STUCK_MIN,
  STALE_MS,
  type Card,
  type State,
} from "../src/model.js";
import snapshot from "../fixtures/state-snapshot.json" with { type: "json" };

const T0 = 1_000_000;
const card = (over: Partial<Card> = {}): Card => ({
  id: "c1",
  lane: "implement",
  ref: "repo#1",
  role: "code",
  title: "t",
  enteredAt: T0,
  ...over,
});

describe("derivations", () => {
  it("flips to 'bad' exactly at the stuck threshold", () => {
    const c = card({ enteredAt: T0 });
    const justUnder = T0 + STUCK_MIN * MIN; // not yet over
    const justOver = T0 + STUCK_MIN * MIN + 1;
    expect(cardState(c, justUnder)).not.toBe("bad");
    expect(cardState(c, justOver)).toBe("bad");
    expect(isStuck(c, justOver)).toBe(true);
  });

  it("CI-failed is 'bad' but not 'stuck' (it's a CI problem, not a wedge)", () => {
    const c = card({ ci: "failed" });
    expect(cardState(c, T0)).toBe("bad");
    expect(isStuck(c, T0)).toBe(false);
  });

  it("merged cards never count as stuck/old", () => {
    const c = card({ lane: "done", merged: true, enteredAt: T0 - 100 * MIN });
    expect(cardState(c, T0)).toBe("ok");
    expect(isStuck(c, T0)).toBe(false);
  });

  it("stuck cards sort to the top of their lane", () => {
    let s = initialState(T0);
    s = apply(s, { t: "snapshot", seq: 1, state: { cards: {
      fresh: card({ id: "fresh", enteredAt: T0 }),
      old: card({ id: "old", enteredAt: T0 - 30 * MIN }),
    } } });
    s = { ...s, now: T0 };
    const order = laneCards(s, "implement").map((c) => c.id);
    expect(order[0]).toBe("old"); // stuck floats up
  });
});

describe("reducer", () => {
  it("ignores events for unknown ids (no throw, advances cursor)", () => {
    const s = initialState(T0);
    const next = apply(s, { t: "card.move", seq: 1, id: "nope", lane: "review", now: T0 });
    expect(next.cards).toEqual({});
    expect(next.cursor).toBe(1);
  });

  it("card.move resets age (enteredAt) to the move time", () => {
    let s = apply(initialState(T0), { t: "snapshot", seq: 1, state: { cards: { c1: card() } } });
    s = apply(s, { t: "card.move", seq: 2, id: "c1", lane: "review", now: T0 + 5 * MIN });
    expect(s.cards.c1!.lane).toBe("review");
    expect(s.cards.c1!.enteredAt).toBe(T0 + 5 * MIN);
  });

  it("drops duplicate/out-of-order events via the seq cursor (no gap, no dup)", () => {
    let s = apply(initialState(T0), { t: "snapshot", seq: 10, state: { cards: { c1: card() } } });
    // a replayed older event must be a no-op
    const replayed = apply(s, { t: "card.activity", seq: 5, id: "c1", activity: { kind: "tool", text: "stale" } });
    expect(replayed.cards.c1!.activity).toBeUndefined();
    expect(replayed.cursor).toBe(10);
    // the next in-order event applies
    s = apply(s, { t: "card.activity", seq: 11, id: "c1", activity: { kind: "tool", text: "fresh" } });
    expect(s.cards.c1!.activity?.text).toBe("fresh");
  });

  it("problem add then clear", () => {
    let s = initialState(T0);
    s = apply(s, { t: "problem.add", seq: 1, id: "x", problem: { sev: "warn", msg: "m", card: "c1", since: T0 } });
    expect(stuckCount(s)).toBe(0);
    expect(Object.keys(s.problems)).toEqual(["x"]);
    s = apply(s, { t: "problem.clear", seq: 2, id: "x" });
    expect(s.problems).toEqual({});
  });

  it("pipe goes 'stale' after STALE_MS without an event", () => {
    let s = initialState(T0);
    s = apply(s, { t: "tick", now: T0 + STALE_MS + 1 });
    expect(s.pipe).toBe("stale");
  });

  it("pipe 'dead' (connection lost) is sticky across ticks", () => {
    let s = apply(initialState(T0), { t: "pipe", pipe: "dead" });
    s = apply(s, { t: "tick", now: T0 + 1 });
    expect(s.pipe).toBe("dead");
  });
});

describe("feed contract (Layer 3): the real snapshot shape hydrates", () => {
  it("applies the fixture snapshot without throwing and populates the board", () => {
    const s = apply(initialState(T0), snapshot as any);
    expect(Object.keys(s.cards)).toHaveLength(4);
    expect(s.workers).toEqual({ healthy: 3, total: 3 });
    expect(s.cursor).toBe(100);
    // p6 has failed CI in the fixture
    expect((s as State).cards.p6!.ci).toBe("failed");
  });
});
