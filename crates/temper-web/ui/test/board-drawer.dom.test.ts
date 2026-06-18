// LAYER 2 — drawer + polish DOM tests (PR A). happy-dom (NO real/headless
// browser) via Testing Library, plus the FakeEventSource to PUSH server events.
// NO backend, NO network.
//
// Covers: the drill-down drawer opening for the clicked card (stream + ledger),
// Esc and scrim-click close, ticker-row click opening the referenced card's
// drawer, the saturation badge presence/absence, and the theme toggle.
//
// @vitest-environment happy-dom

import { describe, it, expect, beforeEach } from "vitest";
import { screen, within } from "@testing-library/dom";
import userEvent from "@testing-library/user-event";
import { createApp, type App } from "../src/app.js";
import { FakeEventSource } from "./fake-event-source.js";
import { type Card } from "../src/model.js";

const T0 = 1_000_000;
const card = (over: Partial<Card>): Card => ({
  id: "c1", lane: "implement", ref: "repo#1", role: "code", title: "t", enteredAt: T0, ...over,
});

let root: HTMLElement;
let feed: FakeEventSource;
let app: App;

beforeEach(() => {
  // include the static-shell theme button (index.html mounts it outside #root)
  document.documentElement.dataset.theme = "dark";
  document.body.innerHTML = `
    <header><button id="theme" aria-label="toggle theme">◐</button></header>
    <div id="root"></div>`;
  root = document.getElementById("root")!;
  feed = new FakeEventSource("/events");
  app = createApp({ root, eventSource: () => feed, now: () => T0 });
});

describe("drill-down drawer (UX §5)", () => {
  it("opens for the clicked card and shows its stream + ledger", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {
      c1: card({ id: "c1", ref: "r#1", steps: { done: 1, total: 3 } }),
    } } });
    // push a couple of live-activity lines into that card's ring buffer
    feed.push({ t: "card.stream", seq: 2, id: "c1", event: { kind: "think", v: "planning the fix" } });
    feed.push({ t: "card.stream", seq: 3, id: "c1", event: { kind: "tool", k: "bash", v: "cargo test" } });

    // drawer is closed until a card is opened
    expect(screen.getByLabelText("agent activity").getAttribute("data-open")).toBe("false");

    await userEvent.click(screen.getByLabelText("r#1: t"));
    const drawer = screen.getByLabelText("agent activity");
    expect(drawer.getAttribute("data-open")).toBe("true");
    expect(app.state.openCard).toBe("c1");

    // live activity stream shows the pushed lines
    const stream = within(drawer).getByLabelText("live activity");
    expect(stream.textContent).toContain("planning the fix");
    expect(stream.textContent).toContain("cargo test");

    // durable ledger shows 3 derived steps (1 done, 1 now, 1 todo)
    const steps = within(drawer).getByLabelText("steps");
    expect(steps.querySelectorAll("[data-step]")).toHaveLength(3);
    expect(steps.querySelector('[data-step="1"]')!.getAttribute("data-status")).toBe("done");
    expect(steps.querySelector('[data-step="2"]')!.getAttribute("data-status")).toBe("now");
    expect(steps.querySelector('[data-step="3"]')!.getAttribute("data-status")).toBe("todo");
  });

  it("opens the RIGHT drawer when two cards exist", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {
      c1: card({ id: "c1", ref: "r#1", title: "first" }),
      c2: card({ id: "c2", ref: "r#2", title: "second" }),
    } } });
    await userEvent.click(screen.getByLabelText("r#2: second"));
    expect(app.state.openCard).toBe("c2");
    expect(screen.getByLabelText("agent activity").textContent).toContain("r#2");
  });

  it("Escape closes the drawer", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1", ref: "r#1" }) } } });
    await userEvent.click(screen.getByLabelText("r#1: t"));
    expect(app.state.openCard).toBe("c1");
    await userEvent.keyboard("{Escape}");
    expect(app.state.openCard).toBeNull();
    expect(screen.getByLabelText("agent activity").getAttribute("data-open")).toBe("false");
  });

  it("clicking the scrim closes the drawer", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1", ref: "r#1" }) } } });
    await userEvent.click(screen.getByLabelText("r#1: t"));
    expect(app.state.openCard).toBe("c1");
    const scrim = root.querySelector('[data-region="scrim"]') as HTMLElement;
    await userEvent.click(scrim);
    expect(app.state.openCard).toBeNull();
  });

  it("the ✕ close button closes the drawer (and does not re-open it)", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1", ref: "r#1" }) } } });
    await userEvent.click(screen.getByLabelText("r#1: t"));
    await userEvent.click(within(screen.getByLabelText("agent activity")).getByLabelText("close"));
    expect(app.state.openCard).toBeNull();
  });
});

