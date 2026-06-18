// Browser entry point for the chat page. esbuild bundles this to
// ui/dist/chat.js (see esbuild.config.mjs); chat.html loads it as a module.
//
// Like board-main.ts, it only selects the real seams — the native EventSource
// and the POST seams — and hands them to createChatApp. The reducer/view and
// all the testable controller wiring live in chat-app.ts, which tests drive
// with the FakeEventSource and stubbed POSTs.

import { createChatApp } from "./chat-app.js";
import { nativeEventSource } from "./native-event-source.js";

const root = document.getElementById("root");
if (!root) throw new Error("temper-web chat: missing #root element");

createChatApp({
  root,
  eventSource: nativeEventSource,
  // PR E wires these to the conversation proxy's real routes; the shapes match
  // POST /conversations/{id}/turns and .../proposals/{id}/accept.
  postTurn: (id, body) => {
    void fetch(`/conversations/${encodeURIComponent(id)}/turns`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ body }),
    });
  },
  postAccept: (id, proposalId) => {
    void fetch(
      `/conversations/${encodeURIComponent(id)}/proposals/${encodeURIComponent(proposalId)}/accept`,
      { method: "POST" },
    );
  },
  // PR E wires this to POST /conversations (open a new conversation). Profile
  // selection belongs to PR E's real route — this composition-root stub stays
  // provider-neutral and never names a concrete interaction profile; the
  // created conversation arrives via conversation_opened on the feed and the
  // feed updates the rail.
  postNewConversation: () => {
    void fetch(`/conversations`, { method: "POST" });
  },
});
