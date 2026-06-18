// CONTROLLER for the chat page. Same shape as the board's app.ts: the
// per-conversation SSE source is INJECTED (Appendix B.3, seam 1), and the
// reducer/view are imported modules. Sending a turn / accepting a proposal are
// POSTs in prod; the resulting ConversationEvents arrive back on the feed.

import { applyChat, emptyChat, type ChatState, type ChatEvent, type ConversationEvent } from "./chat-model.js";
import { renderChat } from "./chat-view.js";
import type { EventSourceLike } from "./app.js";

export interface ChatDeps {
  root: HTMLElement;
  eventSource: (url: string) => EventSourceLike;
  // POST seams — injected so tests can stub them; defaults are inert.
  postTurn?: (id: string, body: string) => void;
  postAccept?: (id: string, proposalId: string) => void;
  seed?: ChatState; // optional initial state (e.g. from GET /conversations)
}

export interface ChatApp {
  state: ChatState;
  dispatch: (ev: ChatEvent) => void;
  stop: () => void;
}

export function createChatApp(deps: ChatDeps): ChatApp {
  const app: ChatApp = { state: deps.seed ?? emptyChat(), dispatch, stop };

  function dispatch(ev: ChatEvent): void {
    app.state = applyChat(app.state, ev);
    renderChat(deps.root, app.state);
  }

  const es = deps.eventSource("/conversations/events");
  es.onmessage = (m) => dispatch({ t: "conv.event", event: JSON.parse(m.data) as ConversationEvent });

  deps.root.addEventListener("click", (e) => {
    const sel = (e.target as HTMLElement).closest("[data-select]");
    if (sel) return dispatch({ t: "select", id: (sel as HTMLElement).dataset.select! });

    const acc = (e.target as HTMLElement).closest("[data-accept]");
    if (acc && app.state.active) return deps.postAccept?.(app.state.active, (acc as HTMLElement).dataset.accept!);
  });

  function stop() {
    es.close();
  }

  renderChat(deps.root, app.state);
  return app;
}
