# Integrations: priority order and a generic integration layer

**Date:** 2026-08-21
**Status:** Approved, ready for an implementation plan
**Covers:** roadmap steps 7 and 8

## Why this document exists

Chief reads GitHub. The next question is which service it reads second, and the answer is not
obvious: the services worth adding differ by two orders of magnitude in how hard they are to
integrate, and the reasons are almost never about writing Rust. They are about whether a desktop
binary that holds no secret can authenticate at all, and about what a provider charges — in money,
in review time, or in throttling — for the privilege of reading a user's own data.

This document records what was found, the order that falls out of it, and the design for the first
two steps.

## The constraint that decides everything

Chief has no server. There is nowhere to keep a client secret, so a provider is viable only if a
distributed binary containing no confidential credential can complete an authentication handshake.
GitHub passes because its device flow needs no secret. That single test disqualifies or reshapes
almost every other provider on the list.

A second constraint follows from the first: there is no server to receive an OAuth redirect, so the
only options are a loopback listener on `127.0.0.1` or a custom URL scheme registered with the OS.

## Findings

Every claim below was researched against primary developer documentation and then put through an
independent adversarial pass whose brief was to refute it. Where the two disagreed, the refutation
is recorded.

### Microsoft — viable, cleanest of the six

Entra ID public clients are explicitly secret-less: _"Public clients, which include native
applications and single page apps, must not use secrets or certificates when redeeming an
authorization code."_ The `client_secret` parameter is annotated _"required for confidential web
apps"_ with a direct instruction not to use one in a native app. PKCE with S256 is first-class.

**The device flow is not available in practice.** This is the most important difference from
GitHub. Security defaults now include "Blocking device code flow"; as of 1 July 2026 all new Entra
tenants block it by default; and a Microsoft-managed Conditional Access policy named "Block device
code flow" is created in eligible tenants and enabled automatically. Microsoft's own guidance says
to _"get as close as possible to a unilateral block"_. Personal Microsoft accounts are unaffected,
but no org story can be built on it. Outlook therefore needs authorization code + PKCE + loopback.

**Org consent is the real cost.** Risk-based step-up consent is on by default and blocks ordinary
users in foreign tenants from consenting to a post-November-2020 multi-tenant app that is not
publisher verified and asks for more than basic sign-in. The user sees `AADSTS90094`. Publisher
verification is free but requires a verified Microsoft AI Cloud Partner Program account, and
_"apps that are registered by using a Microsoft account can't be publisher verified"_.

Corrections the adversarial pass made, all absorbed into the design below:

- Conditional Access "require compliant device" was **overstated** as unworkable. Device-based
  Conditional Access is satisfiable through a supported system browser, which is exactly the flow
  recommended here. It depends on the fleet's browser configuration; it is not categorically
  impossible.
- `x-ms-throttle-priority` is **not documented for Outlook** — only for Entra directory resources.
  A first draft proposed designing daemon yielding around it. Do not.
- An arbitrary custom scheme such as `chief://oauth/callback` is **not blessed** by Entra. The only
  documented shapes are `msauth.<bundle.id>://auth` and `msal{client_id}://auth`. Use loopback.
- "Allow public client flows = Yes" is **not documented** as required for auth-code + PKCE; what
  makes it a public client is registering the redirect under "Mobile and desktop applications".
  Harmless to enable, but do not document it to users as a required step.

### Google — viable, but Gmail is a different product from Calendar and Tasks

Google issues a `client_secret` for Desktop clients and documents it as not confidential:
_"(In this context, the client secret is obviously not treated as a secret.)"_ Shipping it is the
sanctioned design, not a compromise. Loopback on an ephemeral port is required; the out-of-band
copy-paste redirect has been fully dead since 31 January 2023. The device flow is useless here —
it supports a closed list of scopes that includes no Gmail, Calendar or Tasks scope.

**There is no cheap Gmail tier.** `gmail.readonly` and `gmail.metadata` are _both_ restricted
scopes and there is no lesser Gmail read scope. Restricted triggers an annual App Defense Alliance
CASA security assessment, paid by the developer, repeated at least every twelve months. The
adversarial pass explicitly rejected the argument that a serverless architecture mitigates this:
_"Applications requesting access to restricted scopes must undergo an annual security assessment"_,
unqualified.

