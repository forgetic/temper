// VIEW — render(state) -> DOM. Pure projection, no business logic, no feed access.
// Tested in Layer 2 against happy-dom (Appendix B.1). Querying is done by the
// test via Testing Library, so we add a few aria/role/text hooks here.

import { cardState, isStuck, laneCards, stuckCount, type State, type Lane, type Card } from "./model.js";

const LANES: { id: Lane; title: string }[] = [
  { id: "triage", title: "Triage" },
  { id: "implement", title: "Implement" },
  { id: "review", title: "Review" },
  { id: "ci", title: "CI / Gates" },
  { id: "done", title: "Done" },
];

const MIN = 60_000;
const esc = (s: string) =>
  s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

export function render(root: HTMLElement, state: State): void {
  root.innerHTML = `
    ${renderHealth(state)}
    <main role="main" aria-label="board">${LANES.map((l) => renderLane(state, l)).join("")}</main>
    ${renderTicker(state)}
  `;
}

function renderHealth(state: State): string {
  const inFlight = Object.values(state.cards).filter(
    (c) => c.lane === "implement" || c.lane === "review",
  ).length;
  const stuck = stuckCount(state);
  return `
    <header>
      <span data-testid="pulse" data-pipe="${state.pipe}">${state.pipe}</span>
      <span role="status" aria-label="workers">${state.workers.healthy}/${state.workers.total}</span>
      <span role="status" aria-label="in flight">${inFlight}</span>
      <span role="status" aria-label="stuck" data-stuck="${stuck}">${stuck}</span>
    </header>`;
}

function renderLane(state: State, lane: { id: Lane; title: string }): string {
  const cards = laneCards(state, lane.id);
  return `
    <section aria-label="${lane.title}">
      <h2>${esc(lane.title)} <span aria-label="count">${cards.length}</span></h2>
      <div class="lane-body">${cards.map((c) => cardView(c, state.now)).join("")}</div>
    </section>`;
}

function cardView(c: Card, now: number): string {
  const st = cardState(c, now);
  const ageMin = Math.floor((now - c.enteredAt) / MIN);
  const ageTxt = c.merged ? "merged" : ageMin < 1 ? "just now" : `${ageMin}m in stage`;
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
  return `
    <article role="button" tabindex="0"
             data-card="${c.id}"
             aria-label="${esc(c.ref)}: ${esc(c.title)}"
             class="card state-${st} ${isStuck(c, now) ? "stuck" : ""}">
      <span class="ref">${esc(c.ref)}</span>
      <span class="badge">${esc(c.role)}</span>
      <span class="title">${esc(c.title)}</span>
      <span class="activity">${esc(activity)}</span>
      <span class="age">${ageTxt}</span>
    </article>`;
}

function renderTicker(state: State): string {
  const probs = Object.entries(state.problems).sort((a, b) => a[1].since - b[1].since);
  if (probs.length === 0) {
    return `<section aria-label="problems" data-clear="true">all clear — no problems</section>`;
  }
  return `<section aria-label="problems" data-clear="false">${probs
    .map(
      ([, p]) =>
        `<div role="button" data-card="${esc(p.card)}" class="prob ${p.sev}">${esc(p.msg)}</div>`,
    )
    .join("")}</section>`;
}
