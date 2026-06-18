// CONTROLLER — wires FEED + user events -> apply -> render. The EventSource is
// INJECTED (Appendix B.3, seam 1) so tests pass a fake and prod passes the real
// one. The app never calls `new EventSource` itself.

import { apply, initialState, type State, type Event } from "./model.js";
import { render } from "./view.js";

// The structural contract a feed source must satisfy — the browser's native
// EventSource matches it, and so does the test fake (src `FakeEventSource`).
export interface EventSourceLike {
  onmessage: ((ev: { data: string }) => void) | null;
  onerror: ((ev?: unknown) => void) | null;
  close(): void;
}

export interface AppDeps {
  root: HTMLElement;
  // factory, not an instance — lets the app (re)connect and lets tests inject
  eventSource: (url: string) => EventSourceLike;
  now: () => number;
}

export interface App {
  state: State;
  dispatch: (ev: Event) => void;
  stop: () => void;
}

export function createApp(deps: AppDeps): App {
  const app: App = {
    state: initialState(deps.now()),
    dispatch,
    stop,
  };

  function dispatch(ev: Event): void {
    app.state = apply(app.state, ev);
    render(deps.root, app.state);
  }

  // FEED: server -> dispatch. Same path the fake uses in tests.
  const es = deps.eventSource("/events");
  es.onmessage = (m) => dispatch(JSON.parse(m.data) as Event);
  es.onerror = () => dispatch({ t: "pipe", pipe: "dead" });

  // user events -> dispatch (delegated; never touch state/DOM directly).
  // Order matters: a close affordance (the ✕ button or the scrim) wins over the
  // card-open path, so a click inside the drawer's chrome doesn't re-open.
  deps.root.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    if (target.closest("[data-close]") || target.closest('[data-region="scrim"]')) {
      dispatch({ t: "close" });
      return;
    }
    const el = target.closest("[data-card]");
    if (el) dispatch({ t: "open", id: (el as HTMLElement).dataset.card! });
  });
  deps.root.ownerDocument.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Escape") dispatch({ t: "close" });
  });

  // theme toggle — a DOM-only side effect (NOT model state, UX §4: "a token
  // swap only — no view code changes"). The button lives in the static shell
  // (index.html header), outside #root, so wire it off the document.
  const themeBtn = deps.root.ownerDocument.getElementById("theme");
  themeBtn?.addEventListener("click", () => {
    const r = deps.root.ownerDocument.documentElement;
    r.dataset.theme = r.dataset.theme === "light" ? "dark" : "light";
  });

  function stop() {
    es.close();
  }

  render(deps.root, app.state);
  return app;
}
