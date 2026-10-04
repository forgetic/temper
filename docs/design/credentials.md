# Credentials

Provisional, 2026-10-03. Where temper's secrets live, who uses each, and
how they reach the processes that need them: the LLM accounts the engine
refreshes and grants to runs, the forge credentials of the engine and of
workers' git, and the secrets that authenticate temper's own channels.
The overview is `protocol.md` (section 5); the channel that carries grants
is `channel.md`, and the requests that use them are `llm.md` and
`forge.md`. What is open is listed in section 11.

## 1. In one page

- **The engine holds every credential.** Workers keep only the secret that
  lets them dial the engine; agents keep nothing beyond the access tokens
  they are granted for their run.
- **LLM accounts are OAuth grants,** refreshed by the engine alone. A
  provider may rotate a refresh token each time it is used, so a second
  process refreshing the same account would spend it twice. A rotated
  token is made durable before the access token it bought is handed
  out.
- **Accounts are a child domain of the engine's.** It decides when to
  refresh, what a failed refresh or a spent account means, and which
  attempts must hear of a new token. The OAuth server is a peer like any
  other.
- **Grants are pushed.** An assignment carries its attempt's grants: one
  per LLM account its charter's endpoints use, and one for the git
  identity its repositories use. After each refresh, the engine pushes
  the new grant to every live attempt that uses the account; after every
  hello, it pushes the current grants of each attempt it keeps. A run that
  finds its token refused reports it, and the engine may refresh early.
- **Names through the domains, values beside them.** A domain names a
  credential by `(account, generation)` and never holds its value. Each
  protocol layer keeps the values it carries in a small table indexed by
  account, fills a value in as it encodes a message, and takes it out as
  it decodes one. No trace, snapshot or log of a domain can leak a token.
- **Times across hosts are durations.** A grant says how long its token is
  valid, and the receiver turns that into a deadline on its own clock.
- **Git gets its token per invocation,** in the environment of that one
  git process, never in a URL or a repository's configuration.

## 2. The credentials

| Credential | Kind | Held by | Used by | Changes |
|---|---|---|---|---|
| an LLM account | OAuth: refresh token, access token, expiry | the engine | agents, through grants | the engine refreshes it |
| a git identity | the forge's token for pushing and fetching | the engine | workers' git, through grants | an operator replaces it |
| the forge's API token | the forge's token | the engine | the engine's forge client | an operator replaces it |
| the webhook secret | a shared secret | the engine | checking webhooks' signatures | an operator replaces it |
| a worker's secret | a shared secret per worker | the engine and that worker | the worker's hello | an operator replaces it |

The git identity is apart from the forge's API token: workers need only
fetching and pushing, so a deployment can give them a token with less
power than the engine's.

## 3. Where they come from

Each process has its configuration and secrets in memory when it starts.
Where they come from is the shell's business, and not decided here. What
the domains need of them:

- **Each LLM endpoint names its account,** and each repository its git
  identity, so the engine knows which accounts each run needs.
- **Each account names its provider, its token endpoint and its client.**
- **The engine knows each worker's secret by the worker's name,** and a
  worker knows its own.
- **One thing must outlive the process:** an LLM account's refresh token,
  once rotated. The engine's protocol layer makes it durable before the
  access token it bought is handed out (section 6). Where it is kept is
  not decided here, beyond its carrying a version, as everything that
  outlives a process does (protocol.md, section 3).

## 4. Names, values and the tables

- **A credential's name is `(account, generation)`.** `account` is the
  deployment's index for it. `generation` counts the access tokens an LLM
  account has bought since it was granted; a static secret stays at
  generation zero. Names cross the domain boundary as plain integers.
- **A name never means two values, across restarts too.** A worker or an
  agent may still hold a grant from the engine's last life when the next
  one speaks. So an account's generation is kept with its rotated tokens
  (section 6), and a restart resumes from it: the engine grants the kept
  token again under its kept generation, and its next refresh buys the
  one after. An account given anew at startup, after a revocation,
  starts above the generation kept for it.
- **A value is the token, and what goes beside it:** a bearer token and,
  for ChatGPT, the account id read once from the token's claims by the
  engine. On the channels they are two byte fields (channel.md, section
  9). Every value is under `Limits::token_bytes`.
- **Each protocol layer has a table, indexed by account,** holding at most
  the two newest generations of each.
  - **The engine's** also holds each LLM account's refresh token, which
    never leaves the engine.
  - **The worker's** holds what grants brought it.
  - **The agent's** holds the grants of its run.

  The table is sized at startup from the configured accounts, so it never
  grows.
- **Two generations, so a message in flight still finds its value.** A
  grant for generation g can be encoded just after g+1 has arrived; the
  table still holds g, and fills it in. A grant naming a generation older
  than both is dropped as it is encoded, since a newer grant follows it.
