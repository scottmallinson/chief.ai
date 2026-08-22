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

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Give up if the user never finishes in the browser.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Give up on one connection that never asks for anything. A browser writes
/// its request line as soon as it has connected, so this only ever ends a
/// socket that was not going to say anything — one of which would otherwise
/// sit in the read for the whole of `TIMEOUT`, holding a file descriptor.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How many connections may be answered at once. A browser opens a handful;
/// the rest wait in the kernel's backlog, which costs this process nothing,
/// and it is a process that is also holding the database pool and the engine.
const MAX_CONNECTIONS: usize = 32;

/// How many failed accepts in a row mean the listener itself is broken rather
/// than one client having gone away.
const MAX_ACCEPT_FAILURES: u32 = 8;

/// A pause after a failed accept, so a listener that is out of file
/// descriptors waits for one to come back instead of spinning the CPU.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// The only path this answers.
const CALLBACK_PATH: &str = "/oauth/callback";

/// A request line longer than this is not a redirect.
const MAX_REQUEST_LINE: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not listen for the sign-in redirect: {0}")]
    Bind(String),
    #[error("could not answer the sign-in redirect: {0}")]
    Accept(String),
    #[error("sign-in was not completed in time")]
    TimedOut,
    #[error("the browser did not come back with a sign-in code")]
    NoCode,
    #[error("sign-in was declined")]
    Declined,
    /// What the authorization server said, in its own words. A misconfigured
    /// client id, a scope the app may not ask for or a fault at the provider
    /// all arrive this way, and this is the only place that account of it
    /// exists.
    #[error("sign-in failed: {0}")]
    Refused(String),
}

/// What the browser handed back.
#[derive(Clone, PartialEq, Eq)]
pub struct Redirect {
    pub code: String,
    pub state: String,
}

/// The code is one half of the token exchange, so it never renders itself, for
/// the same reason [`super::pkce::Verifier`] does not: derived `Debug` would
/// carry it into any error or trace that formats a struct holding one. The
/// state is a public nonce and prints as itself.
impl std::fmt::Debug for Redirect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Redirect")
            .field("code", &"<redacted>")
            .field("state", &self.state)
            .finish()
    }
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
        self.wait_for(TIMEOUT).await
    }

    /// The same, giving up after a chosen wait. Only the tests choose: it is
    /// how the timeout is exercised at all, and it is what lets a test that
    /// must not hang fail in seconds rather than in two minutes.
    async fn wait_for(self, timeout: Duration) -> Result<Redirect, Error> {
        match tokio::time::timeout(timeout, self.accept_callback()).await {
            Ok(result) => result,
            Err(_) => Err(Error::TimedOut),
        }
    }

    /// Answer every connection at once, and take the first real callback.
    ///
    /// Concurrently rather than one at a time, because a browser opens more
    /// sockets than it sends requests on: a speculative connection it never
    /// writes to is ordinary, and answering in turn would let one of those
    /// hold the redirect behind it until the timeout.
    async fn accept_callback(&self) -> Result<Redirect, Error> {
        let mut answering = FuturesUnordered::new();
        let mut failures = 0_u32;

        loop {
            let in_flight = answering.len();

            tokio::select! {
                // Stop taking new connections once enough are in flight. The
                // rest queue in the kernel until one finishes.
                accepted = self.listener.accept(), if in_flight < MAX_CONNECTIONS => {
                    match accepted {
                        Ok((socket, _)) => {
                            failures = 0;
                            answering.push(answer(socket));
                        }
                        // Usually one client's problem — it reset before the
                        // handshake finished, or the process is momentarily
                        // out of file descriptors — and the redirect may still
                        // be to come, so one failure does not end the sign-in.
                        // A listener failing over and over is a broken one.
                        Err(error) => {
                            failures += 1;
                            if failures >= MAX_ACCEPT_FAILURES {
                                return Err(Error::Accept(error.to_string()));
                            }
                            tokio::time::sleep(ACCEPT_BACKOFF).await;
                        }
                    }
                }
                // `None` is a connection that was not the callback, already
                // turned away; the wait goes on.
                Some(answered) = answering.next() => {
                    if let Some(outcome) = answered {
                        return outcome;
                    }
                }
            }
        }
    }
}

