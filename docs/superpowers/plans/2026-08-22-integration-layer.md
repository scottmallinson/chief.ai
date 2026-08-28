# Generic Integration Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Chief's GitHub-shaped integration code into a multi-account, service-generic layer, with GitHub as its only consumer and its existing tests proving nothing broke.

**Architecture:** A new `oauth` module holds two pure, self-contained primitives — PKCE (RFC 7636) and a one-shot loopback redirect listener (RFC 8252 §7.3). Migration v3 replaces the one-row-per-service `integrations` table with `integration_accounts`, keyed on `(service, account_key)`. A `Provider` trait lets `Session` generalise so renewal-and-retry-once is written once instead of per provider, and `connect.rs`'s four GitHub-named Tauri commands become service-parameterised.

**Tech Stack:** Rust (Tauri v2, sqlx, reqwest, tokio), TypeScript (React 18, Vitest, Testing Library).

**Spec:** `docs/superpowers/specs/2026-08-21-integration-roadmap-design.md`

---

## Two deliberate deviations from the spec

Both narrow the spec's "New modules" list. Flagging them here rather than silently dropping them.

1. **`oauth/flow.rs` is deferred to step 8.** The spec lists `authorize_loopback::<P>()`. Writing it now means designing a generic composition against zero real consumers. `pkce.rs` and `loopback.rs` are built and fully tested here because they are genuinely self-contained; composing them belongs with the first provider that uses them, which is Outlook.

2. **`oauth/device.rs` is deferred indefinitely.** The spec proposes lifting GitHub's device-flow machinery into a shared module. It has exactly one consumer and, per the research, is likely to keep exactly one — Microsoft's device flow is being switched off and no other planned provider offers it. Extracting a shared abstraction from one example, at the cost of touching working sign-in code, is the wrong trade. GitHub keeps its device flow in `github.rs` and implements `Provider` alongside it.

3. **`CredentialStore` is a module boundary, not a trait.** The spec asks for a trait so the OS
   keychain becomes a swap of one implementation. `integrations.rs` already gives that property:
   every credential read and write goes through four functions in one file, and moving them to the
   keychain means changing those bodies. A trait with a single implementation adds indirection
   without adding a seam. If two stores ever coexist — keychain with a SQLite fallback on a Linux
   box with no Secret Service — introduce the trait then, against two real implementations.

If any of these calls is wrong, say so before Task 6 — that is the last task where reversing them
is cheap.

---

## File structure

**Created**

| File                              | Responsibility                                                                       |
| --------------------------------- | ------------------------------------------------------------------------------------ |
| `src-tauri/src/oauth/mod.rs`      | Module root; re-exports `pkce` and `loopback`; the `Provider` trait and `Endpoints`. |
| `src-tauri/src/oauth/pkce.rs`     | `Verifier` and `State`. Pure: no I/O, no clock, no provider.                         |
| `src-tauri/src/oauth/loopback.rs` | One-shot `127.0.0.1` redirect listener on an ephemeral port.                         |
| `src/hooks/use-integrations.ts`   | Replaces `use-github.ts`; drives sign-in for any service, over many accounts.        |

**Modified**

| File                                    | Change                                                                              |
| --------------------------------------- | ----------------------------------------------------------------------------------- |
| `src-tauri/src/db.rs`                   | Migration v3: `integration_accounts`, `work_logs.account_id`, rebuilt dedupe index. |
| `src-tauri/src/integrations.rs`         | Account-aware CRUD keyed on `(service, account_key)`.                               |
| `src-tauri/src/github.rs`               | Implements `Provider`; gains `viewer()` for account identity.                       |
| `src-tauri/src/session.rs`              | `Session<'a, P: Provider>`; renewal written once.                                   |
| `src-tauri/src/connect.rs`              | Four service-parameterised commands plus `label_account`.                           |
| `src-tauri/src/daemon.rs`               | Iterates connected accounts instead of assuming one GitHub.                         |
| `src-tauri/src/work_log.rs`             | `has_logged` and `NewWorkLogEntry` carry `account_id`.                              |
| `src-tauri/src/lib.rs`                  | Command registration.                                                               |
| `src-tauri/Cargo.toml`                  | Adds `sha2`, `base64`, `getrandom`; promotes `tokio/net`.                           |
| `src/lib/integrations.ts`               | Service-parameterised invoke wrappers; `Account` type.                              |
| `src/components/views/SettingsView.tsx` | Lists accounts per service; per-account disconnect and label.                       |

**Deleted**

| File                      | Reason                               |
| ------------------------- | ------------------------------------ |
| `src/hooks/use-github.ts` | Superseded by `use-integrations.ts`. |

**Deliberately untouched**

`src-tauri/tauri.conf.json` and `src-tauri/capabilities/default.json` do not change. The loopback
listener is a Rust server contacted by the _external_ system browser; the renderer never fetches
it, so `connect-src 'self' ipc: http://ipc.localhost` stays exactly as it is, and `opener:default`
already covers launching the browser. Widening the CSP for this would weaken it for no reason. If a
task seems to need either file edited, stop — something else is wrong.

---

## Task 1: Dependencies

**Files:**

- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1: Add the three crates and promote `tokio/net`**

In `[dependencies]`, change the `tokio` line to include `net` (it is currently only in `[dev-dependencies]`), and add the three new crates after `futures-util`:

```toml
tokio = { version = "1.53.1", features = ["fs", "io-util", "net", "process", "time"] }
futures-util = { version = "0.3.34", default-features = false, features = ["std"] }
# PKCE: SHA-256 for the code challenge, base64url for both halves, and the
# operating system's CSPRNG for the verifier and the state nonce.
sha2 = "0.10"
base64 = "0.22"
getrandom = "0.3"
```

- [ ] **Step 2: Verify it resolves and still builds**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: compiles clean.

Then confirm which `getrandom` you got, because the API differs between majors:

Run: `cargo tree --manifest-path src-tauri/Cargo.toml -p getrandom --depth 0`
Expected: `getrandom v0.3.x`.

Task 2 uses the 0.3 API, `getrandom::fill(&mut bytes)`. If the resolver pinned 0.2 instead, the equivalent call is `getrandom::getrandom(&mut bytes)` and it returns the same `Result` — change that one line and nothing else.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "build(deps): add sha2, base64 and getrandom for PKCE"
```

---

## Task 2: PKCE

A native public client MUST use PKCE and MUST use `S256` where it can (RFC 8252 §8.1), so `plain` is not implemented at all.

**Files:**

- Create: `src-tauri/src/oauth/pkce.rs`
- Create: `src-tauri/src/oauth/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/oauth/pkce.rs` containing **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B — the specification's own worked example.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn derives_the_challenge_from_rfc_7636_appendix_b() {
        assert_eq!(
            Verifier::from_stored(RFC_VERIFIER).challenge(),
            RFC_CHALLENGE
        );
    }

    #[test]
    fn generates_a_verifier_of_the_length_rfc_7636_allows() {
        let length = Verifier::generate().as_str().len();
        assert!((43..=128).contains(&length), "got {length} characters");
    }

    #[test]
    fn generates_a_different_verifier_every_time() {
        assert_ne!(Verifier::generate(), Verifier::generate());
    }

    #[test]
    fn uses_only_the_unreserved_characters() {
        let verifier = Verifier::generate();

        assert!(
            verifier
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'.' | b'_' | b'~')),
            "got {}",
            verifier.as_str()
        );
    }

    #[test]
    fn a_state_matches_only_itself() {
        let state = State::generate();

        assert!(state.matches(state.as_str()));
        assert!(!state.matches("something-else"));
        assert!(!State::generate().matches(state.as_str()));
    }
}
```

Create `src-tauri/src/oauth/mod.rs`:

```rust
//! OAuth machinery shared by every provider.
//!
//! Nothing here talks to a particular service. A provider describes itself and
//! these pieces do the protocol, so adding the second and third provider costs
//! a description rather than a flow.

pub mod loopback;
pub mod pkce;
```

Comment out the `pub mod loopback;` line for now — that file arrives in Task 3.

Add to `src-tauri/src/lib.rs`, in the module list after `mod integrations;`:

```rust
mod oauth;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml oauth::pkce`
Expected: FAIL to compile, `cannot find type 'Verifier' in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `src-tauri/src/oauth/pkce.rs`:

```rust
//! PKCE (RFC 7636) and the `state` nonce.
//!
//! Pure: no I/O, no clock, no provider, so every branch is testable against the
//! specification's own vectors. Only `S256` exists here — RFC 8252 §8.1 makes
//! PKCE mandatory for a native public client, and `plain` protects nothing.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// The secret half of a PKCE exchange: sent with the code, never before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verifier(String);

impl Verifier {
    /// A fresh verifier from the operating system's CSPRNG.
    ///
    /// 32 random bytes base64url-encode to 43 characters — the shortest RFC
    /// 7636 §4.1 permits, and a full 256 bits of entropy.
    pub fn generate() -> Self {
        Self(URL_SAFE_NO_PAD.encode(random_bytes()))
    }

    /// Rebuild a verifier held between the two halves of a sign-in.
    pub fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// What goes in the token request.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// What goes in the authorization request: BASE64URL(SHA256(verifier)).
    pub fn challenge(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }
}

/// A single-use value echoed back by the authorization server, checked before
/// the code is exchanged so another site cannot feed us one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State(String);

