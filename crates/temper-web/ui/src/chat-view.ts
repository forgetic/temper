// VIEW for the chat page — render(state) -> DOM. Pure projection, no feed access.
// Adds aria/text hooks so Layer-2 tests query like a user (Testing Library).

import type {
  ChatState,
  Conversation,
  ConversationTurn,
  Proposal,
  AcceptedProposalTarget,
} from "./chat-model.js";

// The accepted map stores either a full target (issue created) or a bare
// `{ created }` flag; the outcome renders whatever numbered fields it has.
type AcceptedTarget = Partial<AcceptedProposalTarget>;

const esc = (s: string) =>
  s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

export function renderChat(root: HTMLElement, state: ChatState): void {
  const active = state.active ? state.conversations[state.active] : undefined;
  root.innerHTML = `
    <aside class="rail" role="navigation" aria-label="conversations">
      <div class="rail-head">
        <button class="newbtn" data-new aria-label="new conversation">+ new conversation</button>
      </div>
      <div class="convos">
        ${Object.values(state.conversations).map((c) => railItem(c, c.id === state.active)).join("")}
      </div>
    </aside>
    <main class="pane" role="main" aria-label="transcript">
      ${active ? pane(active) : `<p>no conversation selected</p>`}
    </main>
    ${toast(state)}
  `;
}

function railItem(c: Conversation, current: boolean): string {
  const preview = c.turns.at(-1)?.body ?? "—";
  return `
    <div class="convo" role="button" data-select="${esc(c.id)}" aria-current="${current}" aria-label="conversation ${esc(c.id)}">
      <span class="profile">${esc(c.profile_id)}</span>
      <span class="preview">${esc(preview)}</span>
    </div>`;
}

function pane(c: Conversation): string {
  const turns = c.turns.map(turnView).join("");
  const typing = c.agentTyping
    ? `<div class="typing" data-testid="typing" aria-label="agent typing"><i></i><i></i><i></i></div>`
    : "";
  const proposals = c.proposals.map((p) => proposalView(p, c.accepted[p.id] as AcceptedTarget | undefined)).join("");
  return `
    <header class="pane-head" aria-label="transcript header">${esc(c.profile_id)} · issue #${c.transcript.issue_number}</header>
    <div class="transcript" data-testid="turns">${turns}${typing}${proposals}</div>
    <div class="composer">
      <textarea data-input aria-label="message" placeholder="Message the ${esc(c.profile_id)} agent…" rows="1"></textarea>
      <button class="send" data-send aria-label="send">Send</button>
    </div>`;
}

function turnView(t: ConversationTurn): string {
  const who = t.participant.display_name ?? t.participant.kind;
  return `
    <div class="turn ${t.participant.kind}" data-role="${t.participant.kind}" aria-label="${esc(who)} turn">
      <span class="meta">${esc(who)}</span>
      <div class="bubble">${esc(t.body)}</div>
    </div>`;
}

function proposalView(p: Proposal, accepted: AcceptedTarget | undefined): string {
  const body = (p.payload as { body?: string })?.body;
  const foot = accepted
    ? `<div class="outcome" role="status" aria-label="proposal outcome">✓ accepted → created
         <a href="${esc(accepted.url ?? "#")}">${esc(accepted.kind ?? "issue")} #${esc(String(accepted.number ?? "—"))}</a></div>`
    : `<button class="accept" data-accept="${esc(p.id)}" aria-label="accept ${esc(p.title)}">Accept</button>`;
  return `
    <div class="proposal ${accepted ? "accepted" : ""}" aria-label="proposal ${esc(p.title)}">
      <div class="phead">
        <span class="kind">${esc(p.kind)}</span>
        <span class="ptitle">${esc(p.title)}</span>
      </div>
      <div class="pbody">
        ${p.summary ? `<p class="summary">${esc(p.summary)}</p>` : ""}
        ${body ? `<pre class="draft">${esc(body)}</pre>` : ""}
      </div>
      <div class="pfoot">${foot}</div>
    </div>`;
}

function toast(state: ChatState): string {
  if (!state.toast) return "";
  return `<div class="toast show" role="alert" data-testid="toast">
    <span>✓ issue #${state.toast.number} created — now in the pipeline</span>
    <a href="./index.html" data-board-link>view board →</a>
  </div>`;
}
