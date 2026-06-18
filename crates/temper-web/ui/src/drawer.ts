// DRAWER — the drill-down slide-over (Agent Activity view, UX §5). Pure
// projection: render(state) -> drawer markup. No business logic, no feed access.
//
// Two registers stacked (UX §5):
//   • a LIVE ACTIVITY STREAM — the ephemeral, bounded ring buffer of agent
//     activity (think / text / tool), the "watch it think" view; and
//   • a DURABLE STEP LEDGER — the StepProgress checklist derived from
//     `steps {done,total}`, the "what has it accomplished" view.
//
// Markup/classes are ported from the mockup's `.drawer` / `.scrim` and
// renderDrawer (docs/plans/temper-web-mockup.html). Tested in Layer 2 via
// Testing Library; we add aria/role hooks so queries read like a user.

import { ledger, type Card, type State } from "./model.js";

const esc = (s: string) =>
  s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

// Human labels for the step ledger — the durable checklist mirrors the steps an
// agent posts to the Forge issue (UX §5). Generic fallback past the named set.
const STEP_LABELS = ["write failing test", "implement", "run tests", "open PR"];

export function renderDrawer(state: State): string {
  const c = state.openCard ? state.cards[state.openCard] : null;
  const open = !!c;
  return `
    <div data-region="scrim" data-open="${open}" class="scrim ${open ? "open" : ""}"></div>
    <aside data-region="drawer" data-open="${open}"
           role="dialog" aria-modal="true" aria-label="agent activity"
           aria-hidden="${open ? "false" : "true"}"
           class="drawer ${open ? "open" : ""}">
      ${c ? drawerBody(c) : ""}
    </aside>`;
}

function drawerBody(c: Card): string {
  return `
    <div class="drawer-head">
      <div style="flex:1">
        <span class="ref">${esc(c.ref)}</span>
        <div class="sub">role:${esc(c.role)} · ${c.activity ? "● streaming" : "idle"}</div>
      </div>
      <button class="iconbtn" data-close aria-label="close">✕</button>
    </div>
    <div class="drawer-body">
      <p class="section-label">Live activity</p>
      <div class="stream" aria-label="live activity">${streamView(c)}</div>
      <p class="section-label">Steps</p>
      <div class="ledger" aria-label="steps">${ledgerView(c)}</div>
    </div>`;
}

// the ephemeral ring buffer (newest last, as appended). Empty => a quiet hint.
function streamView(c: Card): string {
  const stream = c.stream ?? [];
  if (stream.length === 0) {
    return `<div class="ev quiet"><span class="v">no live activity yet</span></div>`;
  }
  return stream
    .map(
      (e) => `
    <div class="ev ${e.kind}">
      <span class="k">${esc(e.k ?? e.kind)}</span>
      <span class="v">${esc(e.v)}</span>
    </div>`,
    )
    .join("");
}

// the durable checklist derived from steps {done,total} (UX §5).
function ledgerView(c: Card): string {
  const steps = ledger(c);
  if (steps.length === 0) {
    return `<div class="li quiet"><span class="lbl">no steps recorded</span></div>`;
  }
  return steps
    .map((s) => {
      const box = s.status === "done" ? "[x]" : s.status === "now" ? "[>]" : "[ ]";
      const label = STEP_LABELS[s.n - 1] ?? "step";
      const cls = s.status === "todo" ? "" : s.status;
      return `<div class="li ${cls}" data-step="${s.n}" data-status="${s.status}">
        <span class="box">${box}</span>
        <span class="lbl">${s.n} ${esc(label)}</span>
      </div>`;
    })
    .join("");
}