impl State {
    pub fn generate() -> Self {
        Self(URL_SAFE_NO_PAD.encode(random_bytes()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether a returned value is the one we sent. Compared without an early
    /// exit, which costs nothing at this length and avoids thinking about it.
    pub fn matches(&self, returned: &str) -> bool {
        self.0.len() == returned.len()
            && self
                .0
                .bytes()
                .zip(returned.bytes())
                .fold(0_u8, |differences, (ours, theirs)| {
                    differences | (ours ^ theirs)
                })
                == 0
    }
}

fn random_bytes() -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system should provide randomness");

    bytes
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml oauth::pkce`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/oauth/ src-tauri/src/lib.rs
git commit -m "feat(auth): add PKCE verifier and state nonce"
```

---

## Task 3: Loopback redirect listener

**Files:**

- Create: `src-tauri/src/oauth/loopback.rs`
- Modify: `src-tauri/src/oauth/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/oauth/loopback.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Act like the browser: GET the redirect and read the page back.
    async fn visit(uri: &str, query: &str) -> String {
        let authority = uri
            .trim_start_matches("http://")
            .split('/')
            .next()
            .expect("the redirect uri should have an authority")
            .to_string();

        let mut socket = tokio::net::TcpStream::connect(&authority)
            .await
            .expect("the listener should be accepting");

        socket
            .write_all(
                format!("GET {CALLBACK_PATH}?{query} HTTP/1.1\r\nHost: {authority}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("should send the redirect");

        let mut response = String::new();
        socket
            .read_to_string(&mut response)
            .await
            .expect("should read the page");

        response
    }

    #[tokio::test]
    async fn hands_back_the_code_and_state_the_browser_returned() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();

        let browser = tokio::spawn(async move { visit(&uri, "code=abc123&state=xyz").await });
        let redirect = listener.wait().await.expect("should receive the redirect");

        assert_eq!(redirect.code, "abc123");
        assert_eq!(redirect.state, "xyz");

        let page = browser.await.expect("the browser should finish");
        assert!(page.starts_with("HTTP/1.1 200 OK"), "got {page}");
        assert!(page.contains("go back to Chief"), "got {page}");
    }

    #[tokio::test]
    async fn listens_only_on_the_loopback_interface() {
        let listener = Listener::bind().await.expect("should bind loopback");

        assert!(
            listener.redirect_uri().starts_with("http://127.0.0.1:"),
            "got {}",
            listener.redirect_uri()
        );
        assert!(listener.redirect_uri().ends_with(CALLBACK_PATH));
    }

    #[tokio::test]
    async fn percent_decodes_what_the_browser_sent() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();

        let browser = tokio::spawn(async move { visit(&uri, "code=a%2Fb%2Bc&state=s").await });
        let redirect = listener.wait().await.expect("should receive the redirect");

        assert_eq!(redirect.code, "a/b+c");
        browser.await.expect("the browser should finish");
    }

    #[tokio::test]
    async fn reports_a_declined_sign_in_in_those_words() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();

        let browser = tokio::spawn(async move { visit(&uri, "error=access_denied").await });
        let error = listener.wait().await.expect_err("a refusal should surface");

        assert!(matches!(error, Error::Declined), "got {error:?}");
        browser.await.expect("the browser should finish");
    }

    #[tokio::test]
    async fn ignores_anything_that_is_not_the_callback() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let authority = listener
            .redirect_uri()
            .trim_start_matches("http://")
            .split('/')
            .next()
            .expect("should have an authority")
            .to_string();
        let uri = listener.redirect_uri();

        let stray = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(&authority)
                .await
                .expect("should connect");
            socket
                .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: local\r\n\r\n")
                .await
                .expect("should send");

            let mut response = String::new();
            socket.read_to_string(&mut response).await.ok();

            // Only once the stray request has been turned away does the real
            // redirect arrive, which is what proves the listener kept waiting.
            visit(&uri, "code=real&state=s").await;
            response
        });

        let redirect = listener.wait().await.expect("should receive the redirect");
        assert_eq!(redirect.code, "real");

        let turned_away = stray.await.expect("the stray request should finish");
        assert!(turned_away.starts_with("HTTP/1.1 404"), "got {turned_away}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml oauth::loopback`
Expected: FAIL to compile, `cannot find type 'Listener' in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `src-tauri/src/oauth/loopback.rs`:

```rust
//! The one-shot redirect listener.
//!
//! RFC 8252 §7.3 prefers a loopback redirect for a native app and requires the
//! authorization server to accept any port, so this binds an ephemeral one and
//! reports which it got — one registered redirect URI then covers every run.
//! The same section says `localhost` is NOT RECOMMENDED: binding the IP literal
//! cannot accidentally listen on another interface, and it raises no firewall
//! prompt.
//!
//! The port is bound *before* the authorization URL is built, so the URL always
//! names a port we already hold, and it is released as soon as the browser
//! comes back. Anything that is not the callback path gets a 404, so a stray
//! local request cannot be mistaken for a sign-in.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Give up if the user never finishes in the browser.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The only path this answers.
const CALLBACK_PATH: &str = "/oauth/callback";

/// A request line longer than this is not a redirect.
const MAX_REQUEST_LINE: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not listen for the sign-in redirect: {0}")]
    Bind(String),
    #[error("sign-in was not completed in time")]
    TimedOut,
    #[error("the browser did not come back with a sign-in code")]
    NoCode,
    #[error("sign-in was declined")]
    Declined,
}

/// What the browser handed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub code: String,
    pub state: String,
}

/// A bound port, waiting for the browser.
pub struct Listener {
    listener: TcpListener,
    port: u16,
}

impl Listener {
    /// Bind an ephemeral port on the loopback interface.
    pub async fn bind() -> Result<Self, Error> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| Error::Bind(error.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|error| Error::Bind(error.to_string()))?
            .port();

        Ok(Self { listener, port })
    }

    /// The redirect URI to send, which is the one we are listening on.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    /// Wait for the browser, answer it, and return what it carried.
    pub async fn wait(self) -> Result<Redirect, Error> {
        match tokio::time::timeout(TIMEOUT, self.accept_callback()).await {
            Ok(result) => result,
            Err(_) => Err(Error::TimedOut),
        }
    }

    async fn accept_callback(&self) -> Result<Redirect, Error> {
        loop {
            let (mut socket, _) = self
                .listener
                .accept()
                .await
                .map_err(|error| Error::Bind(error.to_string()))?;

            let Some(query) = read_request_line(&mut socket)
                .await
                .as_deref()
                .and_then(request_target)
                .and_then(|target| target.strip_prefix(CALLBACK_PATH).map(str::to_string))
            else {
                respond(&mut socket, "404 Not Found", "Not found.").await;
                continue;
            };

            let outcome = interpret(query.strip_prefix('?').unwrap_or(""));
            let body = match &outcome {
                Ok(_) => "Signed in. You can close this tab and go back to Chief.",
                Err(_) => "Sign-in did not complete. Go back to Chief and try again.",
            };
            respond(&mut socket, "200 OK", body).await;

            return outcome;
        }
    }
}

/// The target of a `GET`, or nothing for any other method.
fn request_target(request_line: &str) -> Option<&str> {
    let mut parts = request_line.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;

    (method == "GET").then_some(target)
}

/// Read one authorization response out of the redirect's query string.
fn interpret(query: &str) -> Result<Redirect, Error> {
    let mut code = None;
    let mut state = None;
    let mut failure = None;

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));

        match name {
            "code" => code = Some(decode(value)),
            "state" => state = Some(decode(value)),
            "error" => failure = Some(decode(value)),
            _ => {}
        }
    }

    if let Some(failure) = failure {
        return Err(match failure.as_str() {
            "access_denied" => Error::Declined,
            _ => Error::NoCode,
        });
    }

    match (code, state) {
        (Some(code), Some(state)) => Ok(Redirect { code, state }),
        _ => Err(Error::NoCode),
    }
}

/// Percent-decoding, plus `+` for a space. All a redirect ever carries.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[index + 1..index + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        decoded.push(byte);
                        index += 3;
                    }
                    None => {
                        decoded.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

async fn read_request_line(socket: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(end) = buffer.windows(2).position(|window| window == b"\r\n") {
            return Some(String::from_utf8_lossy(&buffer[..end]).into_owned());
        }

        if buffer.len() > MAX_REQUEST_LINE {
            return None;
        }
    }
}

/// Answer the browser so the user sees something other than a dead tab.
async fn respond(socket: &mut TcpStream, status: &str, message: &str) {
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Chief</title>\
         <body style=\"font-family:system-ui;padding:3rem\">{message}</body>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );

    // Best effort: the sign-in itself has already succeeded or failed.
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}
```

Then uncomment `pub mod loopback;` in `src-tauri/src/oauth/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml oauth::loopback`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/oauth/
git commit -m "feat(auth): add the loopback redirect listener"
```

---

## Task 4: Migration v3

**Files:**

- Modify: `src-tauri/src/db.rs`

- [ ] **Step 1: Write the failing test**

Replace the `integrations_hold_one_row_per_service` and `integrations_timestamp_themselves` tests in `src-tauri/src/db.rs`'s `mod tests` with these, and add the upgrade test:

