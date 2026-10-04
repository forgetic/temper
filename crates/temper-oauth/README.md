`temper-oauth` implements the neutral token-exchange documents used by both the
engine client and fake provider server, per [credentials.md](../../docs/design/credentials.md),
sections 4–6. It depends on skein's bounded containers, JSON tokenizer and
measured writer, with no domain dependency and no I/O.

`RefreshState` holds the account, kept generation and refresh token required by
Starting or a later refresh. `rotate` builds the next `SavedToken`, retains the
old refresh token when the response omits a replacement, and refuses generation
or expiry overflow. The owner keeps that candidate before reporting Refreshed.
`SavedToken::remaining(wall)` must be read at keep completion or grant encoding;
the original expires_in must not be sent after time spent keeping the record.
The absolute Wall expiry is saved restart metadata. Channels send a remaining
Duration, as required by credentials.md section 7.

Saved records are big endian, with magic `TPOT`, u16 version 1, u32 account,
u64 generation, u64 expiry nanoseconds, then three u32-sized byte fields:
access token, refresh token and optional account id (zero length means absent).
Readers refuse unknown versions, trailing bytes, malformed lengths and fields
above their configured caps before allocating them. Records deliberately
contain secrets and belong only in the engine's secret store.

`read_claims` performs bounded unpadded base64url and JSON decoding once per
ChatGPT refresh. It reads `https://api.openai.com/auth.chatgpt_account_id` and
optional exp, returning remaining validity at its injected wall clock. It does
not verify signatures or authenticate a bearer: its tokens must have come
from the authenticated token endpoint. Rotation caps expiry by both expires_in
and a supplied exp claim. Account ids and bearer tokens cannot inject headers.

JSON decoders accept unknown extension fields under the full document, token,
string and depth bounds, and reject duplicate recognized fields. OAuth error
code and description are bounded; descriptions are untrusted peer content and
must stay outside domain records/logs. An answered transient server failure is
TimedOut (the exchange already left); only a transport proving no request left
may report Unavailable. HTTP 429 supplies RateLimited with the owner's parsed
retry delay; invalid_grant and authentication refusals supply Refused.

The focused tests use explicitly synthetic tokens and an independently written
binary golden. No test fixture here claims to be an actual OAuth capture.
`worst_case` counts per-exchange parsing, candidate and record temporaries and
two saved generations. The owner's fixed account table multiplies this bound
by its admission cap and accounts for its own container and queues.