Calendar and Tasks are merely _sensitive_: free review, but it needs a verified domain, a hosted
privacy policy linked from both the homepage and the consent screen, and a demonstration video.

An unverified app is capped at 100 users **for the lifetime of the Cloud project**, cannot be
reset, and shows every user a "Google hasn't verified this app" interstitial. In testing status,
refresh tokens expire after seven days.

### Atlassian — classic OAuth is impossible; the MCP server's OAuth is not

Classic 3LO is settled by Atlassian's own discovery document at
`auth.atlassian.com/.well-known/openid-configuration`, fetched live during verification:
`token_endpoint_auth_methods_supported` is `["client_secret_basic", "client_secret_post"]`. There
is no `none`. **Every 3LO client is a confidential client.** PKCE is genuinely supported
(`S256`) but it hardens the code exchange on top of client authentication — it never replaces it.
Reading "Atlassian supports PKCE" and concluding a public client is possible is a trap.

**But the Rovo MCP server runs a different authorization server, and that one is open to public
clients.** See "AI-agent access paths" below for the evidence. That makes DCR-over-MCP the
**primary** Atlassian path and demotes the pasted API token to a fallback.

The fallback still works and is still needed: a user-created scoped API token, used as HTTP Basic
with their email, carries both Jira and Confluence scopes, has read-only variants for both, and
must be called against `api.atlassian.com/ex/jira/{cloudId}`, never `<site>.atlassian.net` —
mixing the two is a common source of 401s. It is the answer for Free-tier sites and for
organisations that disable the MCP server.

The adversarial pass corrected one claim: "no infinite tokens any more, 365 days maximum" is
**false**. Non-expiring user API tokens still exist for managed users under an authentication
policy, though the expiration control requires Atlassian Guard Standard. The annual re-paste
hazard is real but softer than first reported.

Admins can block user API tokens, and since April 2025 that control is available to all cloud
customers rather than only Guard subscribers — so assume a non-trivial share of corporate tenants
have it on. The failure is at least legible: the user is told an admin blocked it.

### Slack — trivial auth, crippled data access, and MCP is gated harder still

PKCE went generally available on 30 March 2026, explicitly for _"desktop apps and mobile apps,
without the need to embed a vulnerable client_secret"_. Custom schemes and loopback are both
supported. The refresh also needs no secret; an apparent documentation conflict on that point was
checked during verification and resolved in Chief's favour. As authentication, this is easier than
the GitHub device flow.

The data access is the problem. Since 29 May 2025, an app commercially distributed outside the
Slack Marketplace gets `conversations.history` and `conversations.replies` at **Tier 1 — one
request per minute, with the `limit` parameter's maximum and default both forced to 15 objects**.
A work-log daemon would exhaust its entire minute on a single channel. The modern replacements are
closed by app class rather than by scope: the Real-time Search API is _"available for
directory-published apps and internal apps only"_, and the hosted MCP server states _"unlisted apps
are prohibited"_.

The escape hatch, a Marketplace listing, is closed to the app Chief wants to be: `search:read` is
named on the unsuitable-scopes list, "export or backup message data" and "replicate Slack client
functionality" are both listed as unsuitable, user-token `*:history` scopes are _"unlikely to be
approved"_, and there is a five-workspace, ten-weekly-active-user floor before submission is even
considered. Separately, the API terms bar commercial distribution outside the Marketplace.

A user-created app is an _internal customer-built app_ and is exempt from all of it, keeping
50+ requests/minute with `limit` up to 1000. For Slack, bring-your-own is not a consolation prize;
it is strictly the better integration.

### LinkedIn — not buildable as an API integration

Two independent walls, either one fatal.

`client_secret` is truly required: the token endpoint marks it `Required: Yes` and rejects its
absence with a dedicated 400. There is no device flow — verification fetched the live discovery
document at `linkedin.com/oauth/.well-known/openid-configuration` and found no
`device_authorization_endpoint`. A public-client PKCE endpoint does exist and would be perfect, but
it is enabled per-app by _"your point of contact at LinkedIn"_, which is partner language.

Even granting the auth problem away, **the read permission does not exist**. The complete list of
permissions available without approval is `profile`, `email`, and `w_member_social` — a name, an
email address, and the ability to _write_. `r_member_social` is _"restricted and available to
approved users only"_ and appears in no requestable product anywhere. `r_compliance` is listed
under a heading that says access _"is closed and may not be requested"_. Community Management, the
only programme carrying member-level read scopes, is for _"registered legal organizations for
commercial use cases only"_.

