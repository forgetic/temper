// The ~15-line fake that makes the whole UI testable with NO network and NO
// backend (Appendix B.2). It matches `EventSourceLike` and lets a test PUSH
// events through exactly the path the real SSE stream uses.

import type { EventSourceLike } from "../src/app.js";
import type { Event } from "../src/model.js";

export class FakeEventSource implements EventSourceLike {
  onmessage: ((ev: { data: string }) => void) | null = null;
  onerror: ((ev?: unknown) => void) | null = null;
  closed = false;
  readonly url: string;

  constructor(url: string) {
    this.url = url;
  }

  /** simulate the server pushing one SSE message */
  push(event: Event): void {
    this.onmessage?.({ data: JSON.stringify(event) });
  }

  /** simulate the connection dropping */
  fail(): void {
    this.onerror?.();
  }

  close(): void {
    this.closed = true;
  }
}
