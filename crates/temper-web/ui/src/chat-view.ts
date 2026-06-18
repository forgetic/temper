// VIEW for the chat page — render(state) -> DOM. Pure projection, no feed access.
// Adds aria/text hooks so Layer-2 tests query like a user (Testing Library).

import type { ChatState, Conversation, ConversationTurn, Proposal } from "./chat-model.js";

const esc = (s: string) =>
  s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

export function renderChat(root: HTMLElement, state: ChatState): void {
  const active = state.active ? state.conversations[state.active] : undefined;
  root.innerHTML = `
    <aside role="navigation" aria-label="conversations">
      ${Object.values(state.conversations).map((c) => railItem(c, c.id === state.active)).join("")}
    </aside>
    <main role="main" aria-label="transcript">
      ${active ? pane(active) : `<p>no conversation selected</p>`}
    </main>
    ${toast(state)}
  `;
}

function railItem(c: Conversation, current: boolean): string {
  const preview = c.turns.at(-1)?.body ?? "—";
  return `
    <div role="button" data-select="${esc(c.id)}" aria-current="${current}" aria-label="conversation ${esc(c.id)}">
      <span class="profile">${esc(c.profile_id)}</span>
      <span class="preview">${esc(preview)}</span>
    </div>`;
}

function pane(c: Conversation): string {
  const turns = c.turns.map(turnView).join("");
  const typing = c.agentTyping ? `<div data-testid="typing" aria-label="agent typing">…</div>` : "";
  const proposals = c.proposals.map((p) => proposalView(p, p.id in c.accepted)).join("");
  return `
    <header aria-label="transcript header">${esc(c.profile_id)} · issue #${c.transcript.issue_number}</header>
    <div data-testid="turns">${turns}${typing}${proposals}</div>`;
}

function turnView(t: ConversationTurn): string {
  const who = t.participant.display_name ?? t.participant.kind;
  return `
    <div class="turn ${t.participant.kind}" data-role="${t.participant.kind}" aria-label="${esc(who)} turn">
      <span class="meta">${esc(who)}</span>
      <div class="bubble">${esc(t.body)}</div>
    </div>`;
}

function proposalView(p: Proposal, accepted: boolean): string {
  const body = (p.payload as { body?: string })?.body;
  const foot = accepted
    ? `<div role="status" aria-label="proposal outcome">✓ accepted</div>`
    : `<button data-accept="${esc(p.id)}" aria-label="accept ${esc(p.title)}">Accept</button>`;
  return `
    <div class="proposal ${accepted ? "accepted" : ""}" aria-label="proposal ${esc(p.title)}">
      <span class="kind">${esc(p.kind)}</span>
      <span class="ptitle">${esc(p.title)}</span>
      ${p.summary ? `<p class="summary">${esc(p.summary)}</p>` : ""}
      ${body ? `<pre class="draft">${esc(body)}</pre>` : ""}
      ${foot}
    </div>`;
}

function toast(state: ChatState): string {
  if (!state.toast) return "";
  return `<div role="alert" data-testid="toast">issue #${state.toast.number} created — now in the pipeline</div>`;
}
