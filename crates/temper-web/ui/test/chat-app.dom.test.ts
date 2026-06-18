// LAYER 2 — chat view + interaction tests. happy-dom + Testing Library + the
// shared FakeEventSource. NO browser, NO backend.
//
// @vitest-environment happy-dom

import { describe, it, expect, beforeEach, vi } from "vitest";
import { screen, within } from "@testing-library/dom";
import userEvent from "@testing-library/user-event";
import { createChatApp, type ChatApp } from "../src/chat-app.js";
import { FakeEventSource } from "./fake-event-source.js";
import { emptyChat, seedConversation, type ChatState, type ConversationEvent } from "../src/chat-model.js";
import eventsFixture from "../fixtures/conversation-events.json" with { type: "json" };

// FakeEventSource is generic over the board's Event; conversation events go
// through the same JSON push path, so we adapt with a tiny cast helper.
function pushConv(feed: FakeEventSource, event: ConversationEvent): void {
  feed.onmessage?.({ data: JSON.stringify(event) });
}

function seed(): ChatState {
  return {
    ...emptyChat(),
    active: "conversation-1",
    conversations: {
      "conversation-1": seedConversation({ id: "conversation-1", profile_id: "product-manager", transcript: { issue_number: 88, url: "u" } }),
      "conversation-2": seedConversation({ id: "conversation-2", profile_id: "product-manager", transcript: { issue_number: 73, url: "u" } }),
    },
  };
}

let root: HTMLElement;
let feed: FakeEventSource;
let app: ChatApp;
let postAccept: ReturnType<typeof vi.fn>;
let postTurn: ReturnType<typeof vi.fn>;
let postNewConversation: ReturnType<typeof vi.fn>;
// A controllable timer seam: capture the scheduled callback so a test can fire
// the toast auto-dismiss synchronously, no fake clocks.
let pendingTimer: (() => void) | null;
let cancelledTimer: boolean;

beforeEach(() => {
  document.body.innerHTML = `<div id="root"></div>`;
  root = document.getElementById("root")!;
  feed = new FakeEventSource("/conversations/events");
  postAccept = vi.fn();
  postTurn = vi.fn();
  postNewConversation = vi.fn();
  pendingTimer = null;
  cancelledTimer = false;
  app = createChatApp({
    root,
    eventSource: () => feed,
    postAccept,
    postTurn,
    postNewConversation,
    seed: seed(),
    setTimer: (fn) => {
      pendingTimer = fn;
      return 1;
    },
    clearTimer: () => {
      cancelledTimer = true;
    },
  });
});

describe("transcript renders from conversation events", () => {
  it("shows the active conversation and its turns by role", () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "human_turn_appended", turn: { participant: { kind: "human", display_name: "you" }, body: "export to CSV?" } } });
    pushConv(feed, { sequence: 2, conversation_id: "conversation-1", payload: { type: "agent_reply_appended", reply: { message: "On-demand only?", proposals: [] } } });
    const turns = screen.getByTestId("turns");
    expect(within(turns).getByText("export to CSV?")).toBeTruthy();
    expect(within(turns).getByText("On-demand only?")).toBeTruthy();
    expect(within(turns).getByLabelText("you turn").getAttribute("data-role")).toBe("human");
  });

  it("an agent reply with a proposal renders an inert Accept button", () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "agent_reply_appended", reply: { message: "draft:", proposals: [{ id: "csv-export", kind: "issue", title: "Add CSV export", summary: "s", payload: { body: "do it" } }] } } });
    expect(screen.getByLabelText("proposal Add CSV export")).toBeTruthy();
    expect(screen.getByLabelText("accept Add CSV export")).toBeTruthy();
  });

  it("shows the typing indicator and clears it on reply", () => {
    app.dispatch({ t: "agent.typing", id: "conversation-1", on: true });
    expect(screen.getByTestId("typing")).toBeTruthy();
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "agent_reply_appended", reply: { message: "hi", proposals: [] } } });
    expect(screen.queryByTestId("typing")).toBeNull();
  });

  it("events for a non-active conversation don't change the visible transcript", () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-2", payload: { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "other thread" } } });
    expect(within(screen.getByTestId("turns")).queryByText("other thread")).toBeNull();
  });
});