The honest alternative is not an API at all: every member can download an archive of their own data
from LinkedIn settings, and Chief can import and parse it entirely on-device. Zero outbound
requests, zero tokens, no review, no geographic restriction. It fits Chief's constitution better
than any OAuth path would.

## AI-agent access paths: MCP

The question was whether an official MCP server, or any other AI-agent-oriented access, would
dissolve the no-secret constraint — the hope being that MCP's authorization spec mandates OAuth 2.1
with Dynamic Client Registration, which mints a client id at runtime so nothing is baked into the
binary. It was researched per provider and adversarially verified with live probes against
production hosts.

**The answer is no for four of five providers, and yes for Atlassian.** This section exists because
the question will recur every time a vendor announces an MCP server, and the reasoning should not
have to be rediscovered.

### DCR is deprecated, not ascendant

The premise fails at the spec layer before any vendor is consulted. The normative level for
Dynamic Client Registration has moved in exactly the wrong direction:

| Revision               | Level                                                                  |
| ---------------------- | ---------------------------------------------------------------------- |
| `2025-06-18`           | SHOULD                                                                 |
| `2025-11-25`           | Demoted; Client ID Metadata Documents introduced as the preferred path |
| `2026-07-28` (current) | MAY, and **deprecated**                                                |

It sits in the formal deprecated-features registry with an earliest removal of 2027-07-28, framed
as _"new implementations SHOULD NOT adopt it"_. The stated reason is abuse: open registration
endpoints create unbounded anonymous client records and enable impersonation and flooding.

The successor is **worse** for Chief. A Client ID Metadata Document requires the client to host
JSON at a stable HTTPS URL it controls, and that URL _is_ the `client_id`. The authorization server
fetches it at every sign-in, so a lapsed domain breaks every installed copy. That is a permanent
off-machine dependency for a project whose whole claim is that nothing of it exists off the user's
machine — and it is allowlistable by design, so hosting it guarantees nothing.

### What DCR solves, precisely

It establishes **client identity** at runtime, per installation. That is a real win and worth
stating without hedging: a secret obtained through DCR is not a shipped secret, because Chief's
constraint is about _distribution_, not _possession_.

It solves nothing else. User consent, tenant admin policy, publisher verification and scope tiers
are all enforced at the authorize step, downstream of registration — and a dynamically registered
client is by definition unverified, so such policies are more likely to block it, not less. Every
wall in the Findings section above is one of those. DCR is architecturally incapable of touching
any of them.

### Per provider

- **Atlassian — adopt it.** `mcp.atlassian.com` delegates to the issuer
  `https://auth.atlassian.com/VCeDsk8ZHncYF1g234fKtc4lNipbBhu3`, a different authorization server
  from classic 3LO, whose metadata advertises a `registration_endpoint` and
  `token_endpoint_auth_methods_supported` including `none`. Anonymous registration returns HTTP 201
  with `"none"` echoed back rather than silently upgraded. The decisive control is the pushed
  authorization request endpoint, which is the one place a client is resolved before consent: it
  accepts the dynamically registered client and rejects a fabricated one with `invalid_client`.
  Two weaker proofs were discarded during verification — both `/oauth/token` with a bad code and
  `/authorize` behave identically for a never-registered client id, so neither shows anything.
- **Microsoft — clearly worse than REST.** Work IQ Mail/Calendar is preview, requires a Microsoft
  365 Copilot licence, and its protected-resource metadata names the `/organizations` authority
  rather than `/common`, so personal Microsoft accounts are excluded at the protocol level — reach
  the direct REST path already has. Admin consent is mandatory, and there is no read-only tier:
  granting the Mail server grants `sendMail`. Entra publishes no `registration_endpoint` and
  Microsoft has said DCR is not on the roadmap.
- **Google — no change.** The restricted-scope policy attaches to the **scope**, not the API
  surface, so it follows the data through the MCP door. The Gmail MCP server's own
  `scopes_supported` are five scopes, all of them Restricted — there is no non-restricted route in.
  It is also preview-gated and Workspace-only, and asks for a write scope alongside read.