/// Answer one connection, and report a callback if that is what it was.
///
/// Bounded by `REQUEST_TIMEOUT`, so a connection that opens and then says
/// nothing is let go of rather than held until the sign-in as a whole gives
/// up.
async fn answer(socket: TcpStream) -> Option<Result<Redirect, Error>> {
    tokio::time::timeout(REQUEST_TIMEOUT, read_and_answer(socket))
        .await
        .ok()
        .flatten()
}

async fn read_and_answer(mut socket: TcpStream) -> Option<Result<Redirect, Error>> {
    let Some(query) = read_request_line(&mut socket)
        .await
        .as_deref()
        .and_then(request_target)
        .and_then(callback_query)
    else {
        respond(&mut socket, "404 Not Found", "Not found.").await;
        return None;
    };

    let outcome = interpret(&query);
    let body = match &outcome {
        Ok(_) => "Signed in. You can close this tab and go back to Chief.",
        Err(_) => "Sign-in did not complete. Go back to Chief and try again.",
    };
    respond(&mut socket, "200 OK", body).await;

    Some(outcome)
}

/// The query of a request to the callback path, or nothing for any other path.
///
/// The path has to match in full. On a prefix match `/oauth/callbackery` would
/// be read as a sign-in that carried nothing, ending the wait on the redirect
/// still to come.
fn callback_query(target: &str) -> Option<String> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    (path == CALLBACK_PATH).then(|| query.to_string())
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
    let mut description = None;

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));

        match name {
            "code" => code = Some(decode(value)),
            "state" => state = Some(decode(value)),
            "error" => failure = Some(decode(value)),
            "error_description" => description = Some(decode(value)),
            _ => {}
        }
    }

    // RFC 6749 §4.1.2.1. Say what the server said: an unusable client id, a
    // scope it will not grant or a fault of its own is not the browser failing
    // to come back, and reporting it as that leaves nobody able to act on it.
    if let Some(failure) = failure.filter(|failure| !failure.is_empty()) {
        return Err(match failure.as_str() {
            "access_denied" => Error::Declined,
            _ => Error::Refused(match description.filter(|text| !text.is_empty()) {
                Some(description) => format!("{failure} ({description})"),
                None => failure,
            }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// What the tests that open more than one connection wait for. Long enough
    /// that a loaded machine still finishes, short enough that undoing the
    /// concurrency fails a test in seconds instead of hanging for two minutes.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// The host and port of a redirect uri, which is what a socket connects to.
    fn authority(uri: &str) -> String {
        uri.trim_start_matches("http://")
            .split('/')
            .next()
            .expect("the redirect uri should have an authority")
            .to_string()
    }

    /// Act like the browser: GET the redirect and read the page back.
    async fn visit(uri: &str, query: &str) -> String {
        let authority = authority(uri);

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
    async fn names_the_reason_the_authorization_server_refused() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();

        let browser = tokio::spawn(async move {
            visit(
                &uri,
                "error=unauthorized_client&error_description=The+client+is+not+authorized",
            )
            .await
        });
        let error = listener.wait().await.expect_err("a refusal should surface");

        let reported = error.to_string();
        assert!(reported.contains("unauthorized_client"), "got {reported}");
        assert!(
            reported.contains("The client is not authorized"),
            "got {reported}"
        );
        browser.await.expect("the browser should finish");
    }

    #[test]
    fn never_renders_the_authorization_code() {
        let redirect = Redirect {
            code: "the-authorization-code".to_string(),
            state: "xyz".to_string(),
        };

        let rendered = format!("{redirect:?}");
        assert!(
            !rendered.contains("the-authorization-code"),
            "got {rendered}"
        );
    }

    #[tokio::test]
    async fn ignores_anything_that_is_not_the_callback() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();
        let authority = authority(&uri);

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

        let redirect = listener
            .wait_for(TEST_TIMEOUT)
            .await
            .expect("should receive the redirect");
        assert_eq!(redirect.code, "real");

        let turned_away = stray.await.expect("the stray request should finish");
        assert!(turned_away.starts_with("HTTP/1.1 404"), "got {turned_away}");
    }

    #[tokio::test]
    async fn turns_away_a_path_that_only_begins_with_the_callback() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();
        let authority = authority(&uri);

        let stray = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(&authority)
                .await
                .expect("should connect");
            socket
                .write_all(
                    format!(
                        "GET {CALLBACK_PATH}ery?code=stray&state=s HTTP/1.1\r\n\
                         Host: local\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("should send");

            let mut response = String::new();
            socket.read_to_string(&mut response).await.ok();

            visit(&uri, "code=real&state=s").await;
            response
        });

        let redirect = listener
            .wait_for(TEST_TIMEOUT)
            .await
            .expect("should receive the redirect");
        assert_eq!(redirect.code, "real");

        let turned_away = stray.await.expect("the stray request should finish");
        assert!(turned_away.starts_with("HTTP/1.1 404"), "got {turned_away}");
    }

    #[tokio::test]
    async fn answers_the_redirect_while_another_connection_sits_idle() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();

        // A browser opens more sockets than it sends requests on. One it never
        // writes to must not hold the redirect behind it until the timeout.
        let idle = tokio::net::TcpStream::connect(authority(&uri))
            .await
            .expect("should connect");

        let browser = tokio::spawn(async move { visit(&uri, "code=real&state=s").await });
        let redirect = listener
            .wait_for(TEST_TIMEOUT)
            .await
            .expect("should receive the redirect");

        assert_eq!(redirect.code, "real");
        browser.await.expect("the browser should finish");
        drop(idle);
    }

    #[tokio::test]
    async fn turns_away_a_request_line_longer_than_a_redirect() {
        let listener = Listener::bind().await.expect("should bind loopback");
        let uri = listener.redirect_uri();
        let authority = authority(&uri);

        let stray = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(&authority)
                .await
                .expect("should connect");
            // Past what a request line may be, and with no end to it, so only
            // the ceiling stops the read.
            socket
                .write_all(format!("GET /{}", "x".repeat(MAX_REQUEST_LINE + 1)).as_bytes())
                .await
                .expect("should send");

            let mut response = String::new();
            socket.read_to_string(&mut response).await.ok();

            visit(&uri, "code=real&state=s").await;
            response
        });

        let redirect = listener
            .wait_for(TEST_TIMEOUT)
            .await
            .expect("should receive the redirect");
        assert_eq!(redirect.code, "real");

        let turned_away = stray.await.expect("the stray request should finish");
        assert!(turned_away.starts_with("HTTP/1.1 404"), "got {turned_away}");
    }

    #[tokio::test]
    async fn gives_up_when_the_browser_never_comes_back() {
        let listener = Listener::bind().await.expect("should bind loopback");

        let error = listener
            .wait_for(Duration::from_millis(50))
            .await
            .expect_err("the wait should end on its own");

        assert!(matches!(error, Error::TimedOut), "got {error:?}");
    }

    #[test]
    fn refuses_a_callback_that_carries_a_code_but_no_state() {
        let outcome = interpret("code=abc123");

        assert!(matches!(outcome, Err(Error::NoCode)), "got {outcome:?}");
    }

    #[test]
    fn answers_only_a_get() {
        assert_eq!(
            request_target("GET /oauth/callback?code=abc HTTP/1.1"),
            Some("/oauth/callback?code=abc")
        );
        assert_eq!(
            request_target("POST /oauth/callback?code=abc HTTP/1.1"),
            None
        );
        assert_eq!(request_target("HEAD /oauth/callback HTTP/1.1"), None);
    }

    #[test]
    fn keeps_the_error_the_server_named() {
        let outcome = interpret("error=invalid_scope&state=xyz");

        assert!(
            matches!(&outcome, Err(Error::Refused(reason)) if reason == "invalid_scope"),
            "got {outcome:?}"
        );
    }

    #[test]
    fn reads_a_refusal_by_the_user_as_declined_however_it_is_described() {
        let outcome = interpret("error=access_denied&error_description=The+user+said+no");

        assert!(matches!(outcome, Err(Error::Declined)), "got {outcome:?}");
    }

    #[test]
    fn an_empty_error_is_not_a_reason() {
        let outcome = interpret("error=");

        assert!(matches!(outcome, Err(Error::NoCode)), "got {outcome:?}");
    }
}
