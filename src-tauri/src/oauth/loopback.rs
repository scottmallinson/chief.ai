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