- **Slack — gated harder than REST.** _"Only apps published in the Slack Marketplace and internal
  apps can use MCP at this time; unlisted apps are prohibited."_ A distributed Chief is banned
  outright from MCP, while the same Chief may still call a throttled `conversations.history`. The
  live metadata has no `registration_endpoint` and offers only `client_secret_post`. MCP is
  available to a user's own internal app, which makes it part of the bring-your-own story rather
  than an alternative to it.
- **LinkedIn — nothing to adopt.** No first-party server exists. Third-party ones either terminate
  the user's token on someone else's infrastructure or are browser-cookie scrapers whose own
  documentation warns of account restriction. One publishes **forged** metadata claiming
  `issuer: https://www.linkedin.com` and a registration endpoint that 404s — a spec-following
  client would discover it and act on fiction, which is a reason to pin hosts rather than follow
  discovery wherever it leads.

### The context budget forbids a general MCP client

This is independent of authorization and would rule out general adoption even if every vendor
supported DCR. `CONTEXT_SIZE` is **4096** (`src-tauri/src/engine.rs`), and that window holds the
system prompt, the clock preamble, every tool schema, the conversation, the tool results and the
answer. A measured Notion MCP toolset is 24 tools and **17,161 tokens — 4.2× the entire window**;
Atlassian's exposes roughly 37 tools. Around 97% of that cost is `inputSchema`, not descriptions.

The model compounds it. Meta's own documentation says the **8B** _"can not reliably maintain a
conversation alongside tool calling definitions"_, and Chief runs 3B — which the Berkeley Function
Calling Leaderboard removed outright. Reported accuracy for the 1–3B class tops out around 66%
_after_ fine-tuning Chief does not do, and multi-turn is the weakest category, which is exactly
what `MAX_TOOL_ROUNDS` implies.

Every mitigation collapses to the same place: Chief filters `tools/list` down to a handful
client-side. At which point a general MCP client has bought a protocol, a transport, an OAuth
discovery chain and an MSRV bump from 1.77.2 to 1.88 to arrive at a hand-picked list of four
tools — which is `src-tauri/src/tools.rs` today. So Atlassian gets a narrow module that speaks just
enough of the protocol, not a general client.

### Two rules that follow

**Deterministic tools only.** Atlassian's `searchAtlassian` and `fetchAtlassian` are the Rovo
natural-language layer; calling them would send the user's typed question to Atlassian. The
deterministic tools — `searchJiraIssuesUsingJql`, `getConfluencePage`, `getJiraIssue` — take
structured arguments Chief builds. Only those may be used. `src/components/Sidebar.tsx` promises
"Nothing you type leaves this machine" on every screen of the app, and that promise is load-bearing.

**Never declare the `sampling` capability.** A server that can request sampling can run its own
agentic loop on the user's local model. Chief's client capabilities must be empty, and that should
be an asserted test rather than a default.

### Honest caveats on the Atlassian path

- The DCR endpoint is **undocumented by Atlassian**. It can be withdrawn without notice, so the
  pasted API token stays implemented as a fallback rather than being deleted.
- **Cloudflare hosts the MCP server.** Atlassian's own sub-processor page names it _"Hosting
  provider for the Remote Model Context Protocol (MCP) Server"_, and header probes confirm the
  asymmetry — `cf-ray` appears on `mcp.atlassian.com` and only there, while `api.atlassian.com`
  and `auth.atlassian.com` route via CloudFront. It is a disclosed sub-processor under Atlassian's
  DPA rather than an independent broker, so it stays within the rule about services the user
  connected — but the MCP path is not topologically identical to the REST path, and should not be
  described as though it were.
- **Two independent admin gates.** A redirect-URL domain allowlist, on which `127.0.0.1` and
  `localhost` ship by default, and per-tool permission groups that can revoke `read_confluence`
  on its own.
- **Rovo requires a paid plan**, so Free-tier sites are excluded. Not verified against a primary
  source; treat as likely rather than certain.
- Write scopes sit on the same resource as read scopes, so read-only is enforced by what Chief
  asks for. Requesting only read scopes still makes it enforceable at the authorization server,
  which is better than the broad `repo` scope Chief settles for on GitHub.

## The order

Ranked by the complexity scores that survived adversarial review, with the caveat that each score
fuses implementation effort and user cost, which frequently disagree.