- **An entry goes when its token has expired,** on the holder's own clock,
  or when a newer generation pushes it out. No domain tells a table to
  forget: names carry no lifecycle beyond the token's own.

## 5. Accounts: the engine's child domain

`temper-engine-domain-accounts` joins the engine's tree (engine-domain.md,
section 3). It is a capability, like the fleet, and keeps no secret.

- **An account's state:**
  - *Starting:* given only a refresh token, so it refreshes at once and
    is unusable until that succeeds;
  - *Fresh:* its generation, and its deadline: when the token expires,
    on the engine's clock;
  - *Refreshing:* a refresh in flight, which names the generation it
    buys (the current one plus one), echoed when it ends;
  - *Retrying:* the last refresh failed transiently; the next one is due
    after a backoff;
  - *Revoked:* the provider refused the refresh token; only a person can
    help.
  - *Spent* is an overlay on the others: the provider said the account's
    usage is exhausted, until a time.
- **Transitions:**

  | State | On | Goes to |
  |---|---|---|
  | Starting | the engine starts | Refreshing: asks for a refresh |
  | Fresh | its deadline less the margin | Refreshing: asks for a refresh |
  | Fresh | a `Rejected` naming the current generation, past the minimum interval since the last refresh | Refreshing |
  | Fresh | a `Rejected` naming an older generation | Fresh, unchanged |
  | Refreshing | `Refreshed { generation, valid }` | Fresh, the new generation granted to every live attempt that uses the account |
  | Refreshing | failed: unavailable, timed out or rate limited | Retrying, the backoff doubled up to its ceiling |
  | Refreshing | failed: refused | Revoked; people are told |
  | Retrying | the backoff's end | Refreshing |
  | Retrying | its token's deadline | Retrying, now unusable: the runs that need it wait |
  | any | `Exhausted { account, retry_after }` | Spent until then, unusable; the runs that need it wait |
  | Revoked | a new grant from a person, after a restart | Fresh |

- **Usable or not, for the work hub.** An account is usable while it is
  fresh, or refreshing or retrying with time left on its token, and not
  spent. A run whose charter needs an unusable account is not started:
  the hub treats it as waiting, as it would for a slot. The accounts
  domain tells its parent when an account becomes usable again, and the
  hub then starts what waited for it. A live run is not cancelled for its account: it fails
  on its own if its calls do, and the hub's retry policy takes over.
- **People are told** when an account becomes revoked, or spent beyond a
  threshold. The account's state is a fact that views carry to watchers
  (engine-domain.md, section 11), and the engine logs it. How a person is
  reached beyond that is open (section 11).
- **Limits:** the accounts; the refresh margin; the backoff's base and
  ceiling; the minimum interval between refreshes that a `Rejected`
  forces; and the threshold past which a spent account is reported.
- **Which attempts use which accounts.** The engine's configuration maps
  each LLM endpoint to an account, and each repository to a git identity.
  A charter's models name their endpoints, and an assignment's workspace
  names its repositories, so the top level knows each live attempt's
  accounts. It routes each `Granted` from accounts to the fleet, as a
  `Grant` for each of those attempts.

## 6. Refreshing

- **The exchange** is the engine's protocol layer's: an HTTP POST to the
  account's token endpoint, over TLS. Its body is JSON:
  `grant_type=refresh_token`, the client's id and the refresh token. The
  answer carries a new access token, perhaps a new refresh token, and how
  many seconds the access token lasts. The stack is that of any HTTP
  client in temper (llm.md, forge.md): socket, TLS, HTTP, JSON tokens,
  and a small decoder in `temper-oauth`, whose server side the fake LLM
  provider uses.
- **Kept before it is used.** The protocol layer makes the rotated
  refresh token durable, with the new access token and its generation
  (section 4). Only once that has
  completed does it tell the domain `Refreshed`.
  - A crash before then loses the new tokens. The old refresh token is
    still the one kept, and the provider either still takes it, or
    refuses it and the account is revoked.
  - A crash after loses nothing.
- **Keeping that fails** holds the new tokens in the table, fails the
  refresh as unsaved, and is tried again on the domain's backoff. Until it
  succeeds, the account is usable but at risk, and people are told.
- **Failures,** classified for the domain:
  - unavailable: the request provably never left;
  - timed out: no answer, or a dropped connection after sending;
  - rate limited, with a reset;
  - refused: the provider answered `invalid_grant` or the like: the
    refresh token is no good.
- **The provider's extras.** ChatGPT's requests name the account, whose id
  is a claim in the access token. The engine's protocol layer decodes the
  token's payload (base64url, then JSON) once per refresh, and stores the
  id with the token. Agents never parse tokens.

## 7. Grants on the channels

The messages are `channel.md`'s; their meaning is this section's.

