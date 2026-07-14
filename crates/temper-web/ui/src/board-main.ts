// Browser entry point for the board page. esbuild bundles this to
// ui/dist/board.js (see esbuild.config.mjs); index.html loads it as a module.
//
// It does the one thing a controller's entry should: pick the real seams (the
// native EventSource, the wall clock) and hand them to the injected-deps
// createApp from app.ts. Tests never import this file — they construct createApp
// with the FakeEventSource directly — so all the testable logic stays in app.ts.

import { createApp } from "./app.js";
import { nativeEventSource } from "./native-event-source.js";

const root = document.getElementById("root");
if (!root) throw new Error("temper-web board: missing #root element");

createApp({
  root,
  eventSource: nativeEventSource,
  now: () => Date.now(),
  fetchJson: async (url) => {
    const response = await fetch(url, { headers: { Accept: "application/json" } });
    if (!response.ok) throw new Error(`trace request failed: ${response.status}`);
    return response.json() as Promise<unknown>;
  },
});
