// LAYER 1 — reducer tests for the board drill-down + polish (PR A). Pure
// data-to-data, NO DOM, NO backend. Runs in the plain `node` environment.
//
// Covers: the live activity ring-buffer cap, drawer open/close for a specific
// id, saturation set/clear, the durable step-ledger derivation, and the
// pulse stale/dead transitions.

import { describe, it, expect } from "vitest";
import {
  apply,
  initialState,
  ledger,
  STREAM_CAP,
  STALE_MS,
  type Card,
  type Event,
  type StreamEvent,
} from "../src/model.js";
import snapshot from "../fixtures/state-snapshot.json" with { type: "json" };
import streamFixture from "../fixtures/activity/stream-i41.json" with { type: "json" };
import satFixture from "../fixtures/activity/saturation.json" with { type: "json" };

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

// seed a single card via a snapshot, returning the populated state.
function withCard(over: Partial<Card> = {}) {
  return apply(initialState(T0), {
    t: "snapshot",
    seq: 1,
    state: { cards: { c1: card(over) } },
  });
}

describe("live activity ring buffer (UX §5)", () => {
  const ev = (n: number): StreamEvent => ({ kind: "text", v: `line ${n}` });

  it("appends activity in order onto the target card", () => {
    let s = withCard();
    s = apply(s, { t: "card.stream", seq: 2, id: "c1", event: ev(1) });
    s = apply(s, { t: "card.stream", seq: 3, id: "c1", event: ev(2) });
    expect(s.cards.c1!.stream!.map((e) => e.v)).toEqual(["line 1", "line 2"]);
  });

  it("caps at STREAM_CAP, dropping the OLDEST (boundary)", () => {
    let s = withCard();
    let seq = 2;
    // push exactly one past the cap
    for (let i = 0; i < STREAM_CAP + 1; i++) {
      s = apply(s, { t: "card.stream", seq: seq++, id: "c1", event: ev(i) });
    }
    const stream = s.cards.c1!.stream!;
    expect(stream).toHaveLength(STREAM_CAP);
    // the very first line (index 0) was dropped; the window starts at line 1
    expect(stream[0]!.v).toBe("line 1");
    expect(stream[STREAM_CAP - 1]!.v).toBe(`line ${STREAM_CAP}`);
  });

  it("exactly STREAM_CAP fits with nothing dropped (lower boundary)", () => {
    let s = withCard();
    let seq = 2;
    for (let i = 0; i < STREAM_CAP; i++) {
      s = apply(s, { t: "card.stream", seq: seq++, id: "c1", event: ev(i) });
    }
    expect(s.cards.c1!.stream).toHaveLength(STREAM_CAP);
    expect(s.cards.c1!.stream![0]!.v).toBe("line 0");
  });

  it("stream for an unknown id is a no-op but still advances the cursor", () => {
    let s = withCard();
    s = apply(s, { t: "card.stream", seq: 2, id: "nope", event: ev(1) });
    expect(s.cards.c1!.stream).toBeUndefined();
    expect(s.cursor).toBe(2);
  });

  it("a card's ring buffer is isolated from another card's", () => {
    let s = apply(initialState(T0), {
      t: "snapshot",
      seq: 1,
      state: { cards: { c1: card(), c2: card({ id: "c2", ref: "repo#2" }) } },
    });
    s = apply(s, { t: "card.stream", seq: 2, id: "c1", event: ev(1) });
    expect(s.cards.c1!.stream).toHaveLength(1);
    expect(s.cards.c2!.stream).toBeUndefined();
  });
});

describe("drawer open/close (a specific id)", () => {
  it("open targets the given card id; close clears it", () => {
    let s = withCard();
    s = apply(s, { t: "open", id: "c1" });
    expect(s.openCard).toBe("c1");
    s = apply(s, { t: "close" });
    expect(s.openCard).toBeNull();
  });

  it("opening a different id replaces the target", () => {
    let s = withCard();
    s = apply(s, { t: "open", id: "c1" });
    s = apply(s, { t: "open", id: "other" });
    expect(s.openCard).toBe("other");
  });
});