describe("user interactions", () => {
  it("selecting a conversation in the rail switches the transcript", async () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-2", payload: { type: "human_turn_appended", turn: { participant: { kind: "human" }, body: "thread two" } } });
    await userEvent.click(screen.getByLabelText("conversation conversation-2"));
    expect(app.state.active).toBe("conversation-2");
    expect(within(screen.getByTestId("turns")).getByText("thread two")).toBeTruthy();
  });

  it("clicking Accept calls the injected POST seam (no direct mutation)", async () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "agent_reply_appended", reply: { message: "draft", proposals: [{ id: "csv-export", kind: "issue", title: "Add CSV export", payload: {} }] } } });
    await userEvent.click(screen.getByLabelText("accept Add CSV export"));
    expect(postAccept).toHaveBeenCalledWith("conversation-1", "csv-export");
    // the proposal becomes "accepted" only when the server's event arrives
    pushConv(feed, { sequence: 2, conversation_id: "conversation-1", payload: { type: "proposal_accepted", proposal_id: "csv-export", created: true, target: { kind: "issue", number: 142, title: "Add CSV export" } } });
    expect(screen.getByLabelText("proposal outcome")).toBeTruthy();
  });

  it("accepting an issue raises the cross-surface board toast", () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "proposal_accepted", proposal_id: "csv-export", created: true, target: { kind: "issue", number: 142, title: "Add CSV export" } } });
    const toast = screen.getByTestId("toast");
    expect(toast.textContent).toContain("#142");
    expect(toast.textContent).toContain("now in the pipeline");
    // the cross-surface affordance: a link back to the board
    const link = within(toast).getByText("view board →") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("./index.html");
  });

  it("the accepted proposal outcome links to the created issue", () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "agent_reply_appended", reply: { message: "draft", proposals: [{ id: "csv-export", kind: "issue", title: "Add CSV export", payload: {} }] } } });
    pushConv(feed, { sequence: 2, conversation_id: "conversation-1", payload: { type: "proposal_accepted", proposal_id: "csv-export", created: true, target: { kind: "issue", number: 142, url: "https://forge.test/issues/142", title: "Add CSV export" } } });
    const outcome = screen.getByLabelText("proposal outcome");
    expect(outcome.textContent).toContain("created");
    const link = within(outcome).getByText("issue #142") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("https://forge.test/issues/142");
  });

  it("the toast auto-dismisses when the injected timer fires", () => {
    pushConv(feed, { sequence: 1, conversation_id: "conversation-1", payload: { type: "proposal_accepted", proposal_id: "csv-export", created: true, target: { kind: "issue", number: 142, title: "Add CSV export" } } });
    expect(screen.getByTestId("toast")).toBeTruthy();
    expect(pendingTimer).not.toBeNull(); // a dismiss was scheduled
    pendingTimer!(); // fire the timer the controller scheduled
    expect(screen.queryByTestId("toast")).toBeNull();
  });
});

describe("composer", () => {
  it("typing and pressing Enter sends: optimistic human turn + postTurn, input cleared", async () => {
    const input = screen.getByLabelText("message") as HTMLTextAreaElement;
    await userEvent.type(input, "export to CSV?");
    await userEvent.keyboard("{Enter}");
    // the human turn shows immediately (optimistic echo)
    expect(within(screen.getByTestId("turns")).getByText("export to CSV?")).toBeTruthy();
    // the POST seam was called for the active conversation
    expect(postTurn).toHaveBeenCalledWith("conversation-1", "export to CSV?");
    // input cleared; the agent is now shown "typing"
    expect((screen.getByLabelText("message") as HTMLTextAreaElement).value).toBe("");
    expect(screen.getByTestId("typing")).toBeTruthy();
  });

  it("Shift+Enter inserts a newline and does NOT send", async () => {
    const input = screen.getByLabelText("message") as HTMLTextAreaElement;
    await userEvent.type(input, "line one");
    await userEvent.keyboard("{Shift>}{Enter}{/Shift}");
    expect(postTurn).not.toHaveBeenCalled();
    expect((screen.getByLabelText("message") as HTMLTextAreaElement).value).toContain("\n");
  });

  it("clicking Send also sends the typed turn", async () => {
    await userEvent.type(screen.getByLabelText("message"), "via the button");
    await userEvent.click(screen.getByLabelText("send"));
    expect(postTurn).toHaveBeenCalledWith("conversation-1", "via the button");
    expect(within(screen.getByTestId("turns")).getByText("via the button")).toBeTruthy();
  });

  it("an empty/whitespace composer does not send", async () => {
    await userEvent.type(screen.getByLabelText("message"), "   ");
    await userEvent.keyboard("{Enter}");
    expect(postTurn).not.toHaveBeenCalled();
  });
});

describe("new conversation", () => {
  it("clicking + new conversation calls the injected seam", async () => {
    await userEvent.click(screen.getByLabelText("new conversation"));
    expect(postNewConversation).toHaveBeenCalledTimes(1);
  });
});

describe("feed contract (Layer 3) end-to-end through the DOM", () => {
  it("replays the real /events fixture into a fully-rendered transcript", () => {
    for (const event of eventsFixture.events as ConversationEvent[]) pushConv(feed, event);
    const turns = screen.getByTestId("turns");
    expect(within(turns).getByText("I want users to export reports as CSV.")).toBeTruthy();
    expect(within(turns).getByText("Good scope. On-demand only for v1?")).toBeTruthy();
    expect(screen.getByLabelText("proposal outcome")).toBeTruthy(); // accepted at seq 4
    expect(screen.getByTestId("toast").textContent).toContain("#142");
  });
});