| #   | Integration             | Mechanism                             | Score | Dominant cost                                        |
| --- | ----------------------- | ------------------------------------- | ----- | ---------------------------------------------------- |
| 1   | Outlook Mail + Calendar | Loopback + PKCE, public client        | 6     | Org consent wall without publisher verification      |
| 2   | Google Calendar + Tasks | Same machinery, reused                | ~5    | Verified domain, privacy policy, demo video          |
| 3   | Jira + Confluence       | DCR-over-MCP, PKCE, no shipped secret | 7     | Undocumented mechanism; paid plan; Cloudflare-hosted |
| 4   | Gmail                   | Bring-your-own Cloud project          | 8     | Annual paid CASA assessment otherwise                |
| 5   | Slack                   | Bring-your-own internal app, REST     | 9     | Distributed apps throttled to 1 req/min, 15 messages |
| 6   | LinkedIn                | Archive import, not an API            | 10    | No read scope exists at any self-serve tier          |

The order is build order, not a sort by score. Google Calendar + Tasks scores lower than Outlook
precisely because its ~5 assumes the loopback and PKCE machinery already exists — and Outlook is
what builds it. Sequencing Google first would move that cost onto Google and raise its score to
roughly Outlook's, while delivering one surface instead of two and adding review paperwork before
anything could ship.

Ranks 2 and 3 may swap. Google Calendar reuses machinery Outlook has already built and needs no
new protocol, but it cannot ship until brand and sensitive-scope review clear. Atlassian needs no
review of any kind and its user-facing sign-in is now a browser click, but it costs a slice of MCP
protocol surface and rests on an undocumented registration endpoint. That is a judgement about
appetite for paperwork versus appetite for a mechanism that could be withdrawn, and it can be made
when step 8 lands rather than now.

### Where Chief's own app is shipped, and where it is not

Chief bakes in its own public client id for **GitHub, Outlook, and Google Calendar + Tasks**, where
that genuinely works and the user registers nothing.

Chief registers itself **at runtime** for **Atlassian**, via Dynamic Client Registration against
the Rovo MCP server's authorization server. No client id is baked in and no secret is shipped:
both are minted per installation and stored in that user's own local database.

Chief offers **bring-your-own-app** for **Gmail** and **Slack**, where the shipped app is worse —
Gmail because of CASA, Slack because a self-registered internal app escapes rate limits that make
the distributed one useless — and as a fallback for **any provider under an org policy that blocks
Chief's own registration**.

This is why `integration_accounts.client_id` and `client_secret` exist from the first migration:
they hold a bring-your-own registration and a DCR-minted one alike, so neither path needs a
schema change later.

## Step 7 — a generic integration layer

The refactor lands on its own, with GitHub as its only consumer and its existing tests proving
nothing broke. If the abstraction is wrong, that is discovered while only one provider rides on it.

### Migration v3

`integrations` has `service_name TEXT NOT NULL UNIQUE`, and SQLite cannot drop a constraint, so v3
creates a new table, copies the existing row across, and drops the old one. Migrations remain
append-only in the sense that matters: no shipped migration is edited.

```sql
CREATE TABLE integration_accounts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    service         TEXT NOT NULL,
    account_key     TEXT NOT NULL,
    label           TEXT,
    identity        TEXT,
    credential_kind TEXT NOT NULL DEFAULT 'oauth',
    access_token    TEXT NOT NULL,
    refresh_token   TEXT,
    expires_at      TEXT,
    scopes          TEXT,
    client_id       TEXT,
    client_secret   TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (service, account_key)
);
```

Column rationale, since each one is a decision:

- `account_key` — the provider's stable identifier for the account. Uniqueness is per service, so
  the same person's work and personal Outlook are two rows.
- `label` — user-supplied, e.g. "Work". Optional; `identity` is shown when it is absent.
- `identity` — what to display, and for HTTP Basic providers also what to send: an email address,
  an Atlassian site, a Slack workspace.
- `credential_kind` — `oauth`, `token`, or `dcr`. Explicit, so Atlassian's pasted credential is not
  inferred from which columns happen to be NULL, and so a dynamically registered client is
  distinguishable from a baked-in one at a glance.
- `expires_at` — Graph access tokens last about an hour. Today's renew-reactively-on-401 works but
  wastes a round trip on every first call after expiry.
- `scopes` — what was actually granted, which is not always what was asked for.
- `client_id` — NULL means Chief's own baked-in registration. Set means either bring-your-own or,
  for Atlassian, a client minted at runtime by Dynamic Client Registration.