```rust
    #[tokio::test]
    async fn accounts_are_unique_per_service_and_key() {
        let pool = migrated_pool().await;

        let insert = "INSERT INTO integration_accounts (service, account_key, access_token)
                      VALUES (?1, ?2, ?3)";

        sqlx::query(insert)
            .bind("github")
            .bind("octocat")
            .bind("token-1")
            .execute(&pool)
            .await
            .expect("first account should insert");

        // A second account on the same service is the whole point.
        sqlx::query(insert)
            .bind("github")
            .bind("hubot")
            .bind("token-2")
            .execute(&pool)
            .await
            .expect("a second account should insert");

        let duplicate = sqlx::query(insert)
            .bind("github")
            .bind("octocat")
            .bind("token-3")
            .execute(&pool)
            .await;

        assert!(duplicate.is_err(), "(service, account_key) should be unique");
    }

    #[tokio::test]
    async fn upgrading_from_v2_keeps_the_credential_and_the_log() {
        // A database as it stood before migration 3 shipped.
        let pool = super::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory database");

        let mut applied = super::migrations().into_iter();

        for migration in applied.by_ref().take(2) {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("the first two migrations should apply");
        }

        sqlx::query("INSERT INTO integrations (service_name, access_token, refresh_token)
                     VALUES ('github', 'gho_old', 'ghr_old')")
            .execute(&pool)
            .await
            .expect("a credential should be storable before the upgrade");
        sqlx::query("INSERT INTO work_logs (source, content, external_id)
                     VALUES ('github', 'Merged PR #4', 'owner/repo#4')")
            .execute(&pool)
            .await
            .expect("an entry should be storable before the upgrade");

        for migration in applied {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .expect("migration 3 should apply to an existing database");
        }

        // The credential survives, carried onto the new table.
        let (service, account_key, access_token, kind): (String, String, String, String) =
            sqlx::query_as(
                "SELECT service, account_key, access_token, credential_kind
                 FROM integration_accounts",
            )
            .fetch_one(&pool)
            .await
            .expect("the credential should have been carried across");

        assert_eq!(service, "github");
        assert_eq!(account_key, "github");
        assert_eq!(access_token, "gho_old");
        assert_eq!(kind, "oauth");

        // The log entry survives and is attributed to that account, so the
        // daemon does not log the same merge a second time.
        let (content, account_id): (String, Option<i64>) =
            sqlx::query_as("SELECT content, account_id FROM work_logs")
                .fetch_one(&pool)
                .await
                .expect("the existing entry should still be there");

        assert_eq!(content, "Merged PR #4");
        assert!(account_id.is_some(), "the entry should be attributed");

        // The old table is gone.
        let leftover: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'integrations'",
        )
        .fetch_optional(&pool)
        .await
        .expect("should read the schema");

        assert_eq!(leftover, None, "the v1 table should have been dropped");
    }
```

Also update `migration_creates_both_tables` to assert the new name:

```rust
        assert!(tables.contains(&"work_logs".to_string()));
        assert!(tables.contains(&"integration_accounts".to_string()));
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml db::`
Expected: FAIL — `no such table: integration_accounts`.

- [ ] **Step 3: Write the migration**

Add to `src-tauri/src/db.rs`, after the `ADD_WORK_LOG_EXTERNAL_ID` constant:

```rust
/// One row per connected *account*, rather than one per service.
///
/// `integrations` made `service_name` unique, so a person with a work and a
/// personal mailbox could connect only one. SQLite cannot drop a constraint, so
/// the table is rebuilt and its single row carried across.
///
/// Three columns exist for providers Chief has not added yet, because adding
/// them later would mean a migration for a value the provider hands over on the
/// first day: `expires_at` (Graph tokens last an hour), and `client_id` /
/// `client_secret`, which hold either a registration the user brought or one
/// minted at run time. Neither is a *shipped* secret — both belong to one
/// installation and never leave this machine.
const ADD_INTEGRATION_ACCOUNTS: &str = r"
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

INSERT INTO integration_accounts
    (service, account_key, credential_kind, access_token, refresh_token, created_at)
SELECT service_name, service_name, 'oauth', access_token, refresh_token, created_at
  FROM integrations;

DROP TABLE integrations;

ALTER TABLE work_logs ADD COLUMN account_id INTEGER;

UPDATE work_logs
   SET account_id = (SELECT id FROM integration_accounts WHERE service = work_logs.source)
 WHERE external_id IS NOT NULL;

DROP INDEX IF EXISTS idx_work_logs_external_id;

CREATE UNIQUE INDEX idx_work_logs_external_id
    ON work_logs (source, account_id, external_id)
    WHERE external_id IS NOT NULL;
";
```

The `UPDATE` reads the id back rather than assuming `1`, so it stays correct whatever the table hands out.

Then add the migration to the `migrations()` vector, after version 2:

```rust
        Migration {
            version: 3,
            description: "hold many labelled accounts per service",
            sql: ADD_INTEGRATION_ACCOUNTS,
            kind: MigrationKind::Up,
        },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml db::`
Expected: PASS. Other modules will not compile yet — that is Task 5.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "feat(db): hold many labelled accounts per service"
```

---

## Task 5: Account-aware credential storage

**Files:**

- Modify: `src-tauri/src/integrations.rs` (replace the whole file)

- [ ] **Step 1: Write the failing tests**

Replace the `mod tests` block at the bottom of `src-tauri/src/integrations.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;

    fn github_account(account_key: &str, access_token: &str) -> NewAccount<'_> {
        NewAccount {
            service: GITHUB,
            account_key,
            identity: Some("octocat@example.com"),
            credential_kind: OAUTH,
            access_token,
            refresh_token: None,
            expires_at: None,
            scopes: Some("repo read:user"),
            client_id: None,
            client_secret: None,
        }
    }

    #[tokio::test]
    async fn stores_and_reads_back_a_credential() {
        let pool = migrated_pool().await;

        let account = save(&pool, github_account("octocat", "gho_first"))
            .await
            .expect("should save");

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_first");
        assert_eq!(account.service, GITHUB);
        assert_eq!(account.identity.as_deref(), Some("octocat@example.com"));
    }

    #[tokio::test]
    async fn holds_a_work_and_a_personal_account_side_by_side() {
        let pool = migrated_pool().await;

        save(&pool, github_account("octocat", "gho_personal"))
            .await
            .expect("should save the first");
        save(&pool, github_account("hubot", "gho_work"))
            .await
            .expect("should save the second");

        let accounts = accounts(&pool, GITHUB).await.expect("should read");

        assert_eq!(accounts.len(), 2, "both accounts should be kept");
    }

    #[tokio::test]
    async fn reconnecting_the_same_account_replaces_its_credential() {
        let pool = migrated_pool().await;

        save(&pool, github_account("octocat", "gho_first"))
            .await
            .expect("should save");
        let account = save(&pool, github_account("octocat", "gho_second"))
            .await
            .expect("should save again");

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_second");
        assert_eq!(
            accounts(&pool, GITHUB).await.expect("should read").len(),
            1,
            "reconnecting should not add a second row"
        );
    }

    #[tokio::test]
    async fn reports_nothing_when_a_service_has_no_accounts() {
        let pool = migrated_pool().await;

        assert!(accounts(&pool, GITHUB).await.expect("should read").is_empty());
    }

    #[tokio::test]
    async fn timestamps_an_account_when_it_is_connected() {
        let pool = migrated_pool().await;

        let account = save(&pool, github_account("octocat", "gho_token"))
            .await
            .expect("should save");

        assert!(
            account.connected_at.contains('T') && account.connected_at.ends_with('Z'),
            "expected an ISO-8601 timestamp, got {}",
            account.connected_at
        );
    }

    #[tokio::test]
    async fn stores_renewed_tokens_without_disturbing_the_account() {
        let pool = migrated_pool().await;
        let account = save(&pool, github_account("octocat", "gho_old"))
            .await
            .expect("should save");

        store_tokens(
            &pool,
            account.id,
            "gho_new",
            Some("ghr_new"),
            Some("2026-08-22T12:00:00.000Z"),
        )
        .await
        .expect("should store renewed tokens");

        let stored = credentials(&pool, account.id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_new");
        assert_eq!(stored.refresh_token.as_deref(), Some("ghr_new"));
        assert_eq!(stored.expires_at.as_deref(), Some("2026-08-22T12:00:00.000Z"));
    }

    #[tokio::test]
    async fn names_an_account_and_backfills_its_identity() {
        let pool = migrated_pool().await;
        let account = save(
            &pool,
            NewAccount {
                identity: None,
                ..github_account("octocat", "gho_token")
            },
        )
        .await
        .expect("should save");

        set_label(&pool, account.id, Some("Work"))
            .await
            .expect("should label");
        set_identity(&pool, account.id, "octocat")
            .await
            .expect("should backfill the identity");

        let named = accounts(&pool, GITHUB)
            .await
            .expect("should read")
            .into_iter()
            .next()
            .expect("the account should be there");

        assert_eq!(named.label.as_deref(), Some("Work"));
        assert_eq!(named.identity.as_deref(), Some("octocat"));
    }

    #[tokio::test]
    async fn forgetting_removes_only_that_account() {
        let pool = migrated_pool().await;
        let personal = save(&pool, github_account("octocat", "gho_personal"))
            .await
            .expect("should save");
        save(&pool, github_account("hubot", "gho_work"))
            .await
            .expect("should save");

        forget(&pool, personal.id).await.expect("should forget");

        let left = accounts(&pool, GITHUB).await.expect("should read");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].account_key, "hubot");
        assert!(credentials(&pool, personal.id)
            .await
            .expect("should read")
            .is_none());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml integrations::`
Expected: FAIL to compile — `cannot find struct 'NewAccount'`.

- [ ] **Step 3: Write the implementation**

Replace everything above `#[cfg(test)]` in `src-tauri/src/integrations.rs` with:

```rust
//! Credentials for the accounts the user has connected.
//!
//! One row per *account*, not per service, so a work and a personal mailbox can
//! both be connected. Tokens live here on this machine and are never sent
//! anywhere except to the service they belong to.
//!
//! Every read and write of a credential goes through this module. That is the
//! seam for moving secrets to the OS keychain later: one implementation
//! changes, and no provider has to know.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::Error;

/// The services Chief knows how to connect.
pub const GITHUB: &str = "github";

/// How a credential was obtained, so routing is explicit rather than inferred
/// from which columns happen to be NULL.
pub const OAUTH: &str = "oauth";

/// A connected account, as the settings screen sees it. Carries no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub service: String,
    /// The provider's stable identifier for this account.
    pub account_key: String,
    /// What the user called it, if they named it.
    pub label: Option<String>,
    /// What to show when there is no label: a login, an address, a site.
    pub identity: Option<String>,
    /// When the credential was stored, ISO-8601.
    pub connected_at: String,
}

/// A credential to store.
#[derive(Debug, Clone)]
pub struct NewAccount<'a> {
    pub service: &'a str,
    pub account_key: &'a str,
    pub identity: Option<&'a str>,
    pub credential_kind: &'a str,
    pub access_token: &'a str,
    pub refresh_token: Option<&'a str>,
    pub expires_at: Option<&'a str>,
    pub scopes: Option<&'a str>,
    pub client_id: Option<&'a str>,
    pub client_secret: Option<&'a str>,
}

/// What is stored for a connected account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub id: i64,
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// ISO-8601, or `None` for a token that does not expire.
    pub expires_at: Option<String>,
    /// `None` when the registration is Chief's own.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

/// The columns that make up an [`Account`], so every query agrees.
const ACCOUNT_COLUMNS: &str = "id, service, account_key, label, identity, created_at";

type AccountRow = (i64, String, String, Option<String>, Option<String>, String);

fn account_from(row: AccountRow) -> Account {
    let (id, service, account_key, label, identity, connected_at) = row;

    Account {
        id,
        service,
        account_key,
        label,
        identity,
        connected_at,
    }
}

/// Store a credential, replacing any previous one for the same account.
pub async fn save(pool: &SqlitePool, account: NewAccount<'_>) -> Result<Account, Error> {
    let row = sqlx::query_as::<_, AccountRow>(&format!(
        "INSERT INTO integration_accounts
             (service, account_key, identity, credential_kind, access_token,
              refresh_token, expires_at, scopes, client_id, client_secret)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (service, account_key) DO UPDATE SET
             identity      = COALESCE(excluded.identity, integration_accounts.identity),
             credential_kind = excluded.credential_kind,
             access_token  = excluded.access_token,
             refresh_token = excluded.refresh_token,
             expires_at    = excluded.expires_at,
             scopes        = excluded.scopes,
             client_id     = excluded.client_id,
             client_secret = excluded.client_secret,
             created_at    = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         RETURNING {ACCOUNT_COLUMNS}"
    ))
    .bind(account.service)
    .bind(account.account_key)
    .bind(account.identity)
    .bind(account.credential_kind)
    .bind(account.access_token)
    .bind(account.refresh_token)
    .bind(account.expires_at)
    .bind(account.scopes)
    .bind(account.client_id)
    .bind(account.client_secret)
    .fetch_one(pool)
    .await?;

    Ok(account_from(row))
}

/// Every account connected for one service, oldest first.
pub async fn accounts(pool: &SqlitePool, service: &str) -> Result<Vec<Account>, Error> {
    let rows = sqlx::query_as::<_, AccountRow>(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM integration_accounts
          WHERE service = ?1 ORDER BY id"
    ))
    .bind(service)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(account_from).collect())
}

/// Every connected account, whatever the service.
pub async fn all_accounts(pool: &SqlitePool) -> Result<Vec<Account>, Error> {
    let rows = sqlx::query_as::<_, AccountRow>(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM integration_accounts ORDER BY service, id"
    ))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(account_from).collect())
}

/// The stored secrets for one account.
pub async fn credentials(pool: &SqlitePool, id: i64) -> Result<Option<Credentials>, Error> {
    let row = sqlx::query_as::<_, (i64, String, Option<String>, Option<String>, Option<String>, Option<String>)>(
        "SELECT id, access_token, refresh_token, expires_at, client_id, client_secret
           FROM integration_accounts WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, access_token, refresh_token, expires_at, client_id, client_secret)| Credentials {
            id,
            access_token,
            refresh_token,
            expires_at,
            client_id,
            client_secret,
        },
    ))
}

/// Replace an account's tokens after a renewal, leaving everything else alone.
pub async fn store_tokens(
    pool: &SqlitePool,
    id: i64,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "UPDATE integration_accounts
            SET access_token = ?2, refresh_token = ?3, expires_at = ?4
          WHERE id = ?1",
    )
    .bind(id)
    .bind(access_token)
    .bind(refresh_token)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Name an account, or clear the name.
pub async fn set_label(pool: &SqlitePool, id: i64, label: Option<&str>) -> Result<(), Error> {
    sqlx::query("UPDATE integration_accounts SET label = ?2 WHERE id = ?1")
        .bind(id)
        .bind(label)
        .execute(pool)
        .await?;

    Ok(())
}

/// Record who an account belongs to, once the provider has told us.
///
/// The row migrated from v1 has no identity, because the old table never stored
/// one. It is filled in on the next successful read rather than by asking the
/// user to reconnect.
pub async fn set_identity(pool: &SqlitePool, id: i64, identity: &str) -> Result<(), Error> {
    sqlx::query("UPDATE integration_accounts SET identity = ?2 WHERE id = ?1")
        .bind(id)
        .bind(identity)
        .execute(pool)
        .await?;

    Ok(())
}

/// Forget one account's credential.
pub async fn forget(pool: &SqlitePool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM integration_accounts WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml integrations::`
Expected: PASS, 8 tests. Other modules still will not compile — Tasks 6 to 9 fix them.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/integrations.rs
git commit -m "feat(db): store one credential per connected account"
```

---

## Task 6: The `Provider` trait

**Files:**

- Modify: `src-tauri/src/oauth/mod.rs`
- Modify: `src-tauri/src/github.rs`

- [ ] **Step 1: Write the failing test**

Add to `src-tauri/src/github.rs`'s `mod tests`:

```rust
    #[test]
    fn describes_itself_as_a_provider() {
        use crate::oauth::Provider;

        let client = Client::against("http://127.0.0.1:1").expect("should build a client");

        assert_eq!(Client::SERVICE, crate::integrations::GITHUB);
        assert_eq!(client.scopes(), &["repo", "read:user"]);
        assert!(
            client.client_secret().is_none(),
            "the device flow has no secret and a shipped one would not be secret"
        );
        assert!(
            client.endpoints().device_code.is_some(),
            "GitHub signs in by device code"
        );
    }

    #[tokio::test]
    async fn reports_who_the_token_belongs_to() {
        let (host, server) = serve(vec![(
            "HTTP/1.1 200 OK",
            r#"{"login":"octocat","name":"The Octocat"}"#,
        )]);
        let client = Client::against(&host).expect("should build a client");

        let viewer = client.viewer("gho_token").await.expect("should read");

        assert_eq!(viewer, "octocat");

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = crate::llama::test_support::split(&requests[0]);
        assert!(request_line.contains("/user"), "got {request_line}");
    }
```

Check the top of `github.rs`'s `mod tests` imports `serve`; if not, add
`use crate::llama::test_support::serve;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml github::`
Expected: FAIL to compile — `no trait named 'Provider'`.

- [ ] **Step 3: Write the trait**

Append to `src-tauri/src/oauth/mod.rs`:

```rust
/// A fresh pair of tokens, however they were obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds from now, when the provider says so.
    pub expires_in: Option<u64>,
}

/// Where a provider's OAuth endpoints live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: &'static str,
    pub token: &'static str,
    /// `Some` when the provider supports the device grant.
    pub device_code: Option<&'static str>,
}

/// What a service has to say about itself for the shared machinery to sign in
/// on its behalf and keep the credential alive.
///
/// Deliberately small. Everything here is a fact about the provider; nothing is
/// a flow. Flows live in the provider's own module until a second provider
/// wants the same one, because an abstraction drawn from one example is a
/// guess.
pub trait Provider {
    /// The value stored in `integration_accounts.service`.
    const SERVICE: &'static str;

    /// This provider's error type, so callers keep the errors they already
    /// surface to the user rather than a lowest common denominator.
    type Error: std::error::Error + From<crate::db::Error>;

    fn endpoints(&self) -> Endpoints;

    /// Public by design. Never a secret; see `client_secret`.
    fn client_id(&self) -> Result<String, Self::Error>;

    fn scopes(&self) -> &'static [&'static str];

    /// Only ever a value the provider documents as non-confidential, or one
    /// minted for this installation alone. A secret compiled into a binary
    /// Chief distributes is not a secret, and must never be returned here.
    fn client_secret(&self) -> Option<String> {
        None
    }

    /// Swap a refresh token for a fresh pair.
    fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> impl std::future::Future<Output = Result<Tokens, Self::Error>> + Send;

    /// Whether an error means the stored credential is no longer usable.
    fn is_token_rejected(error: &Self::Error) -> bool;
}
```

- [ ] **Step 4: Implement it for GitHub**

In `src-tauri/src/github.rs`, add `viewer` as a method on `impl Client`, next to `pull_requests`:

```rust
    /// Who the stored token belongs to, so an account can name itself.
    pub async fn viewer(&self, token: &str) -> Result<String, Error> {
        let response = self
            .http
            .get(format!("{}/user", self.api_host))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        let body: Value = self.read(response).await?;

        body["login"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| Error::Decode("the user response had no login".to_string()))
    }
```

Then add the trait implementation at the end of the file, above `mod tests`:

```rust
/// The scopes Chief asks GitHub for, as the shared machinery wants them.
const SCOPE_LIST: &[&str] = &["repo", "read:user"];

impl crate::oauth::Provider for Client {
    const SERVICE: &'static str = crate::integrations::GITHUB;

    type Error = Error;

    fn endpoints(&self) -> crate::oauth::Endpoints {
        crate::oauth::Endpoints {
            authorize: "https://github.com/login/oauth/authorize",
            token: "https://github.com/login/oauth/access_token",
            device_code: Some("https://github.com/login/device/code"),
        }
    }