describe("problem ticker → drawer (UX §4.3)", () => {
  it("clicking a ticker row opens the referenced card's drawer", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {
      p6: card({ id: "p6", lane: "ci", ref: "PR#6", title: "scheduler", ci: "failed" }),
    } } });
    feed.push({ t: "problem.add", seq: 2, id: "p6-ci",
      problem: { sev: "bad", msg: "PR#6 CI failed: test_apply", card: "p6", since: T0 } });

    const ticker = screen.getByLabelText("problems");
    const row = within(ticker).getByRole("button");
    expect(row.textContent).toContain("CI failed");
    expect(row.textContent).toContain("open ▸");

    await userEvent.click(row);
    expect(app.state.openCard).toBe("p6"); // opened the card the problem points at
    expect(screen.getByLabelText("agent activity").textContent).toContain("PR#6");
  });

  it("enriched rows show a severity dot and an age", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: { c1: card({ id: "c1" }) } } });
    feed.push({ t: "problem.add", seq: 2, id: "x",
      problem: { sev: "warn", msg: "role saturated", card: "c1", since: T0 - 3 * 60_000 } });
    const row = within(screen.getByLabelText("problems")).getByRole("button");
    expect(row.querySelector(".sev")).toBeTruthy();
    expect(row.querySelector(".when")!.textContent).toBe("3m"); // 3 minutes old
  });
});

describe("saturation badge on lane headers (UX §4.3)", () => {
  it("renders when sat > 0", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {} } });
    expect(within(screen.getByLabelText("Implement")).queryByLabelText("saturation")).toBeNull();
    feed.push({ t: "role.saturated", seq: 2, lane: "implement", waiting: 3 });
    const badge = within(screen.getByLabelText("Implement")).getByLabelText("saturation");
    expect(badge.textContent).toContain("3 waiting · 0 slots");
  });

  it("is absent again once cleared (waiting:0)", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {} } });
    feed.push({ t: "role.saturated", seq: 2, lane: "review", waiting: 2 });
    expect(within(screen.getByLabelText("Review")).queryByLabelText("saturation")).toBeTruthy();
    feed.push({ t: "role.saturated", seq: 3, lane: "review", waiting: 0 });
    expect(within(screen.getByLabelText("Review")).queryByLabelText("saturation")).toBeNull();
  });
});

describe("pulse text (UX §4.2)", () => {
  it("shows 'disconnected' when the pipe is dead", () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {} } });
    feed.fail(); // SSE drop -> pipe dead
    const pulse = screen.getByTestId("pulse");
    expect(pulse.getAttribute("data-pipe")).toBe("dead");
    expect(pulse.textContent).toBe("disconnected");
  });
});

describe("theme toggle (DOM-only side effect, UX §4)", () => {
  it("flips document[data-theme] without touching model state", async () => {
    feed.push({ t: "snapshot", seq: 1, state: { cards: {} } });
    expect(document.documentElement.dataset.theme).toBe("dark");
    const before = JSON.stringify(app.state);

    await userEvent.click(screen.getByLabelText("toggle theme"));
    expect(document.documentElement.dataset.theme).toBe("light");
    // it is a pure DOM side effect — the model is unchanged
    expect(JSON.stringify(app.state)).toBe(before);

    await userEvent.click(screen.getByLabelText("toggle theme"));
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});
