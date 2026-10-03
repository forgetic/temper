# The forge's protocol layer

Provisional, 2026-10-03. How the engine reaches the forge: Forgejo's API
over HTTP and JSON, the webhooks Forgejo sends back, the engine's own
blocks inside comments and wiki pages, and the fake forge's protocol layer
that serves the same API in the worlds. The overview is protocol.md
(sections 3 and 7); the vocabulary carried is the forge child domain's
(`temper_engine_domain_forge::api`, engine-domain.md, sections 12 and 13);
the machines are skein's (http.md, json.md, tls.md). GitHub comes later,
and section 11 says where it fits. What the domains and skein owe this
layer is in section 12, what was checked of Forgejo and what is still to
check in section 13, and what is open in section 14.

## 1. In one page

- **Translation only.** Each call of the forge child domain (an `Op`) is
  answered by exactly one `Answer` or `Error`, made of one or a few HTTP
  exchanges on one connection. The protocol layer chooses the requests;
  the domain decides what to ask, when, and whether to ask again.
- **No retries, and every request counted.** A call goes out once. Its
  answer says how many requests it cost, so the domain's budget charges
  what the forge actually served, the requests that filled a cache
  included.
- **Paged, filtered, and decoded as it streams.** Every listing names its
  page and its size, at the cap the forge reports at startup, and every
  periodic listing is filtered by a time or a state and a label. Answers
  are decoded from JSON tokens as they arrive: what the domain does not
  carry (an item's body in a listing, a whole thread's text) is skipped
  as it passes, never held.
- **The engine's own blocks, versioned.** Keys, records, outcomes posted
  and notes' pages are written into comments, bodies and wiki pages in
  HTML comments, each with a version, a digest, and a fixed shape, so a
  person who edits one is told from one who did not.
- **Webhooks are hints, verified and answered first.** The body's
  HMAC-SHA256 is checked against the repository's secret as it streams;
  the delivery is answered before any hint goes up; each hint names the
  repository, the item, the commit or the branch, and who caused it, so
  the domain drops the echoes of its own writes.
- **The fake forge speaks the same API,** from the server's side, with
  Forgejo's quirks where they are known (its default order, its page
  cap, its unpaged threads), and counts the requests it serves, so the
  engine's worlds can hold the forge's load to a ceiling.
- **Facts are checked against a real Forgejo** before the client relies
  on them, by a conformance run against a throwaway instance, outside the
  suites; what it records becomes the machines' transcripts.

## 2. In the engine

```
domain (forge child)          Op, Answer, Error, Hint
  ▲  the engine's root domain: Request::Forge, Event::Answered, Event::Hint
temper-engine-protocol        calls, connections, caches, blocks, webhooks
  ▲  documents
temper-forge-forgejo          Forgejo's requests and answers, both sides; webhook payloads
  ▲  tokens
skein-json
  ▲  body stream
skein-http (client; server for webhooks)
  ▲  plaintext
skein-tls (client; server where the engine terminates TLS for webhooks)
  ▲  ciphertext
skein-io socket
```

- **`temper-forge-forgejo`** depends on skein-lib and skein-json. It holds
  Forgejo's documents and nothing of temper's domains: each request's
  method, path, query and JSON body as a sized encoder; each answer's
  decoder, a small state machine over JSON tokens; the webhook payloads
  and their signature check. Both sides live there, the client's for the
  engine and the server's for the fake forge (protocol.md, section 3), so
  the two are also fed transcripts of the real Forgejo (section 10).
- **The engine's protocol crate** holds the forge's connections, the
  calls in flight, the caches of section 3.5, the blocks of section 4, the
  webhook server, and the translation between `Request::Forge` and
  `Event::Answered` and the documents.
