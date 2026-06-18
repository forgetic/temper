// LAYER 2 — view + interaction tests. Uses happy-dom (NO real/headless browser)
// driven through Testing Library, plus the FakeEventSource to PUSH server events.
// NO backend, NO network.
//
// @vitest-environment happy-dom

import { describe, it, expect, beforeEach } from "vitest";
import { screen, within } from "@testing-library/dom";
import userEvent from "@testing-library/user-event";
import { createApp, type App } from "../src/app.js";
import { FakeEventSource } from "./fake-event-source.js";
import { MIN, type Card } from "../src/model.js";
import snapshot from "../fixtures/state-snapshot.json" with { type: "json" };

const T0 = 1_000_000;
const card = (over: Partial<Card>): Card => ({
  id: "c1", lane: "implement", ref: "repo#1", role: "code", title: "t", enteredAt: T0, ...over,
});

// Fixtures store ages as offsets from 0; re-base every card's enteredAt onto the
// test clock so derived "age in stage" is deterministic and relative.
function rebaseAges(snap: any, now: number): any {
  const cards: Record<string, any> = {};
  for (const [id, c] of Object.entries<any>(snap.state.cards)) {
    cards[id] = { ...c, enteredAt: now + c.enteredAt };
  }
  return { ...snap, state: { ...snap.state, cards } };
}

let root: HTMLElement;
let feed: FakeEventSource;
let app: App;

beforeEach(() => {
  document.body.innerHTML = `<div id="root"></div>`;
  root = document.getElementById("root")!;
  feed = new FakeEventSource("/events");
  // INJECT the fake (Appendix B.3, seam 1). `now` is fixed so age is deterministic.
  app = createApp({ root, eventSource: () => feed, now: () => T0 });
});

describe("server events render the board (push via the fake feed)", () => {
  it("hydrates from the snapshot fixture", () => {
    // The fixture stores ages as `enteredAt: 0`; re-base them to the test clock
    // so "age in stage" is relative to now() (the real read-model carries live
    // timestamps, not zero). Without this, every card reads as ~16m old.
    feed.push(rebaseAges(snapshot, T0));
    // a card from the fixture is on screen, queried like a user would
    expect(screen.getByLabelText("widgets#42: Add CSV export")).toBeTruthy();
    // only p6 (CI failed) is a problem; the freshly-aged cards are not stuck
    expect(screen.getByLabelText("stuck").getAttribute("data-stuck")).toBe("0");
  });

  it("a card that ages past the threshold gets the 'stuck' class and floats up", () => {
    // age is measured against now()=T0, so seed enteredAt in the past
    feed.push({ t: "snapshot", seq: 1, state: { cards: {
      fresh: card({ id: "fresh", ref: "r#fresh", enteredAt: T0 }),
      old: card({ id: "old", ref: "r#old", enteredAt: T0 - 30 * MIN }),
    } } });
    const lane = screen.getByLabelText("Implement");
    const [first, second] = within(lane).getAllByRole("button");
    expect(first!.getAttribute("data-card")).toBe("old"); // stuck sorts first
    expect(first!.className).toContain("stuck");
    expect(second!.className).not.toContain("stuck");
  });

  it("an incoming card.move animates the card into the next lane", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1", ref: "r#1" }) } } });
    expect(within(screen.getByLabelText("Implement")).queryByLabelText(/r#1/)).toBeTruthy();
    feed.push({ t: "card.move", seq: 2, id: "c1", lane: "review", now: T0 });
    expect(within(screen.getByLabelText("Implement")).queryByLabelText(/r#1/)).toBeNull();
    expect(within(screen.getByLabelText("Review")).queryByLabelText(/r#1/)).toBeTruthy();
  });

  it("empty problem set renders the 'all clear' line; a problem replaces it", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {} } });
    const ticker = () => screen.getByLabelText("problems");
    expect(ticker().getAttribute("data-clear")).toBe("true");
    expect(ticker().textContent).toContain("all clear");
    feed.push({ t: "problem.add", seq: 2, id: "x",
      problem: { sev: "bad", msg: "worker w-2 lease lost", card: "c1", since: T0 } });
    expect(ticker().getAttribute("data-clear")).toBe("false");
    expect(ticker().textContent).toContain("lease lost");
  });

  it("losing the connection flips the pulse to dead", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {} } });
    expect(screen.getByTestId("pulse").getAttribute("data-pipe")).toBe("live");
    feed.fail(); // simulate SSE drop -> onerror -> apply({pipe:'dead'})
    expect(screen.getByTestId("pulse").getAttribute("data-pipe")).toBe("dead");
  });
});

describe("user interactions funnel through the reducer", () => {
  it("clicking a card opens the drawer for THAT id", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1", ref: "r#1" }) } } });
    await userEvent.click(screen.getByLabelText("r#1: t"));
    expect(app.state.openCard).toBe("c1");
  });

  it("Escape closes the drawer", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1" }) } } });
    await userEvent.click(screen.getByLabelText(/repo#1/));
    expect(app.state.openCard).toBe("c1");
    await userEvent.keyboard("{Escape}");
    expect(app.state.openCard).toBeNull();
  });
});