    fn client_id(&self) -> Result<String, Error> {
        client_id()
    }

    fn scopes(&self) -> &'static [&'static str] {
        SCOPE_LIST
    }

    async fn refresh(&self, client_id: &str, refresh_token: &str) -> Result<crate::oauth::Tokens, Error> {
        let (access_token, refresh_token) = Client::refresh(self, client_id, refresh_token).await?;

        Ok(crate::oauth::Tokens {
            access_token,
            refresh_token,
            // GitHub's device-flow refresh does not say, and the reactive
            // renewal on a rejected token covers it.
            expires_in: None,
        })
    }

    fn is_token_rejected(error: &Error) -> bool {
        matches!(error, Error::TokenRejected)
    }
}
```

The existing `SCOPES` constant stays: it is the space-separated form the device flow posts. Change its definition to be derived so the two cannot drift — replace `const SCOPES: &str = "repo read:user";` with nothing, and at its single use site in `start_login` replace `("scope", SCOPES)` with `("scope", SCOPE_LIST.join(" ").as_str())`. If the borrow checker objects to the temporary, bind it first:

```rust
        let scope = SCOPE_LIST.join(" ");
        let response: DeviceCodeResponse = self
            .post_form(
                &format!("{}/login/device/code", self.auth_host),
                &[("client_id", client_id), ("scope", &scope)],
            )
            .await?;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml github::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/oauth/mod.rs src-tauri/src/github.rs