- **What crosses to the root domain** is the engine boundary's: a
  `Request::Forge { call, repository, op, payload }`, ended by exactly one
  `Event::Answered { call, result, decoded }`; and `Event::Hint`, which no
  request causes (engine-domain.md, section 13). The payload the parent
  names (a record, an outcome posted, a note's page) is encoded into the
  body as the call goes out; what of the engine's own the answer carries
  is decoded into `decoded` (section 4).

## 3. Forgejo's API as temper uses it

### 3.1 Conventions

- **Base and authentication.** Every request goes to the deployment's
  forge under `/api/v1`, with `Authorization: token <t>`: the engine's
  forge token, which it holds from startup, never logged.
  `User-Agent` names temper and its version.
- **JSON in, JSON out.** Bodies are written with skein-json's sized writer
  (`Content-Type: application/json`), and answers decoded from its
  tokens. Strings are UTF-8; text the domain carries is bytes.
- **Names in paths** (an owner, a repository, a branch, a wiki page, a
  user's login) are percent-encoded as a path segment; a branch with a
  slash is section 13's to check.
- **Times.** Forgejo writes RFC 3339 times with an offset; they are read
  as the forge's clock, in nanoseconds since the epoch, at its resolution
  of a second. `since` is written in RFC 3339, in UTC. The time an answer
  was made is its `Date` header, which a listing's answer carries as
  `now` (api.rs).
- **Commits.** A commit is named by 32 bytes in the domain (a SHA-1 id
  zero-padded, worker-domain.md, section 8). A repository's object format
  (`object_format_name`, read at startup) says whether its ids are 40 or
  64 hex digits; anything else is a malformed answer.
- **Paging is always explicit.** Every listing sends `page` and `limit`.
  The size is the smaller of the domain's `page` limit and the forge's
  `max_response_items`, read from `/settings/api` at startup (50 on the
  deployment's Forgejo, which also pages 30 by default when no limit is
  given). A listing without both is never sent: Forgejo answers with
  everything where it ignores the size. An answer has more after it when
  it came full.
- **Users by id.** The domain names people by their forge user id; the
  API names them by login in paths and filters. The protocol layer keeps
  a table of the two (section 3.5).

### 3.2 Each operation

What each `Op` sends, in order; R is the repository's `{owner}/{repo}`.
Calls of more than one request make them in turn on one connection, and
end on the first failure.

| `Op` | Requests | Answer from |
|---|---|---|
| `Items { state, kind, label, author, since, page }` | `GET R/issues?state=open\|closed\|all&type=issues\|pulls&labels=<name>&created_by=<login>&since=<time>&sort=leastupdate&page=<n>&limit=<size>`, each filter only when given | the rows, `Date` as `now` |
| `Item { number, after, since }` | `GET R/issues/<n>`; `GET R/issues/<n>/comments?since=<time>` (unpaged on Forgejo) | the item; the comments with ids above `after`, oldest first, the first `page` of them |
| `Comment { number, id }` | `GET R/issues/comments/<id>` | the comment, `Missing` if its `issue_url` is not item `number` |
| `Pull { number }` | `GET R/pulls/<n>`; `GET R/commits/<head>/status?page=1&limit=1` | the pull request; CI from the combined state |
| `PullFor { head, base }` | `GET R/pulls/<base>/<head>`; then as `Pull` | which pull request it names is section 13's to check |
| `Reviews { number, page }` | `GET R/pulls/<n>/reviews?page=<n>&limit=<size>` | submitted reviews, oldest first |
| `Statuses { commit, page }` | `GET R/commits/<sha>/status?page=<n>&limit=<size>` | the combined state, and the latest status of each context |
| `Remarks { number, review, page }` | `GET R/pulls/<n>/reviews/<id>/comments` (unpaged on Forgejo) | the `page`th slice, cut as it streams |
| `Permission { user }` | `GET R/collaborators/<login>/permission` | `none`, `read`, `write`, `admin` or `owner` (as `Admin`) |
| `Branch { branch }` | `GET R/branches/<branch>` | `commit.id` |
| `Pages { after }` | `GET R/wiki/pages?page=<n>&limit=<size>` | names and each page's last commit; the cursor (section 12) |
| `Page { name }` | `GET R/wiki/page/<name>` | `content_base64`, decoded; the revision from `last_commit.sha` |
| `CreateIssue { key, title, body, labels }` | `POST R/issues` `{title, body, labels: [ids]}` | `number` |
| `Post { number, key, person, body }` | `POST R/issues/<n>/comments` `{body}` | `id`; the revision of the body it returns |
| `EditComment { number, id, body }` | `PATCH R/issues/comments/<id>` `{body}` | the revision of the body it returns, or of the body sent if none |
| `AddLabels { number, labels }` | `POST R/issues/<n>/labels` `{labels: [names]}` | done |
| `RemoveLabels { number, labels }` | `DELETE R/issues/<n>/labels/<name>`, one per label; `Missing` on one it does not carry counts as done | done |
| `OpenPull { title, body, head, base }` | `POST R/pulls` `{head, base, title, body}` | `number` |
| `Merge { number, head }` | `POST R/pulls/<n>/merge` `{Do: <style>, head_commit_id: <head>}`; `GET R/pulls/<n>` | `merge_commit_sha` (the merge answers with no body) |
| `Review { number, key, verdict, body }` | `POST R/pulls/<n>/reviews` `{event, body}` | `id` |
| `SetReviewers { number, reviewers }` | `GET R/pulls/<n>`; `POST R/pulls/<n>/requested_reviewers` for those missing; `DELETE` for those extra | done |
| `SetDependencies { number, dependencies }` | `GET R/issues/<n>/dependencies?page&limit`, every page; `POST` for those missing; `DELETE` for those extra, each `{owner, repo, index}` | done |
| `Close { number }`, `Reopen { number }` | `PATCH R/issues/<n>` `{state: closed\|open}` | done |
| `DeleteBranch { branch }` | `DELETE R/branches/<branch>` | done |
| `PutPage { name, content, nonce }` | `PATCH R/wiki/page/<name>` `{title, content_base64, message}`; on `Missing`, `POST R/wiki/new` | the revision from `last_commit.sha` |
| `DeletePage { name }` | `DELETE R/wiki/page/<name>` | done |

Notes on the table:

- **The keep-up listing** (`state` and `label` absent) is the only
  listing with `state=all`, and it always carries `since`: its rows are
  what changed since the last pass, so its cost follows the rate of
  change (engine-domain.md, section 12). Listings by label (cold start,
  room after a refusal) and the slow pass (open items, one page per
  `slow`) carry a state and a label or a pace of their own.
- **`sort=leastupdate`** is Forgejo's least-recently-updated order. Its
  default is `latest`, newest *created* first, and a GitHub-style
  `sort=updated&direction=asc` is not in its vocabulary: a client that
  sends the wrong words gets the wrong order with no error. The fake forge
  answers the same way (section 9), and section 13 checks the real one.
- **An item's comments are not paged by Forgejo.** The call reads them
  since a forge time the domain gives (the item's updated time when it
  last read them, or the creation time of comment `after` when it pages
  through a thread), keeps the first `page` with ids above `after`, and
  skips the rest as they stream, bodies unread. A read with no time, the
  first of an item it admits, streams the whole thread once. Section 14
  weighs the timeline's paged listing for long threads.
- **Issues and pull requests share one numbering,** so an `Item` read of
  a pull request is its conversation, and `Close`, `Reopen` and labels
  are the issue endpoints for both.
- **Labels.** Forgejo takes label names when adding and removing, but
  only ids when creating an issue, so `CreateIssue` maps names to ids from
  the label cache (section 3.5).
- **Reviews** are mapped by their state: `APPROVED` to `Approve`,
  `REQUEST_CHANGES` to `RequestChanges`, `COMMENT` to `Comment`; pending
  reviews and review requests are not reviews, and are skipped. A
  dismissed review is a question of section 14.
- **CI.** The combined state's `failure` and `error` are `Failed`,
  `pending` is `Pending`, `success` and `warning` are `Passed`, and a
  commit with no status is `None`. Forgejo's Actions report each job as
  a status on the commit it ran on (to be checked, section 13), so CI is
  read from statuses alone, never from a history of runs.
- **Merges.** The merge style (`Do`) is the repository's, from the
  deployment's configuration: squash by default, as the fake forge
  merges. `head_commit_id` makes the merge conditional on the head the
  domain names, which Forgejo refuses when it moved (`Stale`).
- **Several requests, one call.** `Pull`, `Merge`, `SetReviewers`,
  `SetDependencies`, `PutPage` and `PullFor` make more than one; a cache
  miss adds one (section 3.5). The domain asks for what it needs, and the
  answer's cost tells its budget what that took (section 12).

### 3.3 Decoding answers

- **Streams, not documents.** Each answer's decoder is a state machine
  over skein-json's tokens with a bounded stack, pulled as the body
  arrives (json.md). It keeps only the fields its answer needs, as
  plain values; every other field, nested object and array is skipped as
  it passes.
- **Text is cut, skipped, or captured at its head:**
  - a title, a body, a comment's text, a status's description and URL
    are cut to the domain's `title_bytes` and `body_bytes`, the rest of
    the string skipped (api.rs: the protocol layer cuts what an answer
    brings to the limits);
  - **in a listing,** an item's title and body are not carried at all:
    only the head of its body is captured, at most `marker_bytes`, to
    find a creation's key (section 4); the listing's summaries carry the
    key and empty text (section 12);
  - every string is read as UTF-8 by skein-json; a body that is not
    valid UTF-8 is a malformed answer.
- **Revisions.** A comment's revision is a 64-bit digest of its whole
  body as the forge returns it, computed as the string streams past,
  before any cut. So it changes whenever the body does, at any resolution
  of the forge's clock, including within a second; and the answer to a
  post or an edit names the revision of what the forge stored, not of
  what was sent. A wiki page's revision is the first eight bytes of its
  last commit's id.
- **Markers** at the head of a body, a comment or a review are decoded
  into `Mark` and `key` (section 4); a record's parent part, an outcome
  posted and a note's page into `Decoded`.
- **What may follow** is told by the page coming full. For a call made of
  several requests, `more` is the last request's.
- **A malformed answer** (a document that does not decode, a field
  missing, a number out of range, a commit id of the wrong length) fails
  the call as `Timeout`: the request reached the forge, so what it asked
  may have been done; and the connection is closed, since what followed
  on it can no longer be trusted.

### 3.4 Failures

A call's `Error` is the first failure of its requests. "Not sent" means
the call failed before any byte of its first request was written.

| What happened | `Error` |
|---|---|
| no connection, a refused connect, a TLS handshake that failed, before anything was sent | `Unavailable` |
| `429`, with `Retry-After` (seconds or a date) | `RateLimited { after }`: the wait, from the answer's `Date` |
| anything after a byte was sent: a reset, the end of the stream, the call's deadline, `5xx`, a malformed answer | `Timeout` |
| `401`, `403` | `Forbidden` |
| `404` | `Missing` |
| `413` (Forgejo's quota) | `Full` |
| `422` on a write whose title or body is empty | `Empty` |
| `409` on `OpenPull` (a pull request for the head and base is open) | `Exists` |
| `422` on `OpenPull` with no changes between head and base | `NothingToMerge` |
| `405` on `Merge` for a pull request merged or closed | `Closed` |
| `409` on `Merge` for `head_commit_id` that is not the head | `Stale` |
| `405` on `Merge` for a conflict | `Conflict` |
| `405` on `Merge`, `403` on `DeleteBranch`, for a branch's protection | `Protected` |
| `4xx` on `SetDependencies` for a cycle | `Circular` |
| `423` (the repository is archived) | `Forbidden` |

- **Unavailable is provable only before the first byte.** A connection
  kept alive that the forge closed just as a request went out may have
  been read: it is a `Timeout`. To make that rare, an idle connection is
  closed before the forge's keep-alive timeout would close it.
- **One status, several meanings.** Where Forgejo answers two failures
  with one status (`405` on a merge), the decoder reads its `message`
  into the failure; the messages are pinned by transcripts of the real
  Forgejo (section 13), and one the decoder does not know is a `Timeout`
  on a write, `Forbidden` on a read: never a guess that something was
  not done.
- **The deadline** of a call is the forge child domain's `lifetime`, the
  longest a call may still take effect (its `Limits`), which the engine's
  configuration gives both layers; past it the call is a `Timeout` and its
  connection is aborted.

### 3.5 What the protocol layer keeps

Mechanism only, bounded, and rebuilt by asking again:

- **Repositories:** for each of the deployment's, its `{owner}/{repo}`,
  its object format, whether its wiki is on, and its merge style; read
  with `GET R` at startup, one request each.
- **Labels:** each repository's label names and ids, read with
  `GET R/labels?page&limit` at startup, and again when a `CreateIssue`
  names one it does not know; a label still unknown then fails the call
  as `Missing`.
- **Users:** a table of logins by user id, filled from every user object
  an answer carries (the engine's own from `GET /user` at startup); an id
  the table does not hold is looked up with
  `GET /users/search?uid=<id>&limit=1`. When the table is full, the entry
  used longest ago goes.
- **The forge's page cap,** from `/settings/api` at startup.

Startup costs 2 + 2R requests or so, before the first call is answered;
calls that come before then wait, which costs nothing, since a new domain
makes no call until `lifetime` after its first moment (limits.rs).

## 4. The engine's blocks

What the engine writes inside what it creates. Each piece is an HTML
comment, which Forgejo's rendering hides; each carries the version of its
shape; and a decoder reads every version an engine has written (protocol.md,
section 3).

- **A creation's key,** at the head of an issue's body, a comment or a
  review: `<!-- temper:key 1 <key in hex>[ for <user id>] -->` and a line
  break. `for` names the person a message from the web was written for
  (api.rs, `Mark::Key`).
- **A record,** a comment the engine owns:

  ```
  <!-- temper:record 1 <comment> <pull_comment> <reviews> <head|-> <ci> <nonce> -->
  temper keeps its record of this item here: please leave this comment as it is.
  <!-- temper:block 1
  <base64 of the parent's record and its digest, in lines of 76>
  -->
  ```

  The head is the forge child domain's part: the inbox position (comment
  ids, the reviews' count, the head in hex or `-`, CI as 0 to 3) and the
  nonce of the write that made it. The block is the parent's
  `Record`, encoded in temper's sized binary form (the channel's
  primitives, channel.md) with a version of its own, followed by the first
  16 bytes of its SHA-256.
- **An outcome posted:** a key's marker, the line `temper outcome`, and a
  block holding the parent's `Posted`.
- **A note's page:** `<!-- temper:nonce <nonce> -->` first when the engine
  wrote it; the description on the next line; then
  `<!-- temper:note 1 <base64 of who wrote it and what it refers to, and its digest> -->`;
  then the body, as a person may read and correct it
  (temper-engine-domain-notes, boundary.rs).
- **Told apart from a person's edit.** A head that does not parse makes
  the comment's mark `Mangled`; a block whose digest or version does not
  hold leaves `decoded` without the parent's part, which the parent reads
  as a record or page mangled. Bodies are cut to `body_bytes`, so a block
  longer than that is mangled too: the parent keeps what it writes within
  the limit.
- **The digest is SHA-256,** from RustCrypto's `sha2`, already in step code
  for webhooks (protocol.md, section 7). It catches accidental edits;
  whose comment it is, and so whether to believe it, is the domain's
  check of the author.
- **The codecs the worlds use today** (`tests/engine/domain/src/codec.rs`,
  `tests/engine/forge/src/translate.rs`) become these, with the versions
  added, base64 in place of hex, the block hidden, and a SHA-256 digest
  in place of a 64-bit one; the worlds then use the protocol crate's.

## 5. Webhooks

- **A server in the engine,** on skein's HTTP server, over TLS where the
  forge reaches the engine from another host. It takes `POST` on one
  path, one request at a time per connection, kept alive.
- **Verified as it streams.** Forgejo signs each delivery's body with the
  repository's secret, in `X-Forgejo-Signature` (HMAC-SHA256, in hex).
  The body is hashed as it arrives, and decoded at the same time into
  the few fields a hint needs; nothing of it is believed until the whole
  body is read and the signature holds. A delivery that fails is answered
  `403` and dropped. The secret is the repository's, which the engine
  holds from startup, compared in constant time.
- **Answered first.** Once the body is read and verified, the response
  (`204`) is queued, and only then does the hint go up. The engine never
  makes Forgejo wait on its work.
- **Which deliveries hint what**, by `X-Forgejo-Event`:

  | Event | `Hint` |
  |---|---|
  | `issues`, `issue_comment`, `issue_label`, `pull_request` and its kinds (`_label`, `_comment`, `_review_*`, `_sync`) | the repository, the item's number |
  | `push` | the repository, the branch, the commit it moved to |
  | `create`, `delete` of a branch | the repository, the branch |
  | `status`, an Actions run's end, where Forgejo sends them | the repository, the commit |
  | `wiki` | the repository (section 12) |
  | anything else, or a repository the deployment does not hold | nothing |

  Each hint also names the user who caused it (`sender.id`), so the
  domain drops its own writes' echoes (section 12).
- **Bounded.** Connections, the head's size and the body's are capped; a
  body past its cap is answered `413` and hints nothing, and polling
  finds what it told of. Push payloads list commits, which the decoder
  skips.
- **Set up by the deployment.** Each repository's webhook (its URL, its
  secret, its events) is configured on the forge by whoever deploys the
  engine; making them through the API is a question of section 14.

## 6. The load rules, and where each is kept

| Rule | Kept by |
|---|---|
| No listing without a filter on a periodic path; no history | the domain's one keep-up listing per repository, always `since`; open items only, one page per `slow`, for the slow pass (3.2) |
| Calls per pass do not grow with history, labels or items | one listing per pass, label listings only at cold start or after a refusal; checked by the cost checks (10) |
| Cache by change token | comments read `since` the last read, revisions by digest; labels, users, repositories cached (3.5); a pull request read on a backoff (engine-domain.md, 12) |
| One path to CI; never a closed pull request's | statuses of the head only (3.2); a closed item leaves the working set |
| API only; `page` and `limit` always; the forge's cap | 3.1 |
| Webhooks: hints, answered first, merged, targeted, echoes dropped | 5; the domain merges hints into one earlier pass, and drops its own echoes by `by` (12) |
| Closed items by deltas, not sweeps | the keep-up listing's `since`; closed items are never listed again |
| Backoff and jitter; honour the rate; no blind retries of writes | the domain's backoff and budget; `RateLimited { after }`; keyed creations found again after a `Timeout`; no retry here (3.4) |
| Slow cadences; webhooks for latency, polling for liveness | the domain's `poll`, `hinted` and `slow` |

Nothing the engine writes changes often: a record is written when a run's
answer moves the inbox, never as a heartbeat.

## 7. Entities, limits and the worst case

**Entities,** each in a slab of the engine's protocol layer:

- **a call,** bound to the domain's `call` token, holding its `Op`, the
  step it is at, what it has decoded so far, and its cost;
- **a forge connection,** bound to io's socket, holding its stack (TLS,
  HTTP client, JSON tokenizer, the answer's decoder) and its deadlines;
- **a webhook connection,** bound to an accepted socket, holding its stack,
  the HMAC state and the fields decoded;
- **the caches** of section 3.5.

**Limits** (the engine's protocol `Limits`, forge part):

- `connections`: the forge connections, equal to the forge child domain's
  `calls`, so a call never waits for one;
- `head_bytes`, `body_bytes_in`: the largest response head and body read
  (the body is streamed, so this caps time and bytes, not memory);
- `depth`: the JSON nesting followed (Forgejo's documents need about 8);
- `marker_bytes`: the head of a body captured for its markers;
- `users`, `labels` (per repository): the caches;
- `idle`: how long an idle connection is kept, below the forge's
  keep-alive timeout;
- the webhook server's `hooks` (connections), `hook_head_bytes`,
  `hook_body_bytes`, and its read deadline;
- what TLS, HTTP and JSON take as their own (tls.md, http.md, json.md).

**The worst case** is the sum of: each connection's stack and intake and
output caps; each call's answer as it is built, at most the domain's
`answer_bytes` (limits.rs); each webhook connection's stack; the caches.
None of it grows with the forge's history: a long thread or a large
payload costs time, not memory.

## 8. State machines

**A forge connection:**

| State | Holds | Demand | Deadline | On |
|---|---|---|---|---|
| Closed | nothing | none | none | a call: Connecting |
| Connecting | the call | none | connect | connected: Handshaking; failed: the call `Unavailable`, Closed |
| Handshaking | the call, TLS | TLS's | handshake | done: Sending; failed: `Unavailable`, Closed |
| Sending | the call, its request | room | the call's | sent: Reading; a fault: `Timeout`, Closed |
| Reading | the call, the decoder | the head, then the body as the decoder demands | the call's | answered: the next step's Sending, or Idle with the answer up; a fault, a malformed answer or the deadline: `Timeout`, Closed |
| Idle | nothing | none | `idle` | a call: Sending; the deadline or the forge closing: Closed |

Without TLS, Connecting goes to Sending. A connection the forge marked
`Connection: close` goes to Closed after its answer, not to Idle.

**A call:** Waiting for startup (bounded by the domain's `calls`), then
running its steps on its connection, then answered, once. Its result is
the first failure of its steps, or its answer, with its cost.

**A webhook connection:**

| State | Holds | Demand | Deadline | On |
|---|---|---|---|---|
| Head | the head so far | a scan for the head's end | the read deadline | a `POST` to the path, its length within the cap: Body; anything else: answered `4xx`, Closing |
| Body | HMAC state, fields decoded | the body, as the decoder demands | the read deadline | read and verified: Answering; signature wrong: `403`, Closing; past the cap: `413`, Closing |
| Answering | the hint | room | the read deadline | `204` queued: the hint up, then Head (kept alive) or Closing |
| Closing | nothing | none | io's close deadline | closed |

**Startup:** Loading (settings, the engine's user, repositories, labels),
then Ready. A load that fails is tried again after a backoff of the
protocol layer's own: these are its reads, not the domain's calls.

## 9. The fake forge's protocol layer

The fake forge becomes a service (testing.md, 4.2): its domain stays as
it is, and a protocol layer serves Forgejo's API over it, from the server
side of `temper-forge-forgejo`.

- **The routes of section 3.2,** and those of startup: `/settings/api`,
  `/user`, `/users/search`, `R`, `R/labels`. Each request is decoded,
  translated to the fake's `Read` or `Write` (as the engine's world's
  translation does today), and its answer encoded as Forgejo encodes it.
  Users have logins of the fake's choosing, and the user a token stands
  for is the caller.
- **Forgejo's quirks, kept:** the default order is newest created first;
  `sort=leastupdate` gives least recently updated first; a page is capped
  at the configured `max_response_items`; a size without a page is
  ignored where Forgejo ignores it; an issue's comments and a review's
  comments come unpaged; a merge answers with no body. A client that
  leans on Forgejo being better than it is fails in the fake first.
- **Faults move down** (testing.md, 4.2): a call refused for the rate is
  a `429` with `Retry-After`; one failed before it is made is a refused
  connect or a reset before the request is read; one made and then timed
  out is a `5xx` or a dropped connection after the request was read; a
  late answer is a slow one.
- **Webhooks are sent,** for the fake's `Request::Hook`, as Forgejo sends
  them: one `POST` per change, its body signed with the repository's
  secret, after the latency the fake draws, late, out of order or never;
  the fake's protocol layer is the client then, on a connection of its
  own.
- **Requests are counted,** by route, by user and in total, in the fake's
  tally, so a world can hold the engine's load to its ceilings (section
  10).
- **Git over HTTP** is the fake's other face (testing.md, 4.2); the worker's
  protocol layer meets it, not the engine's, and it is designed with the
  worker's git (protocol.md, section 8).

## 10. Testing

- **Machine worlds** for `temper-forge-forgejo`: each decoder against
  transcripts of the real Forgejo (each answer the client reads, each
  error the table of section 3.4 names, webhook deliveries of every kind
  section 5 hints), against answers generated from the seed, and against
  mutations of both; fuzzed. Server encoders meet the client decoders, and
  the client encoders meet the server decoders.
- **Transcripts are format-faithful, content-irrelevant.** Ids, times,
  counts and text vary; keys, nesting, types and status lines must match.
  Volatile fields are masked before comparing; tokens and secrets never
  reach a file.
- **Protocol worlds:** the engine's protocol layer and the fake forge's,
  joined by an in-memory stream cut at random, running the engine world's
  scenarios through bytes (testing.md, 2.2): what the engine's domain
  asked is what the fake's domain did, and every webhook the fake sent is
  a hint or a dropped echo.
- **The cost checks,** in the engine's worlds, the domain worlds counting
  `Request::Forge` and the protocol worlds counting HTTP requests:
  - **idle:** over a span with no change on the forge, the calls made are
    at most the keep-up listings, one per repository every `poll`, the
    held pull requests due on their backoff, and the slow pass's pages and
    probes every `slow`; the bound is computed from the limits;
  - **history-free:** the same scenario on a forge preloaded with ten
    times the closed items and comments makes the same calls while idle;
  - **busy:** listing pages per pass are at most the changed items over
    the page size plus one, and item reads at most the changed items held
    times one plus the pages of their new comments;
  - **honest costs:** in the protocol worlds, the requests the fake served
    equal the sum of the costs the answers reported.
- **The real Forgejo check,** outside the two suites: a throwaway
  Forgejo, the same major version as the deployment's, in a scratch
  directory with SQLite, a small `MAX_RESPONSE_ITEMS` so paging shows,
  and webhooks allowed to loopback (`[webhook] ALLOWED_HOST_LIST`); a
  script drives the facts of section 13 through the engine's protocol
  layer and plain requests, says which hold, and records what it sent and
  received as transcripts, masked. Actions need a runner; until one runs
  there, the Actions facts are read, read-only, on the deployment's
  Forgejo.

## 11. Room for GitHub, and for recording

**GitHub** comes later (protocol.md, section 7). Nothing here assumes
Forgejo above `temper-forge-forgejo` and the call plans of section 3.2:

- **A provider is a crate of documents and a set of call plans.** The
  forge child domain asks for an item, a page of what changed, CI on a
  commit; GitHub's plans answer from its REST or GraphQL API, a call
  made of whatever requests that takes, its cost reported as Forgejo's
  is.
- **Conditional requests,** which GitHub does not count against its rate
  when nothing changed, fit as validators the domain carries without
  reading: an answer brings an opaque validator, a later call names it,
  and the answer can be `Unchanged`. The domain grows that pair when
  GitHub is built; until then nothing names one.
- **Batched reads** (a pull request, its CI and its reviews in one
  GraphQL query) fit inside one call; several calls in one query would
  be the protocol layer gathering calls that wait, a mechanism it may
  add.
- **Checks** are read into the same `Ci` and `Status`es; GitHub's labels on
  pull requests and its `since` listings fit the same `Op`s.
- **The rate as it stands:** GitHub reports what is left of its window
  and when it resets on every answer; an answer carries it to the
  domain's budget (section 12).

**Recording real traffic,** later: each forge connection and webhook
connection can tee the bytes it sends and receives, at the io boundary,
to a recorder that masks credentials and volatile fields and writes
transcripts. The conformance check of section 10 is its first user.

## 12. What others owe

**The forge child domain** (`temper_engine_domain_forge`):

- `Op::Item` gains `since: Option<Time>`: the forge time from which its
  comments are enough (the item's updated time at its last read, or the
  creation time of comment `after`); `None` reads the thread whole.
  Forgejo does not page an item's comments.
- **Listings carry no text:** `Answer::Items`' summaries have empty titles
  and bodies, only their keys; an `Item` read carries them. The child
  domain reads neither today.
- `Op::Pages`' `after` and `Answer::Pages`' `next` are a cursor the
  protocol layer makes and the domain passes back unread (for Forgejo, a
  page number), not a page's name.
- **Costs:** `Event::Answered` (the root's and the child's) carries the
  requests the call made, and the budget charges them instead of one per
  call. Later, with GitHub: the rate as the forge reports it, and
  validators with `Unchanged`.
- **Hints** name who caused them (`by: Option<u64>`), so the domain drops
  its own echoes, and a wiki's change, so the notes hear of it
  (`Event::Refresh`, temper-engine-domain-notes).
- **The deadline:** the call's is the child domain's `lifetime`, which the
  configuration gives both layers; or `Request::Forge` carries it.

**The root domain** passes `cost` and the new hint fields through, and
`decoded` as today.

**skein-json** must let a decoder take a long string in pieces: keep its
first N bytes, digest all of it, and skip the rest, without holding it.
Bodies can be megabytes; json.md's strings are whole, under a maximum.

**skein-http** must let a server answer a request before the domain
above has heard of it, which webhooks need; and tell a client whether any
byte of a request was written before a fault (section 3.4).

## 13. Facts checked, and to check

Checked on 2026-10-03 against the deployment's Forgejo
(`15.0.0+gitea-1.22.0`), from its API description and its answers:

- `max_response_items` is 50 and `default_paging_num` 30
  (`/settings/api`).
- The issue listing takes `sort` among `relevance`, `latest`, `oldest`,
  `recentupdate`, `leastupdate`, `mostcomment`, `leastcomment`,
  `nearduedate`, `farduedate`, defaulting to `latest`; it has no
  `direction`. Its `labels` filter is documented as any of them; temper
  sends one at a time.
- An issue's comments take `since` and `before` (updated times) and no
  page; a review's comments take neither.
- The combined status of a ref is paged; a commit status is `pending`,
  `success`, `error`, `failure` or `warning`.
- Adding labels takes ids or names; creating an issue takes ids only.
- A merge takes `head_commit_id` and answers `200` with no body.
- `GET /users/search` takes a `uid`; a collaborator's permission is read by
  login.
- A repository says its `object_format_name` (`sha1` or `sha256`).
- The API is not open without a token: the public repositories answer
  `404` to an anonymous call, so the rest waits for the conformance check.

To check (section 10), each before the client relies on it:

- that `sort=leastupdate` is honoured, ties ordered by number, and `since`
  is inclusive at the second;
- what moves an item's updated time: comments posted, edited, deleted;
  labels; reviews; a push to a head; a status on it; a base moving;
  dependencies added and removed; closing and reopening;
- that comments' `since` is inclusive, on updated times;
- that Actions report each job as a status on its head, and whether
  Forgejo sends a webhook for a status or for an Actions run's end;
- which pull request `pulls/{base}/{head}` names when several did;
- reviews' order, how a dismissed review shows, and what `position` a
  review comment's line is;
- every status and message of section 3.4, the merge's above all;
- branch and wiki page names with a slash, in paths;
- the largest comment body the deployment's database keeps;
- the webhook headers and payloads of every event of section 5, and
  whether Forgejo delivers the engine's own changes;
- whether Forgejo limits the API's rate, and with which headers.

## 14. Open questions

- **Long threads, read cold.** A thread of C comments costs C/page reads
  of up to C rows each the first time an item is admitted. Forgejo's
  timeline is paged (`issues/{n}/timeline?since&page&limit`) but mixes
  every event with comments and pages by offset, which a deletion shifts.
  Measure before choosing.
- **Dismissed reviews:** whether one counts as no verdict, or as the
  comment it was.
- **Webhooks through the API:** whether the engine makes each repository's
  webhook (its URL, secret and events) at startup, or the deployment
  does; the first needs admin rights on every repository.
- **An expired or revoked forge token** reads as `Forbidden` on every
  call; whether the domain should tell it apart, so a person is told once
  rather than every item held being refused.
- **A wiki's change** as a hint: whether the notes need the page's name,
  which Forgejo's payload carries, or the repository is enough.
