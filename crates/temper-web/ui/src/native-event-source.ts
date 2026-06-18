// Adapts the browser's native EventSource to the structural EventSourceLike the
// board/chat apps consume. The two differ in handler parameter types (the native
// onmessage gets a full MessageEvent; the app only needs `{ data: string }`), so
// we wrap rather than cast. Both entry points (board-main, chat-main) share this.

import type { EventSourceLike } from "./app.js";

export function nativeEventSource(url: string): EventSourceLike {
  const es = new EventSource(url);
  const like: EventSourceLike = {
    onmessage: null,
    onerror: null,
    close: () => es.close(),
  };
  es.onmessage = (ev) => like.onmessage?.({ data: ev.data });
  es.onerror = (ev) => like.onerror?.(ev);
  return like;
}
