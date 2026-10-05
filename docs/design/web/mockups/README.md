# Temper web mockups

Open [chats.html](chats.html) for the chat entry screen, or [index.html](index.html) for a conversation. The other views are [inbox.html](inbox.html), [board.html](board.html), [task.html](task.html), and [changes.html](changes.html). Each file opens directly in a browser; no build step is needed.

These are browser-only mockups with sample data. You can start a chat, send words, accept or reject the shared proposal, answer an inbox choice, release a held task, filter lists, set a goal, and steer T21. Requests show a brief pending state, then update local sample state. No request is sent to temper or the forge. Use the **More** button to reset the demo.

For state to carry reliably between pages, serve this directory locally and open `http://localhost:8765/chats.html`:

```sh
python3 -m http.server 8765 --directory docs/design/web/mockups
```

The HTML files also open directly for visual review. Several sample tasks and chats have no dedicated mockup page.

The layout follows `../ux/`: the inbox is home, a chat is a task with its conversation first, the same pending proposal appears in the chat and inbox, goals are ordered on a board, and the forge contributes a changes view. The warm paper palette, narrow reading column, quiet sidebar, and rounded composer take their visual direction from modern chat interfaces.

The only external asset is the optional Google Fonts stylesheet. System fonts are used if it cannot load.