- **Down: `Grant { run, attempt, account, generation, valid }`** plus the
  value, which the sender's protocol layer fills in and the receiver's
  takes out.
  - **The engine sends the attempt's first grants inside its
    assignment,** so the git grant cannot arrive after the preparation
    that needs it, and later ones as `Grant` messages: after every
    refresh, and after every hello for the attempts it keeps
    (channel.md, section 9).
  - **The worker keeps the git grant** for its own git operations, and
    passes LLM grants on to the agent of that attempt.
  - **Every grant is fenced by attempt,** like every other message
    (worker-domain.md, section 2).
- **Up, from agent to worker to engine, two notices:**
  - `Rejected { run, attempt, account, generation }`: the provider refused
    that token as unauthorized;
  - `Exhausted { run, attempt, account, retry_after }`: the provider said
    the account's usage is spent. `retry_after` is a duration, as every
    time on the channels is.

  The worker passes both through.
- **`valid` is a duration,** the time left on the token when sent. The
  receiver's deadline is `env.now + valid - Limits::skew`: transit only
  shortens what is left, and `skew` covers the time spent in queues.
- **The agent's side:**
  - the start message names each endpoint's account, as the worker's
    protocol layer completes it from the worker's endpoint table (llm.md);
  - the agent's top level holds the newest grant per account, and gives
    each `Complete` the name to use;
  - a call that fails as unauthorized sends `Rejected` up, once per
    generation, and fails to the session, which tries again after its
    backoff, by when a fresh grant has usually come;
  - with no grant left in time, a call fails as unauthorized at once,
    without reaching the provider.
- **Git's side.** An assignment's repositories each name their identity,
  which is now the git grant's account (worker-domain.md, 4.1). The
  worker's protocol layer gives each git invocation the token in that
  process's environment only: `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_0` and
  `GIT_CONFIG_VALUE_0` set `http.extraHeader` to the authorization
  header. Agents run in their own process trees, so they never see a git
  process's environment (section 11).

## 8. The static secrets

- **A worker's secret** goes in its hello, over TLS between hosts
  (channel.md). The engine compares it in constant time with the one it
  knows for that worker's name; a mismatch is refused before the domain
  hears of the worker.
- **The webhook secret** checks each webhook's signature (forge.md).
- **The forge's API token** goes in the engine's forge requests' headers
  (forge.md).

## 9. What the domains owe

- **The engine:**
  - the accounts child domain (section 5);
  - in the fleet, `Grant` down and `Rejected` and `Exhausted` up, all
    fenced by attempt;
  - at the top level, each live attempt's accounts, from configuration
    mapping endpoints and repositories to accounts;
  - in the hub, waiting for an unusable account as for a slot;
  - in views, the accounts' states as facts.
- **The worker:**
  - an assignment's grants, by name;
  - LLM grants passed on to the agent;
  - each repository's identity, as the git grant's account;
  - `Rejected` and `Exhausted` relayed up.
- **The agent:**
  - grants held by name at the top level, and the name given with each
    completion request;
  - `Rejected` and `Exhausted` sent up;
  - in the session, `Unauthorized` becomes transient (it tries again
    after its backoff), and an exhausted account becomes a failure of its
    own (llm.md), which ends the run as transient for the engine to retry
    once the account is usable.

## 10. Testing

- **The accounts world:** the child domain with the world as its parent
  and a scripted token endpoint. Refreshes due by deadline, early
  refreshes forced by `Rejected` and throttled by the minimum interval,
  transient failures with backoff, revocation, spent accounts, and
  writes that fail.
- **The fake LLM provider's OAuth server** rotates refresh tokens and
  refuses a spent one, as the real providers may. Its faults: the refresh
  answer lost after rotating, an unavailable endpoint, a slow one.
  Bearer tokens expire, and a call with an expired or unknown token is
  refused as unauthorized.
- **The system worlds** gain tokens that expire in the middle of runs, an
  engine that restarts between refreshing and writing, workers that
  redial and are sent grants again, and an account revoked or spent under
  live work.
- **Protocol worlds** check that a token's value never appears in what
  crosses a domain boundary. The world's trace of each domain's records is
  searched for the values the fakes issued.

## 11. Open questions

- **A refresh whose answer is lost** may have spent a rotating refresh
  token on the provider's side. Retrying with the old one then fails as
  refused, and the account needs a person. Whether providers allow the
  old token for a grace period decides whether this is rare or a real
  risk.
- **Logging in through the web,** with PKCE, so an account can be added or
  re-authorised from temper itself.
- **Telling people about an account:** beyond views and the log, a forge
  issue or the web's status page.
- **Git's environment.** This assumes agents cannot read a git process's
  environment, which holds when their containment gives them a process
  namespace of their own (testing.md, section 10, the real loop's
  privileges). Without one, the token goes to git through a pipe and a
  credential helper instead.
- **Short-lived git credentials:** Forgejo's tokens do not expire. A
  forge that issues short-lived ones (GitHub's app installation tokens)
  would make the git identity an account the engine refreshes, as it does
  LLM accounts.