git commit -m "feat(auth): describe a provider once, for the shared machinery"
```

---

## Task 7: A session per account

**Files:**

- Modify: `src-tauri/src/session.rs` (replace the whole file)

- [ ] **Step 1: Write the failing tests**

Replace `mod tests` in `src-tauri/src/session.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::integrations::{NewAccount, OAUTH};
    use crate::llama::test_support::serve;

    const SEARCH_RESULTS: &str = r#"{
        "total_count": 1,
        "items": [{
            "number": 12,
            "title": "Add the daemon",
            "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
            "state": "open",
            "draft": false,
            "html_url": "https://github.com/scottmallinson/chief.ai/pull/12",
            "updated_at": "2026-08-19T14:00:00Z"
        }]
    }"#;

    const EXPIRED: &str = r#"{"message":"Bad credentials"}"#;
    const RENEWED: &str =
        r#"{"access_token":"gho_new","refresh_token":"ghr_new","token_type":"bearer"}"#;

    /// A pool with one connected GitHub account, and that account's id.
    async fn connected(refresh_token: Option<&str>) -> (SqlitePool, i64) {
        let pool = migrated_pool().await;
        let account = integrations::save(
            &pool,
            NewAccount {
                service: integrations::GITHUB,
                account_key: "octocat",
                identity: Some("octocat"),
                credential_kind: OAUTH,
                access_token: "gho_old",
                refresh_token,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store credentials");

        (pool, account.id)
    }

    #[tokio::test]
    async fn reads_without_renewing_when_the_token_still_works() {
        let (host, server) = serve(vec![("HTTP/1.1 200 OK", SEARCH_RESULTS)]);
        let (pool, account_id) = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        let prs = GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the stub should answer");

        assert_eq!(prs.len(), 1);
        assert_eq!(
            server.await.expect("the stub should finish").len(),
            1,
            "no renewal should have been attempted"
        );
    }

    #[tokio::test]
    async fn renews_an_expired_token_and_retries() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", RENEWED),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let (pool, account_id) = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        let prs = GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the read should succeed after renewing");

        assert_eq!(prs.len(), 1);

        let requests = server.await.expect("the stub should finish");
        assert_eq!(requests.len(), 3, "expected read, renew, read");

        let (_, renewal) = crate::llama::test_support::split(&requests[1]);
        assert!(renewal.contains("grant_type=refresh_token"), "got {renewal}");
        assert!(
            !renewal.contains("client_secret"),
            "a device-flow refresh needs no secret: {renewal}"
        );
        assert!(
            requests[2]
                .to_lowercase()
                .contains("authorization: bearer gho_new"),
            "the retry should use the renewed token"
        );
    }

    #[tokio::test]
    async fn stores_the_renewed_credentials_against_that_account() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 401 Unauthorized", EXPIRED),
            ("HTTP/1.1 200 OK", RENEWED),
            ("HTTP/1.1 200 OK", SEARCH_RESULTS),
        ]);
        let (pool, account_id) = connected(Some("ghr_old")).await;
        let client = Client::against(&host).expect("should build a client");

        GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect("the read should succeed after renewing");

        let stored = integrations::credentials(&pool, account_id)
            .await
            .expect("should read")
            .expect("should be connected");

        assert_eq!(stored.access_token, "gho_new");
        assert_eq!(stored.refresh_token.as_deref(), Some("ghr_new"));

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn asks_the_user_to_reconnect_when_there_is_nothing_to_renew_with() {
        let (host, server) = serve(vec![("HTTP/1.1 401 Unauthorized", EXPIRED)]);
        let (pool, account_id) = connected(None).await;
        let client = Client::against(&host).expect("should build a client");

        let error = GithubSession::with_client_id(&pool, &client, account_id, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("without a refresh token there is no way back");

        assert!(matches!(error, github::Error::TokenRejected), "got {error:?}");
        assert_eq!(server.await.expect("the stub should finish").len(), 1);
    }

    #[tokio::test]
    async fn says_when_the_account_is_not_connected() {
        let pool = migrated_pool().await;
        let client = Client::against("http://127.0.0.1:1").expect("should build a client");

        let error = GithubSession::with_client_id(&pool, &client, 404, "Iv1.clientid")
            .pull_requests(State::Open, 25)
            .await
            .expect_err("there is nothing to read with");

        assert!(matches!(error, github::Error::NotConnected), "got {error:?}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml session::`
Expected: FAIL to compile — `cannot find type 'GithubSession'`.

- [ ] **Step 3: Write the implementation**

Replace everything above `#[cfg(test)]` in `src-tauri/src/session.rs` with:

```rust
//! Reading from a connected account with the credentials Chief holds for it.
//!
//! Access tokens expire. Rather than making every caller think about that,
//! [`Session`] renews a rejected credential and the caller retries once — so a
//! connection made weeks ago keeps working without the user reconnecting.
//!
//! [`Session`] is generic over the provider because storing, renewing and
//! re-storing a credential is the same work whoever issued it. What is *not*
//! generic is the reading, so each provider keeps a thin wrapper of its own —
//! [`GithubSession`] here. One example is not enough to draw a general reading
//! abstraction from, and guessing at one would cost more than it saves.

use sqlx::SqlitePool;

use crate::github::{self, Client, PullRequest, State};
use crate::integrations;
use crate::oauth::Provider;

/// A conversation with one connected account.
pub struct Session<'a, P: Provider> {
    pool: &'a SqlitePool,
    provider: &'a P,
    account_id: i64,
    /// Needed only to renew. Resolved up front because a build without one
    /// simply cannot refresh — which is no reason to fail a plain read.
    client_id: Option<String>,
}

impl<'a, P: Provider> Session<'a, P> {
    pub fn new(pool: &'a SqlitePool, provider: &'a P, account_id: i64) -> Self {
        Self {
            pool,
            provider,
            account_id,
            client_id: provider.client_id().ok(),
        }
    }

    /// A session that renews with a known client id, for tests.
    #[cfg(test)]
    pub fn with_client_id(
        pool: &'a SqlitePool,
        provider: &'a P,
        account_id: i64,
        client_id: &str,
    ) -> Self {
        Self {
            pool,
            provider,
            account_id,
            client_id: Some(client_id.to_string()),
        }
    }

    /// The stored access token, or an error saying to reconnect.
    pub async fn token(&self) -> Result<String, P::Error> {
        Ok(self.credentials().await?.access_token)
    }

    async fn credentials(&self) -> Result<integrations::Credentials, P::Error> {
        integrations::credentials(self.pool, self.account_id)
            .await
            .map_err(P::Error::from)?
            .ok_or_else(|| P::Error::from(crate::db::Error::NotLoaded))
    }

    /// Swap an expired credential for a fresh one and store it.
    ///
    /// Without a refresh token or a client id there is nothing to try, and the
    /// caller reports that the user should reconnect rather than failing
    /// silently.
    pub async fn renew(&self) -> Result<Option<String>, P::Error> {
        let credentials = self.credentials().await?;

        let (Some(refresh_token), Some(client_id)) = (&credentials.refresh_token, &self.client_id)
        else {
            return Ok(None);
        };

        let tokens = self.provider.refresh(client_id, refresh_token).await?;

        integrations::store_tokens(
            self.pool,
            self.account_id,
            &tokens.access_token,
            tokens.refresh_token.as_deref(),
            None,
        )
        .await
        .map_err(P::Error::from)?;

        Ok(Some(tokens.access_token))
    }

    /// Record who this account belongs to, the first time we find out.
    pub async fn name_once(&self, identity: &str) -> Result<(), P::Error> {
        integrations::set_identity(self.pool, self.account_id, identity)
            .await
            .map_err(P::Error::from)
    }
}

/// Reading GitHub on behalf of one connected account.
pub struct GithubSession<'a> {
    session: Session<'a, Client>,
    client: &'a Client,
}

impl<'a> GithubSession<'a> {
    pub fn new(pool: &'a SqlitePool, client: &'a Client, account_id: i64) -> Self {
        Self {
            session: Session::new(pool, client, account_id),
            client,
        }
    }

    #[cfg(test)]
    pub fn with_client_id(
        pool: &'a SqlitePool,
        client: &'a Client,
        account_id: i64,
        client_id: &str,
    ) -> Self {
        Self {
            session: Session::with_client_id(pool, client, account_id, client_id),
            client,
        }
    }

    /// The user's pull requests, renewing the token if GitHub says it expired.
    pub async fn pull_requests(
        &self,
        state: State,
        limit: u8,
    ) -> Result<Vec<PullRequest>, github::Error> {
        let token = self
            .session
            .token()
            .await
            .map_err(|_| github::Error::NotConnected)?;

        match self.client.pull_requests(&token, state, limit).await {
            Err(github::Error::TokenRejected) => {
                let renewed = self
                    .session
                    .renew()
                    .await?
                    .ok_or(github::Error::TokenRejected)?;

                self.client.pull_requests(&renewed, state, limit).await
            }
            other => other,
        }
    }

    /// Who this account belongs to, recorded so the settings screen can say.
    pub async fn name_account(&self) -> Result<String, github::Error> {
        let token = self
            .session
            .token()
            .await
            .map_err(|_| github::Error::NotConnected)?;
        let login = self.client.viewer(&token).await?;
        self.session.name_once(&login).await?;

        Ok(login)
    }
}
```

Note `Session::credentials` maps a missing row onto `db::Error::NotLoaded`, which `github::Error` already converts through its `Storage` variant — but the caller in `pull_requests` turns any credential failure into `NotConnected`, which is the message the user needs.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml session::`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/session.rs
git commit -m "feat(auth): renew a credential once per account, not per service"
```

---

## Task 8: Service-parameterised commands

**Files:**

- Modify: `src-tauri/src/connect.rs` (replace the whole file)
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write the implementation**

There is no unit test here: these are thin Tauri wrappers over code already covered, and testing them would need a live `AppHandle`. The frontend tests in Tasks 10 and 11 cover the contract from the other side. Replace `src-tauri/src/connect.rs` entirely with:

```rust
//! Connecting and disconnecting the accounts Chief reads from.
//!
//! Sign-in happens in two commands so the user can see their code while we
//! wait: `start_login` returns what to enter, `finish_login` blocks until they
//! have entered it. The device code itself never reaches the renderer — it
//! stays in backend state.
//!
//! Every command takes the service by name rather than being named after one,
//! so a new provider costs an arm in `dispatch` and no new commands.

use tauri::{AppHandle, Runtime, State};
use tokio::sync::Mutex;

use crate::db;
use crate::github::{self, Client, DeviceLogin, PendingLogin};
use crate::integrations::{self, Account, NewAccount, OAUTH};
use crate::oauth::Provider;

/// The sign-in waiting to be completed, if any.
#[derive(Default)]
pub struct Pending(Mutex<Option<PendingLogin>>);

/// Reject a service Chief does not know, rather than failing later and less
/// clearly.
fn known(service: &str) -> Result<(), github::Error> {
    if service == Client::SERVICE {
        return Ok(());
    }

    Err(github::Error::Status {
        status: 400,
        body: format!("there is no integration called '{service}'"),
    })
}

/// Begin signing in and return what the user must enter.
#[tauri::command]
pub async fn start_login(
    service: String,
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<DeviceLogin, github::Error> {
    known(&service)?;

    let client_id = github::client_id()?;
    let started = client.start_login(&client_id).await?;
    let login = started.login.clone();

    *pending.0.lock().await = Some(started);

    Ok(login)
}

/// Wait for the user to finish in the browser, then store the credential.
#[tauri::command]
pub async fn finish_login<R: Runtime>(
    service: String,
    app: AppHandle<R>,
    client: State<'_, Client>,
    pending: State<'_, Pending>,
) -> Result<Vec<Account>, github::Error> {
    known(&service)?;

    let started = pending
        .0
        .lock()
        .await
        .take()
        .ok_or(github::Error::NotConnected)?;

    let client_id = github::client_id()?;
    let (access_token, refresh_token) = client.finish_login(&client_id, &started).await?;

    // Ask who this is before storing, so two accounts on one service do not
    // collide on a placeholder key.
    let login = client.viewer(&access_token).await?;

    let pool = db::pool(&app).await?;
    integrations::save(
        &pool,
        NewAccount {
            service: Client::SERVICE,
            account_key: &login,
            identity: Some(&login),
            credential_kind: OAUTH,
            access_token: &access_token,
            refresh_token: refresh_token.as_deref(),
            expires_at: None,
            scopes: Some(&client.scopes().join(" ")),
            client_id: None,
            client_secret: None,
        },
    )
    .await?;

    Ok(integrations::all_accounts(&pool).await?)
}

/// Every connected account, whatever the service.
#[tauri::command]
pub async fn connections<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Account>, github::Error> {
    let pool = db::pool(&app).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

/// Forget one account's credential.
#[tauri::command]
pub async fn disconnect<R: Runtime>(
    account_id: i64,
    app: AppHandle<R>,
) -> Result<Vec<Account>, github::Error> {
    let pool = db::pool(&app).await?;
    integrations::forget(&pool, account_id).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

/// Name an account, or clear its name.
#[tauri::command]
pub async fn label_account<R: Runtime>(
    account_id: i64,
    label: Option<String>,
    app: AppHandle<R>,
) -> Result<Vec<Account>, github::Error> {
    let pool = db::pool(&app).await?;
    integrations::set_label(&pool, account_id, label.as_deref()).await?;

    Ok(integrations::all_accounts(&pool).await?)
}

```

The account migrated from v1 has no identity, because the old table never stored one. It is filled
in by the daemon on its next successful pass (Task 9) rather than by a command the user has to
trigger, so there is no command for it here.

- [ ] **Step 2: Register the commands**

In `src-tauri/src/lib.rs`, replace the four `connect::*_github*` entries in `tauri::generate_handler!` with:

```rust
            connect::start_login,
            connect::finish_login,
            connect::connections,
            connect::disconnect,
            connect::label_account,
```

- [ ] **Step 3: Verify the backend compiles and every Rust test passes**

Run: `pnpm rust:test`
Expected: PASS. `tools.rs` and `daemon.rs` will fail to compile until Task 9 — if so, do Task 9 and re-run this step before committing both together.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/connect.rs src-tauri/src/lib.rs
git commit -m "feat(integrations): take the service by name rather than per command"
```

---

## Task 9: Attribute the work log to an account

**Files:**

- Modify: `src-tauri/src/work_log.rs`
- Modify: `src-tauri/src/daemon.rs`
- Modify: `src-tauri/src/tools.rs`

- [ ] **Step 1: Write the failing test**

Add to `src-tauri/src/work_log.rs`'s `mod tests`:

```rust
    #[tokio::test]
    async fn two_accounts_may_log_the_same_identifier() {
        let pool = migrated_pool().await;

        let entry = |account_id| NewWorkLogEntry {
            source: "github".to_string(),
            content: "Merged PR #7".to_string(),
            timestamp: None,
            summary: None,
            external_id: Some("owner/repo#7".to_string()),
            account_id: Some(account_id),
        };

        assert!(
            insert_new(&pool, entry(1)).await.expect("should insert").is_some(),
            "the first account should log it"
        );
        assert!(
            insert_new(&pool, entry(2)).await.expect("should insert").is_some(),
            "a different account is different work"
        );
        assert!(
            insert_new(&pool, entry(1)).await.expect("should insert").is_none(),
            "the same account should not log it twice"
        );

        assert!(has_logged(&pool, "github", 1, "owner/repo#7")
            .await
            .expect("should read"));
        assert!(!has_logged(&pool, "github", 3, "owner/repo#7")
            .await
            .expect("should read"));
    }
```

Every existing `NewWorkLogEntry { .. }` literal in the test module needs `account_id: None` adding.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml work_log::`
Expected: FAIL to compile — `struct 'NewWorkLogEntry' has no field named 'account_id'`.

- [ ] **Step 3: Thread `account_id` through `work_log.rs`**

Add the field to `NewWorkLogEntry`, after `external_id`:

```rust
    /// Which connected account the activity came from. `None` for an entry the
    /// user wrote by hand.
    #[serde(default)]
    pub account_id: Option<i64>,
}
```

In `insert`, add the column, the placeholder and the bind:

```rust
        "INSERT INTO work_logs (timestamp, source, content, summary, external_id, account_id)
         VALUES (COALESCE(?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), ?2, ?3, ?4, ?5, ?6)
         RETURNING id, timestamp, source, content, summary, external_id",
```

...and `.bind(entry.account_id)` after `.bind(entry.external_id)`.

In `insert_new`, the same two changes, plus the conflict target must name the new index's columns:

```rust
        "INSERT INTO work_logs (timestamp, source, content, summary, external_id, account_id)
         VALUES (COALESCE(?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (source, account_id, external_id) WHERE external_id IS NOT NULL DO NOTHING
         RETURNING id, timestamp, source, content, summary, external_id",
```

Replace `has_logged` entirely:

```rust
/// Whether this account has already logged that thing.
pub async fn has_logged(
    pool: &SqlitePool,
    source: &str,
    account_id: i64,
    external_id: &str,
) -> Result<bool, Error> {
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM work_logs
          WHERE source = ?1 AND account_id = ?2 AND external_id = ?3
          LIMIT 1",
    )
    .bind(source)
    .bind(account_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;

    Ok(existing.is_some())
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml work_log::`
Expected: PASS.

- [ ] **Step 5: Make the daemon walk every connected account**

In `src-tauri/src/daemon.rs`, replace `run_once` with:

```rust
/// Run one pass over every connected account. Returns how many new entries
/// were written.
///
/// Does nothing at all when nothing is connected — there is no work to read.
pub async fn run_once(context: &Context) -> Result<usize, Error> {
    let accounts = integrations::accounts(&context.pool, integrations::GITHUB).await?;
    let mut written = 0;

    for account in accounts {
        // The user's question is worth more than the log being current, and the
        // engine decodes one request at a time.
        if context.attention.is_engaged() {
            break;
        }

        written += run_one_account(context, &account).await?;
    }

    Ok(written)
}

async fn run_one_account(
    context: &Context,
    account: &integrations::Account,
) -> Result<usize, Error> {
    let session = GithubSession::new(&context.pool, &context.github, account.id);

    // An account carried over from before Chief stored identities has none.
    // Fill it in here rather than making the user reconnect for a label.
    if account.identity.is_none() {
        session.name_account().await?;
    }

    let merged = session.pull_requests(State::Merged, BATCH).await?;

    let mut written = 0;

    for pull_request in merged {
        if context.attention.is_engaged() {
            break;
        }

        if log_one(context, account.id, &pull_request).await? {
            written += 1;
        }
    }

    Ok(written)
}
```

Change `log_one`'s signature and its two uses of the account:

```rust
async fn log_one(
    context: &Context,
    account_id: i64,
    pull_request: &PullRequest,
) -> Result<bool, Error> {
    let external_id = pull_request.external_id();

    if work_log::has_logged(&context.pool, "github", account_id, &external_id).await? {
        return Ok(false);
    }
```

...and in the `NewWorkLogEntry` literal at the end of `log_one`, add:

```rust
        external_id: Some(external_id),
        account_id: Some(account_id),
    };
```

Update the imports at the top of `daemon.rs`: replace `use crate::session::Session;` with `use crate::session::GithubSession;`.

Existing daemon tests construct a `Context` and call `run_once`; each will need a connected account in its pool. Where a test currently calls `integrations::save(&pool, integrations::GITHUB, "gho_token", None)`, replace it with the `NewAccount` form shown in Task 7's `connected` helper, and where a test asserts on `has_logged`, pass the account id it created.

- [ ] **Step 6: Make the tool read the first connected account**

In `src-tauri/src/tools.rs`, replace the body of `fetch_github_prs`:

```rust
/// Read the user's pull requests from GitHub with their own token.
///
/// Reads the first connected account. Asking across several accounts is a
/// question the tool schema cannot yet express, and inventing an argument the
/// model would have to guess at would make answers worse, not better.
async fn fetch_github_prs(
    context: &Context,
    state: PullRequestState,
) -> Result<Value, github::Error> {
    let account = crate::integrations::accounts(&context.pool, crate::integrations::GITHUB)
        .await?
        .into_iter()
        .next()
        .ok_or(github::Error::NotConnected)?;

    let pull_requests = GithubSession::new(&context.pool, &context.github, account.id)
        .pull_requests(state.into(), PR_LIMIT)
        .await?;

    Ok(github::as_tool_result(&pull_requests))
}
```

Change the import at the top from `use crate::session::Session;` to `use crate::session::GithubSession;`. In `mod tests`, the `connected` helper must store a `NewAccount` the same way Task 7's does.

- [ ] **Step 7: Run every Rust test**

Run: `pnpm rust:test`
Expected: PASS across all modules.

Run: `pnpm rust:lint`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/work_log.rs src-tauri/src/daemon.rs src-tauri/src/tools.rs
git commit -m "feat(daemon): log work per account rather than per service"
```

---

## Task 10: The frontend contract

**Files:**

- Modify: `src/lib/integrations.ts` (replace the whole file)
- Create: `src/hooks/use-integrations.ts`
- Delete: `src/hooks/use-github.ts`

- [ ] **Step 1: Write the new contract**

Replace `src/lib/integrations.ts` entirely:

```ts
import { invoke } from '@tauri-apps/api/core';

/** The services Chief knows how to connect. */
export const GITHUB = 'github';

/** One connected account, as the backend reports it. Carries no secret. */
export interface Account {
  id: number;
  service: string;
  /** The provider's stable identifier for this account. */
  accountKey: string;
  /** What the user called it, if they named it. */
  label: string | null;
  /** What to show when there is no label: a login, an address, a site. */
  identity: string | null;
  /** ISO-8601. */
  connectedAt: string;
}

/** What the user must do in the browser to finish signing in. */
export interface DeviceLogin {
  /** The code they type into the provider. */
  userCode: string;
  /** Where they type it. */
  verificationUri: string;
  /** Seconds until the code stops working. */
  expiresIn: number;
}

/** Every connected account, whatever the service. */
export function connections(): Promise<Account[]> {
  return invoke<Account[]>('connections');
}

/** Begin signing in and get the code the user must enter. */
export function startLogin(service: string): Promise<DeviceLogin> {
  return invoke<DeviceLogin>('start_login', { service });
}

/** Wait for the user to finish in the browser, then store the credential. */
export function finishLogin(service: string): Promise<Account[]> {
  return invoke<Account[]>('finish_login', { service });
}

/** Forget one account's credential. */
export function disconnect(accountId: number): Promise<Account[]> {
  return invoke<Account[]>('disconnect', { accountId });
}

/** Name an account, or clear its name. */
export function labelAccount(accountId: number, label: string | null): Promise<Account[]> {
  return invoke<Account[]>('label_account', { accountId, label });
}

/** What to call an account on screen. */
export function accountName(account: Account): string {
  return account.label ?? account.identity ?? account.accountKey;
}
```

- [ ] **Step 2: Write the failing hook test**

Create `src/hooks/use-integrations.test.ts`:

```ts
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useIntegrations } from '@/hooks/use-integrations';

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl }));

