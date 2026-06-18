// VIEW — render(state) -> DOM. Pure projection, no business logic, no feed access.
// Tested in Layer 2 against happy-dom (Appendix B.1). Querying is done by the
// test via Testing Library, so we add a few aria/role/text hooks here.

import {
  cardState,
  isStuck,
  laneCards,
  stuckCount,
  type State,
  type Lane,
  type Card,
  type LaneState,
} from "./model.js";
import { renderDrawer } from "./drawer.js";

const MIN = 60_000;
const esc = (s: string) =>
  s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

export function render(root: HTMLElement, state: State): void {
  root.innerHTML = `
    ${renderHealth(state)}
    <main role="main" aria-label="board">${state.lanes.map((l) => renderLane(state, l)).join("")}</main>
    ${renderTicker(state)}
    ${renderDrawer(state)}
  `;
}

function renderHealth(state: State): string {
  const inFlight = Object.values(state.cards).filter(
    (c) => c.lane === "implement" || c.lane === "review",
  ).length;
  const stuck = stuckCount(state);
  // pulse text mirrors the mockup: live system shows "live"; a stale pipe shows
  // how long since the last event; a dead pipe shows "disconnected".
  const ago = Math.round((state.now - state.lastEventAt) / 1000);
  const pulseText =
    state.pipe === "live" ? "live" : state.pipe === "stale" ? `last event ${ago}s ago` : "disconnected";
  return `
    <header>
      <span data-testid="pulse" data-pipe="${state.pipe}">${esc(pulseText)}</span>
      <span role="status" aria-label="workers">${state.workers.healthy}/${state.workers.total}</span>
      <span role="status" aria-label="in flight">${inFlight}</span>
      <span role="status" aria-label="stuck" data-stuck="${stuck}">${stuck}</span>
    </header>`;
}

function renderLane(state: State, lane: LaneState): string {
  const cards = laneCards(state, lane.id);
  // saturation badge: work waiting with no free capacity (UX §4.3). Absent at 0.
  const sat = lane.sat > 0
    ? ` <span class="sat" aria-label="saturation">${lane.sat} waiting · 0 slots</span>`
    : "";
  return `
    <section aria-label="${lane.title}">
      <h2>${esc(lane.title)} <span aria-label="count">${cards.length}</span>${sat}</h2>
      <div class="lane-body">${cards.map((c) => cardView(c, state.now)).join("")}</div>
    </section>`;
}

function cardView(c: Card, now: number): string {
  const st = cardState(c, now);
  const ageMin = Math.floor((now - c.enteredAt) / MIN);
  const ageTxt = c.merged ? "merged" : ageMin < 1 ? "just now" : `${ageMin}m in stage`;
  const ageCls = st === "bad" ? "bad" : st === "warn" ? "warn" : "";
  const activity = c.merged
    ? "✓ merged"
    : c.ci === "failed"
      ? "✗ CI failed"
      : c.ci === "running"
        ? "running CI…"
        : c.activity?.kind === "think"
          ? c.activity.text
          : c.activity?.kind === "tool"
            ? `▸ ${c.activity.text}`
            : "queued";
  const steps = c.steps
    ? `<span class="steps" aria-label="steps">
         <span class="bar"><i style="width:${Math.round((100 * c.steps.done) / c.steps.total)}%"></i></span>
         <span class="frac">${c.steps.done}/${c.steps.total}</span>
       </span>`
    : "";
  return `
    <article role="button" tabindex="0"
             data-card="${c.id}"
             aria-label="${esc(c.ref)}: ${esc(c.title)}"
             class="card state-${st} ${isStuck(c, now) ? "stuck" : ""}">
      <span class="ref">${esc(c.ref)}</span>
      <span class="badge role-${esc(c.role)}">${esc(c.role)}</span>
      <span class="title">${esc(c.title)}</span>
      <span class="activity">${esc(activity)}</span>
      ${steps}
      <span class="age ${ageCls}">${ageTxt}</span>
    </article>`;
}

function renderTicker(state: State): string {
  const probs = Object.entries(state.problems).sort((a, b) => a[1].since - b[1].since);
  if (probs.length === 0) {
    return `<section aria-label="problems" data-clear="true"><span class="dot"></span><span>all clear — no problems</span></section>`;
  }
  return `<section aria-label="problems" data-clear="false">${probs
    .map(([, p]) => {
      // age of the problem in minutes (mockup: "Nm"); 0m when fresh.
      const ago = Math.max(0, Math.round((state.now - p.since) / MIN));
      return `<div role="button" data-card="${esc(p.card)}" class="prob ${p.sev}">
        <span class="sev"></span>
        <span class="msg">${esc(p.msg)}</span>
        <span class="when">${ago}m</span>
        <span class="go">open ▸</span>
      </div>`;
    })
    .join("")}</section>`;
}
