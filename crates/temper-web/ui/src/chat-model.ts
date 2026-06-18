// MODEL + UPDATE for the chat page. Pure data-to-data, no DOM.
// Types mirror the REAL wire contract (crates/temper-protocol-interaction +
// crates/temper-interaction/src/transport.rs): snake_case enums, and a
// ConversationEventPayload that is INTERNALLY TAGGED on "type".

export type ParticipantKind = "human" | "agent" | "system"; // serde snake_case

export interface Participant {
  kind: ParticipantKind;
  display_name?: string;
}

export interface ConversationTurn {
  id?: string;
  participant: Participant;
  body: string;
}

export interface Proposal {
  id: string; // ProposalId (slug string)
  kind: string; // ProposalKind, e.g. "issue"
  title: string;
  summary?: string;
  payload: unknown; // IssueProposal { title, body, rationale? } for kind "issue"
}

export interface ConversationReply {
  message: string;
  proposals: Proposal[]; // #[serde(default)] => may be []
}

export interface AcceptedProposalTarget {
  kind: string;
  number?: number;
  url?: string;
  title?: string;
}

export interface ConversationTranscriptRef {
  issue_number?: number;
  url?: string;
}

// Internally tagged on "type" — exactly as serde emits ConversationEventPayload.
export type ConversationEventPayload =
  | { type: "conversation_opened"; profile_id: string; transcript?: ConversationTranscriptRef }
  | { type: "human_turn_appended"; turn: ConversationTurn }
  | { type: "agent_reply_appended"; reply: ConversationReply }
  | { type: "proposals_updated"; proposals: Proposal[] }
  | { type: "proposal_accepted"; proposal_id: string; created: boolean; target?: AcceptedProposalTarget };

// The full ConversationEvent envelope as it arrives over SSE.
export interface ConversationEvent {
  sequence: number;
  conversation_id: string;
  // `kind` duplicates the payload tag in the real type; the reducer keys off
  // payload.type, so we keep kind optional/ignored here.
  kind?: string;
  occurred_at?: string;
  payload: ConversationEventPayload;
}

export interface Conversation {
  id: string;
  profile_id: string;
  transcript: { issue_number: number; url: string };
  turns: ConversationTurn[];
  proposals: Proposal[]; // latest_proposals
  accepted: Record<string, AcceptedProposalTarget | { created: boolean }>;
  agentTyping: boolean;
  cursor: number; // last applied sequence (dedup / resume)
}

export interface ChatState {
  conversations: Record<string, Conversation>;
  active: string | null;
  toast: { number: number; title?: string } | null;
}

// Reducer events: a feed ConversationEvent, plus local UI actions.
export type ChatEvent =
  | { t: "conv.event"; event: ConversationEvent }
  | { t: "agent.typing"; id: string; on: boolean }
  | { t: "select"; id: string }
  | { t: "toast.clear" };

export function emptyChat(): ChatState {
  return { conversations: {}, active: null, toast: null };
}

export function seedConversation(c: Partial<Conversation> & { id: string; profile_id: string }): Conversation {
  return {
    transcript: { issue_number: 0, url: "" },
    turns: [],
    proposals: [],
    accepted: {},
    agentTyping: false,
    cursor: 0,
    ...c,
  };
}

// ── UPDATE: single mutation path; returns a NEW state (pure). ────────────────
export function applyChat(state: ChatState, ev: ChatEvent): ChatState {
  switch (ev.t) {
    case "select":
      return { ...state, active: ev.id };
    case "toast.clear":
      return { ...state, toast: null };
    case "agent.typing": {
      const c = state.conversations[ev.id];
      if (!c) return state;
      return { ...state, conversations: { ...state.conversations, [ev.id]: { ...c, agentTyping: ev.on } } };
    }
    case "conv.event":
      return applyConvEvent(state, ev.event);
  }
}

function applyConvEvent(state: ChatState, event: ConversationEvent): ChatState {
  const c = state.conversations[event.conversation_id];
  if (!c) return state; // unknown conversation => no-op
  if (event.sequence <= c.cursor) return state; // dedup/out-of-order => no-op

  let next: Conversation = { ...c, cursor: event.sequence };
  let toast = state.toast;
  const p = event.payload;

  switch (p.type) {
    case "conversation_opened":
      // already seeded; nothing to mutate in this minimal model
      break;
    case "human_turn_appended":
      next = { ...next, turns: [...c.turns, p.turn] };
      break;
    case "agent_reply_appended":
      next = {
        ...next,
        agentTyping: false,
        turns: [...c.turns, { participant: { kind: "agent", display_name: c.profile_id }, body: p.reply.message }],
        proposals: p.reply.proposals.length ? p.reply.proposals : c.proposals,
      };
      break;
    case "proposals_updated":
      next = { ...next, proposals: p.proposals };
      break;
    case "proposal_accepted":
      next = { ...next, accepted: { ...c.accepted, [p.proposal_id]: p.target ?? { created: p.created } } };
      // the cross-surface signal: an accepted issue now exists on the board
      if (p.target?.number != null) toast = { number: p.target.number, title: p.target.title };
      break;
  }

  return { ...state, conversations: { ...state.conversations, [event.conversation_id]: next }, toast };
}

// ── derivation: is a proposal accepted? (tested directly) ────────────────────
export function isAccepted(c: Conversation, proposalId: string): boolean {
  return proposalId in c.accepted;
}