const octocat = {
  id: 1,
  service: 'github',
  accountKey: 'octocat',
  label: null,
  identity: 'octocat',
  connectedAt: '2026-08-19T14:00:00.000Z',
};

describe('useIntegrations', () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
  });

  it('reports the accounts already connected', async () => {
    invoke.mockResolvedValue([octocat]);

    const { result } = renderHook(() => useIntegrations());

    await waitFor(() => expect(result.current.status).toBe('idle'));
    expect(result.current.accountsFor('github')).toEqual([octocat]);
  });

  it('shows the code while it waits for the browser', async () => {
    const login = { userCode: 'ABCD-1234', verificationUri: 'https://github.com/login/device' };

    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      if (command === 'start_login') return Promise.resolve(login);
      // Never settles, so the waiting state is observable.
      return new Promise(() => {});
    });

    const { result } = renderHook(() => useIntegrations());
    await waitFor(() => expect(result.current.status).toBe('idle'));

    act(() => result.current.connect('github'));

    await waitFor(() => expect(result.current.status).toBe('awaiting-user'));
    expect(result.current.login).toEqual(login);
    expect(openUrl).toHaveBeenCalledWith('https://github.com/login/device');
  });

  it('surfaces a refusal and stops waiting', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([]);
      return Promise.reject(new Error('sign-in was declined on GitHub'));
    });

    const { result } = renderHook(() => useIntegrations());
    await waitFor(() => expect(result.current.status).toBe('idle'));

    act(() => result.current.connect('github'));

    await waitFor(() => expect(result.current.error).toBe('sign-in was declined on GitHub'));
    expect(result.current.login).toBeNull();
    expect(result.current.status).toBe('idle');
  });

  it('forgets one account without touching the others', async () => {
    invoke.mockImplementation((command: string) => {
      if (command === 'connections') return Promise.resolve([octocat]);
      if (command === 'disconnect') return Promise.resolve([]);
      return Promise.resolve([]);
    });

    const { result } = renderHook(() => useIntegrations());
    await waitFor(() => expect(result.current.accountsFor('github')).toHaveLength(1));

    act(() => result.current.disconnect(1));

    await waitFor(() => expect(result.current.accountsFor('github')).toHaveLength(0));
    expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 1 });
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `pnpm test -- use-integrations`
Expected: FAIL — cannot resolve `@/hooks/use-integrations`.

- [ ] **Step 4: Write the hook**

Create `src/hooks/use-integrations.ts`:

```ts
import { useCallback, useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';

import {
  connections,
  disconnect as forget,
  finishLogin,
  labelAccount,
  startLogin,
  type Account,
  type DeviceLogin,
} from '@/lib/integrations';

type Status = 'loading' | 'idle' | 'awaiting-user' | 'working';

interface UseIntegrations {
  accounts: Account[];
  accountsFor: (service: string) => Account[];
  login: DeviceLogin | null;
  /** Which service is being connected, so only that card shows the code. */
  connecting: string | null;
  status: Status;
  error: string | null;
  connect: (service: string) => void;
  disconnect: (accountId: number) => void;
  rename: (accountId: number, label: string | null) => void;
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/** Drive sign-in and account management from the settings screen. */
export function useIntegrations(): UseIntegrations {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [login, setLogin] = useState<DeviceLogin | null>(null);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    connections()
      .then((current) => {
        if (cancelled) return;
        setAccounts(current);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setError(describe(cause));
        setStatus('idle');
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const connect = useCallback((service: string) => {
    setError(null);
    setConnecting(service);
    setStatus('working');

    startLogin(service)
      .then(async (started) => {
        setLogin(started);
        setStatus('awaiting-user');

        // Best effort: the code is on screen either way.
        await openUrl(started.verificationUri).catch(() => undefined);

        const current = await finishLogin(service);
        setAccounts(current);
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setLogin(null);
        setConnecting(null);
        setStatus('idle');
      });
  }, []);

  const settle = useCallback((work: Promise<Account[]>) => {
    setError(null);
    setStatus('working');

    work
      .then((current) => {
        setAccounts(current);
        setStatus('idle');
      })
      .catch((cause: unknown) => {
        setError(describe(cause));
        setStatus('idle');
      });
  }, []);

  const disconnect = useCallback((accountId: number) => settle(forget(accountId)), [settle]);

  const rename = useCallback(
    (accountId: number, label: string | null) => settle(labelAccount(accountId, label)),
    [settle],
  );

  const accountsFor = useCallback(
    (service: string) => accounts.filter((account) => account.service === service),
    [accounts],
  );

  return {
    accounts,
    accountsFor,
    login,
    connecting,
    status,
    error,
    connect,
    disconnect,
    rename,
  };
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `pnpm test -- use-integrations`
Expected: PASS, 4 tests.

- [ ] **Step 6: Delete the superseded hook**

```bash
git rm src/hooks/use-github.ts
```

`SettingsView.tsx` still imports it and will not typecheck until Task 11. That is expected; do not fix it here.

- [ ] **Step 7: Commit**

```bash
git add src/lib/integrations.ts src/hooks/use-integrations.ts src/hooks/use-integrations.test.ts
git commit -m "feat(ui): drive sign-in per account rather than per service"
```

---

## Task 11: Accounts on the settings screen

**Files:**

- Modify: `src/components/views/SettingsView.tsx`
- Modify: `src/components/views/SettingsView.test.tsx`

- [ ] **Step 1: Write the failing tests**

Replace the `disconnected` / `connected` fixtures at the top of `src/components/views/SettingsView.test.tsx` with:

```tsx
const octocat = {
  id: 1,
  service: 'github',
  accountKey: 'octocat',
  label: null,
  identity: 'octocat',
  connectedAt: '2026-08-19T14:00:00.000Z',
};

