// CONTROLLER for the chat page. Same shape as the board's app.ts: the
// per-conversation SSE source is INJECTED (Appendix B.3, seam 1), and the
// reducer/view are imported modules. Sending a turn / accepting a proposal /
// starting a new conversation are POSTs in prod; the resulting
// ConversationEvents arrive back on the feed.

import { applyChat, emptyChat, type ChatState, type ChatEvent, type ConversationEvent } from "./chat-model.js";
import { renderChat } from "./chat-view.js";
import type { EventSourceLike } from "./app.js";

export interface ChatDeps {
  root: HTMLElement;
  eventSource: (url: string) => EventSourceLike;
  // POST seams — injected so tests can stub them; defaults are inert.
  postTurn?: (id: string, body: string) => void;
  postAccept?: (id: string, proposalId: string) => void;
  postNewConversation?: () => void;
  seed?: ChatState; // optional initial state (e.g. from GET /conversations)
  // Toast auto-dismiss is a controller side effect; the timer seam is injected
  // so tests run it synchronously (or not at all). Defaults to window timers.
  toastTimeoutMs?: number;
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
}

export interface ChatApp {
  state: ChatState;
  dispatch: (ev: ChatEvent) => void;
  stop: () => void;
}

export function createChatApp(deps: ChatDeps): ChatApp {
  const app: ChatApp = { state: deps.seed ?? emptyChat(), dispatch, stop };

  const toastMs = deps.toastTimeoutMs ?? 6000;
  const setTimer = deps.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearTimer = deps.clearTimer ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
  let toastTimer: unknown = null;

  function dispatch(ev: ChatEvent): void {
    app.state = applyChat(app.state, ev);
    renderChat(deps.root, app.state);
    // schedule the cross-surface toast's auto-dismiss whenever one appears.
    if (app.state.toast && toastTimer == null) {
      toastTimer = setTimer(() => {
        toastTimer = null;
        dispatch({ t: "toast.clear" });
      }, toastMs);
    } else if (!app.state.toast && toastTimer != null) {
      clearTimer(toastTimer);
      toastTimer = null;
    }
  }

  const es = deps.eventSource("/conversations/events");
  es.onmessage = (m) => dispatch({ t: "conv.event", event: JSON.parse(m.data) as ConversationEvent });

  // sending a turn: optimistic local echo, then POST. The agent's "typing"
  // indicator runs until the real agent_reply_appended event clears it.
  function send(): void {
    const input = deps.root.querySelector<HTMLTextAreaElement>("[data-input]");
    const id = app.state.active;
    const body = input?.value.trim() ?? "";
    if (!input || !id || !body) return;
    input.value = "";
    dispatch({ t: "human.optimistic", id, body });
    dispatch({ t: "agent.typing", id, on: true });
    deps.postTurn?.(id, body);
  }

  deps.root.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;

    const sel = target.closest("[data-select]");
    if (sel) return dispatch({ t: "select", id: (sel as HTMLElement).dataset.select! });

    const acc = target.closest("[data-accept]");
    if (acc && app.state.active) return deps.postAccept?.(app.state.active, (acc as HTMLElement).dataset.accept!);

    if (target.closest("[data-send]")) return send();
    if (target.closest("[data-new]")) return deps.postNewConversation?.();
  });

  // Enter sends; Shift+Enter inserts a newline (composer affordance).
  deps.root.addEventListener("keydown", (e) => {
    const ke = e as KeyboardEvent;
    if (!(ke.target as HTMLElement).closest("[data-input]")) return;
    if (ke.key === "Enter" && !ke.shiftKey) {
      ke.preventDefault();
      send();
    }
  });

  function stop() {
    if (toastTimer != null) clearTimer(toastTimer);
    es.close();
  }

  renderChat(deps.root, app.state);
  return app;
}