describe("saturation badge state (UX §4.3)", () => {
  it("role.saturated sets the lane's sat count", () => {
    let s = initialState(T0);
    s = apply(s, { t: "role.saturated", seq: 1, lane: "implement", waiting: 3 });
    expect(s.lanes.find((l) => l.id === "implement")!.sat).toBe(3);
    // other lanes untouched
    expect(s.lanes.find((l) => l.id === "review")!.sat).toBe(0);
  });

  it("waiting:0 clears the badge", () => {
    let s = apply(initialState(T0), { t: "role.saturated", seq: 1, lane: "review", waiting: 2 });
    expect(s.lanes.find((l) => l.id === "review")!.sat).toBe(2);
    s = apply(s, { t: "role.saturated", seq: 2, lane: "review", waiting: 0 });
    expect(s.lanes.find((l) => l.id === "review")!.sat).toBe(0);
  });

  it("saturation survives a card.move (orthogonal to card flow)", () => {
    let s = withCard();
    s = apply(s, { t: "role.saturated", seq: 2, lane: "implement", waiting: 5 });
    s = apply(s, { t: "card.move", seq: 3, id: "c1", lane: "review", now: T0 });
    expect(s.lanes.find((l) => l.id === "implement")!.sat).toBe(5);
  });
});

describe("durable step ledger derivation (UX §5)", () => {
  it("no steps => an empty ledger", () => {
    expect(ledger(card())).toEqual([]);
  });

  it("splits done / now / todo from {done,total}", () => {
    const rows = ledger(card({ steps: { done: 2, total: 4 } }));
    expect(rows.map((r) => r.status)).toEqual(["done", "done", "now", "todo"]);
    expect(rows.map((r) => r.n)).toEqual([1, 2, 3, 4]);
  });

  it("all done => no 'now' row", () => {
    const rows = ledger(card({ steps: { done: 3, total: 3 } }));
    expect(rows.map((r) => r.status)).toEqual(["done", "done", "done"]);
  });

  it("zero done => first row is 'now'", () => {
    const rows = ledger(card({ steps: { done: 0, total: 2 } }));
    expect(rows.map((r) => r.status)).toEqual(["now", "todo"]);
  });
});

describe("pulse stale/dead transitions (UX §4.2)", () => {
  it("a tick within STALE_MS keeps the pipe live", () => {
    let s = initialState(T0);
    s = apply(s, { t: "tick", now: T0 + STALE_MS - 1 });
    expect(s.pipe).toBe("live");
  });

  it("a tick past STALE_MS flips the pipe to stale", () => {
    let s = initialState(T0);
    s = apply(s, { t: "tick", now: T0 + STALE_MS + 1 });
    expect(s.pipe).toBe("stale");
  });

  it("dead (connection lost) stays dead across ticks", () => {
    let s = apply(initialState(T0), { t: "pipe", pipe: "dead" });
    s = apply(s, { t: "tick", now: T0 + 10 * STALE_MS });
    expect(s.pipe).toBe("dead");
  });

  it("a fresh feed event resets liveness, so the next tick reads live", () => {
    let s = initialState(T0);
    // a real event arrives at T0+? — it bumps lastEventAt to state.now
    s = { ...s, now: T0 + 100 };
    s = apply(s, { t: "problem.add", seq: 1, id: "x", problem: { sev: "warn", msg: "m", card: "c1", since: T0 } });
    // a tick shortly after stays live (lastEventAt was just refreshed)
    s = apply(s, { t: "tick", now: T0 + 100 + 10 });
    expect(s.pipe).toBe("live");
  });
});

// Layer 3 — the new activity fixtures are the wire-shape anchors for PR A's
// feeds (the live stream + saturation deltas). Replaying them through the same
// reducer the prod app uses keeps "tests without a backend" honest.
describe("feed contract: the new activity fixtures replay cleanly", () => {
  it("the stream fixture fills the card's ring buffer (in order)", () => {
    let s = apply(initialState(T0), snapshot as Event);
    // the snapshot has no i41; seed it so the stream events target a real card
    s = apply(s, {
      t: "snapshot", seq: 200,
      state: { cards: { ...s.cards, i41: card({ id: "i41", ref: "widgets#41" }) } },
    });
    for (const ev of streamFixture.events) s = apply(s, ev as Event);
    const stream = s.cards.i41!.stream!;
    expect(stream).toHaveLength(streamFixture.events.length);
    expect(stream[0]!.kind).toBe("think");
    expect(stream.at(-1)!.v).toContain("Committing step 3");
  });

  it("the saturation fixture sets then clears the lane badge", () => {
    let s = initialState(T0);
    const [set, clear] = satFixture.events;
    s = apply(s, set as Event);
    expect(s.lanes.find((l) => l.id === "implement")!.sat).toBe(2);
    s = apply(s, clear as Event);
    expect(s.lanes.find((l) => l.id === "implement")!.sat).toBe(0);
  });
});
