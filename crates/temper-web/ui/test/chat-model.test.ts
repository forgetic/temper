// LAYER 1 + 3 — chat reducer + feed-contract tests. Pure, no DOM, no backend.

import { describe, it, expect } from "vitest";
import {
  applyChat,
  emptyChat,
  seedConversation,
  isAccepted,
  type ChatState,
  type ConversationEvent,
} from "../src/chat-model.js";
import eventsFixture from "../fixtures/conversation-events.json" with { type: "json" };

function withConvo(): ChatState {
  return {
    ...emptyChat(),
    active: "conversation-1",
    conversations: {
      "conversation-1": seedConversation({
        id: "conversation-1",
        profile_id: "product-manager",
        transcript: { issue_number: 88, url: "u" },
      }),
    },
  };
}

const evt = (sequence: number, payload: ConversationEvent["payload"]): { t: "conv.event"; event: ConversationEvent } => ({
  t: "conv.event",
  event: { sequence, conversation_id: "conversation-1", payload },
});

describe("chat reducer", () => {
  it("appends a human turn", () => {
    const s = applyChat(withConvo(), evt(1, { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "hi" } }));
    expect(s.conversations["conversation-1"]!.turns).toHaveLength(1);
    expect(s.conversations["conversation-1"]!.turns[0]!.body).toBe("hi");
  });

  it("agent reply clears typing, appends a turn, and surfaces proposals", () => {
    let s = applyChat(withConvo(), { t: "agent.typing", id: "conversation-1", on: true });
    expect(s.conversations["conversation-1"]!.agentTyping).toBe(true);
    s = applyChat(s, evt(1, {
      type: "agent_reply_appended",
      reply: { message: "ok", proposals: [{ id: "p1", kind: "issue", title: "T", payload: {} }] },
    }));
    const c = s.conversations["conversation-1"]!;
    expect(c.agentTyping).toBe(false);
    expect(c.turns.at(-1)).toMatchObject({ participant: { kind: "agent" }, body: "ok" });
    expect(c.proposals).toHaveLength(1);
  });

  it("an empty proposals[] in a reply does not wipe existing proposals", () => {
    let s = applyChat(withConvo(), evt(1, {
      type: "agent_reply_appended",
      reply: { message: "draft", proposals: [{ id: "p1", kind: "issue", title: "T", payload: {} }] },
    }));
    s = applyChat(s, evt(2, { type: "agent_reply_appended", reply: { message: "more", proposals: [] } }));
    expect(s.conversations["conversation-1"]!.proposals).toHaveLength(1); // preserved
  });

  it("accepting a proposal records the target and raises the board toast", () => {
    const s = applyChat(withConvo(), evt(1, {
      type: "proposal_accepted",
      proposal_id: "csv-export",
      created: true,
      target: { kind: "issue", number: 142, title: "Add CSV export" },
    }));
    expect(isAccepted(s.conversations["conversation-1"]!, "csv-export")).toBe(true);
    expect(s.toast).toEqual({ number: 142, title: "Add CSV export" }); // cross-surface signal
  });

  it("ignores events for unknown conversations (no throw)", () => {
    const s = applyChat(emptyChat(), evt(1, { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "x" } }));
    expect(s.conversations).toEqual({});
  });

  it("drops duplicate/out-of-order events by sequence (idempotent stream)", () => {
    let s = applyChat(withConvo(), evt(5, { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "first" } }));
    // replay an older seq -> no-op
    s = applyChat(s, evt(3, { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "stale" } }));
    expect(s.conversations["conversation-1"]!.turns).toHaveLength(1);
    // exact-same seq -> no-op (no dup)
    s = applyChat(s, evt(5, { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "dup" } }));
    expect(s.conversations["conversation-1"]!.turns).toHaveLength(1);
  });

  it("selecting a conversation flips active without touching transcripts", () => {
    const base = {
      ...withConvo(),
      conversations: {
        ...withConvo().conversations,
        "conversation-2": seedConversation({ id: "conversation-2", profile_id: "product-manager", transcript: { issue_number: 9, url: "u" } }),
      },
    };
    const s = applyChat(base, { t: "select", id: "conversation-2" });
    expect(s.active).toBe("conversation-2");
    expect(s.conversations["conversation-1"]!.turns).toHaveLength(0); // untouched
  });

  it("agent.typing sets and clears the per-conversation flag", () => {
    let s = applyChat(withConvo(), { t: "agent.typing", id: "conversation-1", on: true });
    expect(s.conversations["conversation-1"]!.agentTyping).toBe(true);
    s = applyChat(s, { t: "agent.typing", id: "conversation-1", on: false });
    expect(s.conversations["conversation-1"]!.agentTyping).toBe(false);
  });

  it("toast.clear drops the cross-surface toast", () => {
    let s = applyChat(withConvo(), evt(1, {
      type: "proposal_accepted", proposal_id: "csv-export", created: true,
      target: { kind: "issue", number: 142, title: "Add CSV export" },
    }));
    expect(s.toast).not.toBeNull();
    s = applyChat(s, { t: "toast.clear" });
    expect(s.toast).toBeNull();
  });

  it("an optimistic human turn appends locally WITHOUT advancing the cursor", () => {
    // the optimistic echo shows the sent turn immediately...
    let s = applyChat(withConvo(), { t: "human.optimistic", id: "conversation-1", body: "send me" });
    const c1 = s.conversations["conversation-1"]!;
    expect(c1.turns.at(-1)).toMatchObject({ participant: { kind: "human", display_name: "you" }, body: "send me" });
    expect(c1.cursor).toBe(0); // no server sequence consumed
    // ...so the server's own sequenced human_turn_appended still applies.
    s = applyChat(s, evt(1, { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "from server" } }));
    expect(s.conversations["conversation-1"]!.turns).toHaveLength(2);
    expect(s.conversations["conversation-1"]!.cursor).toBe(1);
  });

  it("ignores an optimistic turn for an unknown conversation (no throw)", () => {
    const s = applyChat(emptyChat(), { t: "human.optimistic", id: "nope", body: "x" });
    expect(s.conversations).toEqual({});
  });
});

describe("feed contract (Layer 3): the real /events shape replays cleanly", () => {
  it("replays the fixture event log into the expected end state", () => {
    let s = withConvo();
    for (const event of eventsFixture.events as ConversationEvent[]) {
      s = applyChat(s, { t: "conv.event", event });
    }
    const c = s.conversations["conversation-1"]!;
    // human + agent turns landed in order
    expect(c.turns.map((t) => t.participant.kind)).toEqual(["human", "agent"]);
    expect(c.cursor).toBe(4);
    // proposal surfaced then accepted; board toast carries the created issue
    expect(isAccepted(c, "csv-export")).toBe(true);
    expect(s.toast?.number).toBe(142);
  });

  it("every fixture payload uses a known internally-tagged type", () => {
    const known = new Set(["conversation_opened", "human_turn_appended", "agent_reply_appended", "proposals_updated", "proposal_accepted"]);
    for (const e of eventsFixture.events) {
      expect(known.has((e.payload as { type: string }).type)).toBe(true);
    }
  });
});