- `client_secret` — normally NULL. Atlassian's DCR response returns one even when the client
  registers with `token_endpoint_auth_method: "none"`, and a user who brings their own Atlassian
  3LO app has one too. **Neither is a shipped secret**: both belong to one installation and never
  leave that machine, which is the distinction Chief's rule actually draws. Without this column,
  adopting the Atlassian path would need a migration v4 for a value the provider hands over on
  day one.

The existing GitHub row migrates with `account_key = 'github'` and `identity = NULL`. The Settings
screen shows it as connected without an identity until `Session` backfills one from the provider's
own "who am I" endpoint on the next successful read.

v3 also adds `work_logs.account_id` and rebuilds the dedupe index:

```sql
ALTER TABLE work_logs ADD COLUMN account_id INTEGER;

DROP INDEX IF EXISTS idx_work_logs_external_id;
CREATE UNIQUE INDEX idx_work_logs_external_id
    ON work_logs (source, account_id, external_id)
    WHERE external_id IS NOT NULL;
```

Without this, two Atlassian sites each holding a `PROJ-123` would collide and the second would
silently never be logged. Existing rows inherit the migrated GitHub account, so nothing re-logs.

### New modules

```
src-tauri/src/oauth/
    mod.rs        Error, Tokens, re-exports
    pkce.rs       Verifier and Challenge (S256), state nonce. Pure and trivially testable.
    loopback.rs   Bind 127.0.0.1:0, return the port and a oneshot for the code.
    device.rs     The device-grant machinery lifted out of github.rs unchanged.
    flow.rs       authorize_loopback::<P>() and authorize_device::<P>().
```

A provider describes itself; the flow machinery is written once.

```rust
pub struct Endpoints {
    pub authorize: &'static str,
    pub token: &'static str,
    pub device_code: Option<&'static str>,
}

pub trait Provider {
    const SERVICE: &'static str;
    fn endpoints(&self) -> Endpoints;
    fn client_id(&self) -> Result<String, Error>;
    fn scopes(&self) -> &'static [&'static str];
    /// Only ever a value the provider documents as non-confidential (Google).
    fn client_secret(&self) -> Option<String> { None }
}
```

`github.rs` keeps its device-flow specifics and implements `Provider`. `session.rs` generalises
from `Session<'a>` to `Session<'a, P: Provider>` so renewal-and-retry-once is written once rather
than per provider, and gains proactive refresh when `expires_at` is near. `connect.rs`'s four
GitHub-named commands become `start_login(service)`, `finish_login(service)`, `connections()`,
`disconnect(account_id)` and `label_account(account_id, label)`.

A `CredentialStore` trait wraps every credential read and write, so moving tokens to the OS keychain
later is a swap of one implementation plus a one-time migration, rather than touching every
provider. CLAUDE.md already names that move as a genuine improvement; this is the seam for it.

### Loopback, not deep links

The redirect is a one-shot HTTP listener bound to `127.0.0.1` on an ephemeral port, hit by the
system browser, opened via the `opener` plugin that is already a dependency.

RFC 8252 §7.3 requires authorization servers to allow any port for loopback redirects and §8.4
exempts the port from exact matching, so one registered URI covers every run and no port needs
negotiating. §7.3 also states that _"the use of `localhost` is NOT RECOMMENDED"_ because it risks
listening on interfaces other than loopback. Binding `127.0.0.1` explicitly raises no macOS
firewall prompt; binding `0.0.0.0` does.

`tauri-plugin-deep-link` is rejected on five counts: Google refuses custom schemes for Desktop
clients outright; any application can register the same scheme, whereas a bound port has exactly one
holder enforced by the OS; on Windows and Linux the callback arrives as a CLI argument to a fresh
process, putting the authorization code on the process table; macOS requires a bundled `Info.plist`
so `tauri dev` cannot exercise it on Chief's primary platform; and it is three platform code paths
plus three dependencies against roughly sixty lines of `tokio::net::TcpListener`.

Hardening, all cheap: bind before generating the authorization URL and hold the port for the whole
flow; a single-shot listener that shuts down on the first matching request; a 120-second timeout;
accept only the exact callback path and 404 everything else; compare `state` before exchanging the
code.

RFC 8252 §4 requires an external user-agent and forbids embedded ones. This is not merely normative:
Google has returned `disallowed_useragent` to embedded webviews since July 2023. Chief must never
load an authorization endpoint in the Tauri WebView.