const hubot = {
  id: 2,
  service: 'github',
  accountKey: 'hubot',
  label: 'Work',
  identity: 'hubot',
  connectedAt: '2026-08-20T09:00:00.000Z',
};
```

Existing tests that did `invoke.mockResolvedValue(disconnected)` become `invoke.mockResolvedValue([])`, and those that used `connected` become `invoke.mockResolvedValue([octocat])`. Then add:

```tsx
it('lists every connected account for a service', async () => {
  invoke.mockResolvedValue([octocat, hubot]);

  render(<SettingsView />);

  expect(await screen.findByLabelText('Name for octocat')).toBeInTheDocument();
  expect(screen.getByLabelText('Name for hubot')).toBeInTheDocument();
});

it('prefers the name the user gave an account', async () => {
  invoke.mockResolvedValue([hubot]);

  render(<SettingsView />);

  // The chosen name is the field's value; the raw identity is only its
  // placeholder, so the disconnect button is where the name has to surface.
  expect(await screen.findByRole('button', { name: 'Disconnect Work' })).toBeInTheDocument();
  expect(screen.getByLabelText('Name for hubot')).toHaveValue('Work');
});

it('lets an account be named', async () => {
  invoke.mockResolvedValue([octocat, hubot]);

  render(<SettingsView />);

  expect(await screen.findByLabelText('Name for hubot')).toHaveValue('Work');

  await userEvent.type(screen.getByLabelText('Name for octocat'), 'Personal');
  await userEvent.tab();

  expect(invoke).toHaveBeenCalledWith('label_account', { accountId: 1, label: 'Personal' });
});

it('clears a name when the field is emptied', async () => {
  invoke.mockResolvedValue([hubot]);

  render(<SettingsView />);

  await userEvent.clear(await screen.findByLabelText('Name for hubot'));
  await userEvent.tab();

  expect(invoke).toHaveBeenCalledWith('label_account', { accountId: 2, label: null });
});

it('offers to add another account when one is already connected', async () => {
  invoke.mockResolvedValue([octocat]);

  render(<SettingsView />);

  expect(
    await screen.findByRole('button', { name: 'Add another GitHub account' }),
  ).toBeInTheDocument();
});

it('disconnects the account whose button was pressed', async () => {
  invoke.mockImplementation((command: string) => {
    if (command === 'connections') return Promise.resolve([octocat, hubot]);
    return Promise.resolve([octocat]);
  });

  render(<SettingsView />);

  const buttons = await screen.findAllByRole('button', { name: /^Disconnect/ });
  await userEvent.click(buttons[1]);

  expect(invoke).toHaveBeenCalledWith('disconnect', { accountId: 2 });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `pnpm test -- SettingsView`
Expected: FAIL — `use-github` cannot be resolved.

- [ ] **Step 3: Rewrite the integration section**

In `src/components/views/SettingsView.tsx`, replace the `connectionState` helper and the whole `GithubIntegration` component with the following, and change the import of `useGithub` to `import { useIntegrations } from '@/hooks/use-integrations';` plus
`import { accountName, GITHUB, type Account } from '@/lib/integrations';`:

```tsx
const connectedFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

/** One connected account: what it is called, since when, and how to remove it. */
function ConnectedAccount({
  account,
  onRename,
  onDisconnect,
  disabled,
}: {
  account: Account;
  onRename: (accountId: number, label: string | null) => void;
  onDisconnect: (accountId: number) => void;
  disabled: boolean;
}) {
  const since = new Date(account.connectedAt);
  const identity = account.identity ?? account.accountKey;

  // Committed on blur rather than per keystroke: naming an account is not
  // worth a database write per character.
  const commit = (value: string) => {
    const label = value.trim() === '' ? null : value.trim();

    if (label !== account.label) {
      onRename(account.id, label);
    }
  };

  return (
    <div className="flex items-center justify-between gap-4 border-t border-border py-3">
      <div className="min-w-0 flex-1">
        <input
          type="text"
          aria-label={`Name for ${identity}`}
          defaultValue={account.label ?? ''}
          placeholder={identity}
          disabled={disabled}
          onBlur={(event) => commit(event.target.value)}
          className="w-full rounded-sm bg-transparent text-sm font-medium outline-none placeholder:font-normal placeholder:text-muted-foreground focus:ring-1 focus:ring-ring"
        />
        <p className="mt-0.5 micro text-muted-foreground">
          {Number.isNaN(since.getTime())
            ? 'Connected'
            : `Connected ${connectedFormat.format(since)}`}
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        disabled={disabled}
        onClick={() => onDisconnect(account.id)}
      >
        Disconnect {accountName(account)}
      </Button>
    </div>
  );
}

function GithubIntegration() {
  const { accountsFor, login, connecting, status, error, connect, disconnect, rename } =
    useIntegrations();

  const accounts = accountsFor(GITHUB);
  const isBusy = status === 'working' || status === 'awaiting-user';
  const showCode = login !== null && connecting === GITHUB;

  return (
    <SettingsSection
      title="GitHub"
      description="Lets Chief read your pull requests. Sign-in happens in your browser and the token is stored only on this machine."
      state={
        status === 'loading' ? (
          <Chip tone="quiet">Checking</Chip>
        ) : accounts.length === 0 ? (
          <Chip tone="quiet">Not connected</Chip>
        ) : (
          <Chip tone="verified" dot>
            {accounts.length === 1 ? 'Connected' : `${accounts.length} accounts`}
          </Chip>
        )
      }
    >
      {accounts.length > 0 && (
        <div className="mt-3">
          {accounts.map((account) => (
            <ConnectedAccount
              key={account.id}
              account={account}
              onRename={rename}
              onDisconnect={disconnect}
              disabled={isBusy}
            />
          ))}
        </div>
      )}

      {showCode && (
        <div className="mt-4 rounded-md border border-border p-4" role="status">
          <p className="text-sm">
            Enter this code at{' '}
            <span className="font-mono text-[13px]" data-selectable>
              {login.verificationUri}
            </span>
          </p>
          <p className="mt-2 font-mono text-xl tracking-[0.2em]" data-selectable>
            {login.userCode}
          </p>
          <p className="mt-2.5 flex items-center gap-2 micro text-muted-foreground">
            <Dots />
            Waiting for you to finish in the browser
          </p>
        </div>
      )}

      {error !== null && (
        <p
          className="mt-4 rounded-md border border-destructive bg-destructive-surface px-3.5 py-3 text-[13px] leading-snug text-destructive-text"
          role="alert"
        >
          {error}
        </p>
      )}

      <div className="mt-4">
        <Button
          size="sm"
          variant={accounts.length > 0 ? 'outline' : 'default'}
          onClick={() => connect(GITHUB)}
          disabled={isBusy || status === 'loading'}
        >
          <Github aria-hidden />
          {accounts.length > 0 ? 'Add another GitHub account' : 'Connect GitHub'}
        </Button>
      </div>
    </SettingsSection>
  );
}
```

The chip stays state, never an action — the count is a fact about the section, and every action is a button.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `pnpm test -- SettingsView`
Expected: PASS.

- [ ] **Step 5: Run everything**

Run: `pnpm check`
Expected: format, lint, typecheck and tests all pass.

Run: `pnpm rust:test && pnpm rust:lint`
Expected: PASS, no warnings.

Run: `pnpm verify:layout`
Expected: PASS. The shell measurements are unchanged by this work; if a settings-screen assertion fails on height, the account rows have grown the section — adjust the test's expectation, not the design tokens.

- [ ] **Step 6: Commit**

```bash
git add src/components/views/SettingsView.tsx src/components/views/SettingsView.test.tsx
git commit -m "feat(ui): list every connected account on the settings screen"
```

---

## Done when

- `pnpm verify` passes end to end.
- A database created before this change still has its GitHub credential and its work log, and the daemon does not re-log anything it had already logged.
- Two GitHub accounts can be connected at once, each nameable, each removable on its own.
- `git log --format='%an <%ae> | %cn <%ce>'` shows `Scott Mallinson <scott@scottmallinson.com>` for every commit on the branch, and no commit message carries a co-author or tool trailer.

## Not in this plan

Step 8 (Outlook) builds `oauth/flow.rs` on top of the two primitives here, and is where the loopback listener gets its first real consumer. Nothing in this plan sends a single new request to anybody.