### What does not change

`tauri.conf.json` and `capabilities/default.json` are untouched. The listener is a Rust server
contacted by the _external_ browser; the renderer never fetches it, so
`connect-src 'self' ipc: http://ipc.localhost` remains correct, and `opener:default` already covers
launching the browser. This is worth stating explicitly because the instinct is to widen the CSP,
and doing so would weaken it for no reason.

Cargo gains `tokio`'s `net` feature (currently dev-only), `sha2` and `base64`. No new plugin crates.

### Frontend

`use-github.ts` becomes `use-integrations.ts`, `lib/integrations.ts` becomes
service-parameterised, and `SettingsView` grows a list of accounts per service with a label field
and a per-account disconnect. Chips continue to carry state, never actions.

### Daemon

`run_once` iterates connected accounts rather than assuming a single GitHub connection, and yields
to `Attention` between accounts as well as between items.

### Tests

- PKCE against the RFC 7636 Appendix B vectors.
- A full loopback round trip against a stub authorization server, with the `TcpListener` injected
  so the test drives it the way `daemon::run_once` takes a `Context`.
- A migration test that a v2 database holding a GitHub credential and work log entries survives v3
  with its entries intact and its dedupe still working.
- Every existing GitHub test passes unchanged. That is the point of doing this step alone.

## Step 8 — Outlook Mail and Calendar

One multi-tenant registration with `signInAudience = AzureADandPersonalMicrosoftAccount` against the
`/common` authority, so personal and work accounts share one code path.
`CHIEF_MICROSOFT_CLIENT_ID` overrides at run time and falls back to build time, mirroring GitHub.

Scopes: `Mail.Read Calendars.Read offline_access User.Read`. Full content, because Microsoft has no
restricted-scope tax to tier away from. `Tasks.Read` for Microsoft To Do is available later on the
same registration and the same API.

### Decisions, not discoveries-later

- **Bind `127.0.0.1` and send `127.0.0.1`.** Register it through the `replyUrlsWithType` manifest
  attribute, because the Entra portal's redirect text box refuses `http` with `127.0.0.1`. Sending
  `localhost` while binding only IPv4 breaks on any machine that resolves `localhost` to `::1`
  first, and Entra does not support `[::1]`. Do not register several localhost URIs differing only
  by port — the login server picks one arbitrarily.
- **`AADSTS90094` is a first-class error state.** It means risk-based step-up consent — on by
  default — blocked an unverified multi-tenant app. It must read as _your organisation requires an
  administrator to approve Chief_, naming what to ask for, not as a transport failure.
- **Do not design daemon yielding around `x-ms-throttle-priority`.** It is documented only for
  Entra directory resources. The real Outlook limit is 10,000 requests per 10 minutes per
  app-and-mailbox, which is vast for a single person.
- **Poll with delta queries.** Graph change notifications need a publicly reachable HTTPS
  validation endpoint, or Event Hubs / Event Grid — all unavailable to an app with no cloud
  footprint. Polling is a constraint, not a preference, and delta tokens make it cheap.
- **Refresh tokens are revoked on password change**, on self-service password reset, and by any
  admin "revoke all refresh tokens" action. Silent re-auth failure is routine and must surface as
  "reconnect Outlook", not as an error.

### The org story

Personal Microsoft accounts work on day one with no friction. Corporate tenants may hit the consent
wall, and the ladder out of it — publisher verification, an org registering its own app, or an
admin relaxing `BlockUserConsentForRiskyApps` — is additive and needs no code change, because
`client_id` is already a column.

## Out of scope

- Moving tokens to the OS keychain. The `CredentialStore` seam is built; the move is not.
- Writing to any connected service. Chief reads.
- LinkedIn as an API integration. If LinkedIn is wanted, it is an archive importer.
- Jira and Confluence Data Center / Server. A separate auth path (bearer PAT, customer-owned host,
  no v2 Confluence API) and a deliberate scope decision rather than a freebie.
- A general-purpose MCP client. See "AI-agent access paths" — the context budget forbids it and
  only one provider would benefit. Atlassian gets a narrow, purpose-built module instead.
- Atlassian's Rovo natural-language tools (`searchAtlassian`, `fetchAtlassian`). Deterministic
  tools only; see the privacy rule in that section.
- Telemetry of any kind, here as everywhere.
