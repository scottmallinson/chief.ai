//! Client for a local Ollama instance.
//!
//! Inference happens on the user's own machine. [`Client`] refuses to talk to
//! anything but a loopback address, so a misconfiguration cannot quietly turn
//! into a hosted model seeing the user's work.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where Ollama listens by default.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Give up if Ollama has not accepted a connection by now. Short, because a
/// refused connection means it is not running and we want to say so quickly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// A small model on modest hardware can think for a while before it answers.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(300);

/// Who authored a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    /// The result of a tool call, fed back to the model.
    Tool,
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Tools the model asked us to run. Populated by Ollama, never by us.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Which tool a [`Role::Tool`] message is answering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }
}

/// A request from the model to run one of our tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// Arguments as the model produced them; validated by the caller.
    #[serde(default)]
    pub arguments: Value,
}

/// A tool we advertise to the model, in Ollama's function-calling format.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    pub function: ToolFunction,
}

impl Tool {
    pub fn function(function: ToolFunction) -> Self {
        Self {
            kind: "function",
            function,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    /// A JSON Schema object describing the arguments.
    pub parameters: Value,
}

/// The body posted to `/api/chat`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// Always false: we want one complete reply, not a token stream.
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: false,
            tools: Vec::new(),
        }
    }

    /// Advertise tools the model may call.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }
}

/// A complete (non-streamed) reply.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub model: String,
    pub message: Message,
    #[serde(default)]
    pub done: bool,
}

/// What can go wrong talking to a local model.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("'{0}' is not a valid URL")]
    InvalidUrl(String),
    #[error("refusing to send your work to '{0}' — inference must stay on this machine")]
    NotLoopback(String),
    #[error("could not reach Ollama at {base_url}. Is it running? Start it with `ollama serve`.")]
    Unreachable { base_url: String },
    #[error("the model '{model}' is not installed. Install it with `ollama pull {model}`.")]
    ModelNotFound { model: String },
    #[error("the model took too long to answer")]
    Timeout,
    #[error("Ollama returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("could not read Ollama's response: {0}")]
    Decode(String),
    #[error("request to Ollama failed: {0}")]
    Transport(String),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Is this URL pointing at the machine we are running on?
fn is_loopback(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };

    // IPv6 hosts arrive bracketed, e.g. `[::1]`.
    let host = host.trim_start_matches('[').trim_end_matches(']');

    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// An HTTP client bound to a local Ollama instance.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: reqwest::Url,
}

impl Client {
    /// Connect to Ollama on its default local port.
    pub fn new() -> Result<Self, Error> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    /// Connect to Ollama at `base_url`, which must be a loopback address.
    pub fn with_base_url(base_url: &str) -> Result<Self, Error> {
        let base_url =
            reqwest::Url::parse(base_url).map_err(|_| Error::InvalidUrl(base_url.to_string()))?;

        if !is_loopback(&base_url) {
            return Err(Error::NotLoopback(base_url.to_string()));
        }

        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(RESPONSE_TIMEOUT)
            // Nothing here should ever leave the machine, so a proxy would be
            // both useless and a way for data to escape.
            .no_proxy()
            .build()
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(Self { http, base_url })
    }

    /// Ask the model to continue a conversation.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, Error> {
        let url = self
            .base_url
            .join("/api/chat")
            .map_err(|_| Error::InvalidUrl(self.base_url.to_string()))?;

        let response = self
            .http
            .post(url)
            .json(request)
            .send()
            .await
            .map_err(|error| self.transport_error(error))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            return Err(if status == reqwest::StatusCode::NOT_FOUND {
                Error::ModelNotFound {
                    model: request.model.clone(),
                }
            } else {
                Error::Status {
                    status: status.as_u16(),
                    body,
                }
            });
        }

        response
            .json::<ChatResponse>()
            .await
            .map_err(|error| Error::Decode(error.to_string()))
    }

    fn transport_error(&self, error: reqwest::Error) -> Error {
        if error.is_connect() {
            Error::Unreachable {
                base_url: self.base_url.to_string(),
            }
        } else if error.is_timeout() {
            Error::Timeout
        } else {
            Error::Transport(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_loopback_addresses() {
        for url in [
            "http://localhost:11434",
            "http://127.0.0.1:11434",
            "http://[::1]:11434",
        ] {
            assert!(
                Client::with_base_url(url).is_ok(),
                "{url} should be allowed"
            );
        }
    }

    #[test]
    fn refuses_to_talk_to_anywhere_else() {
        for url in [
            "https://api.openai.com",
            "http://192.168.1.10:11434",
            "http://ollama.example.com",
        ] {
            let error = Client::with_base_url(url).expect_err("{url} should be refused");
            assert!(matches!(error, Error::NotLoopback(_)), "got {error:?}");
        }
    }

    #[test]
    fn rejects_a_malformed_url() {
        let error = Client::with_base_url("not a url").expect_err("should be refused");
        assert!(matches!(error, Error::InvalidUrl(_)), "got {error:?}");
    }

    #[test]
    fn omits_the_tools_array_when_there_is_nothing_to_offer() {
        let request = ChatRequest::new("llama3.2:3b", vec![Message::user("hello")]);
        let body = serde_json::to_value(&request).expect("should serialize");

        assert_eq!(body["stream"], json!(false));
        assert!(body.get("tools").is_none(), "empty tools should be omitted");
    }

    #[test]
    fn serializes_tools_in_ollamas_function_format() {
        let request = ChatRequest::new("llama3.2:3b", vec![Message::user("what is open?")])
            .with_tools(vec![Tool::function(ToolFunction {
                name: "fetch_github_prs".to_string(),
                description: "List the user's open pull requests".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "state": { "type": "string" } },
                    "required": ["state"],
                }),
            })]);

        let body = serde_json::to_value(&request).expect("should serialize");

        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": {
                    "name": "fetch_github_prs",
                    "description": "List the user's open pull requests",
                    "parameters": {
                        "type": "object",
                        "properties": { "state": { "type": "string" } },
                        "required": ["state"],
                    },
                },
            }])
        );
    }

    #[test]
    fn reads_a_reply_that_asks_for_a_tool() {
        let raw = json!({
            "model": "llama3.2:3b",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": { "name": "fetch_github_prs", "arguments": { "state": "open" } },
                }],
            },
            "done": true,
        });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");
        let call = &response.message.tool_calls[0];

        assert_eq!(call.function.name, "fetch_github_prs");
        assert_eq!(call.function.arguments["state"], json!("open"));
    }

    #[test]
    fn reads_a_plain_text_reply() {
        let raw = json!({
            "model": "llama3.2:3b",
            "message": { "role": "assistant", "content": "You merged two pull requests." },
            "done": true,
        });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");

        assert_eq!(response.message.role, Role::Assistant);
        assert_eq!(response.message.content, "You merged two pull requests.");
        assert!(response.message.tool_calls.is_empty());
    }
}

/// A throwaway HTTP server on loopback, so the requests [`Client`] actually
/// puts on the wire are covered without a running Ollama.
#[cfg(test)]
pub(crate) mod test_support {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    const HEADER_END: &[u8] = b"\r\n\r\n";

    fn headers_end(buffer: &[u8]) -> Option<usize> {
        buffer
            .windows(HEADER_END.len())
            .position(|window| window == HEADER_END)
            .map(|index| index + HEADER_END.len())
    }

    fn content_length(headers: &str) -> usize {
        headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())?
            })
            .unwrap_or(0)
    }

    /// Split a raw HTTP request into its request line and body.
    pub fn split(request: &str) -> (&str, &str) {
        let (headers, body) = request
            .split_once("\r\n\r\n")
            .expect("request should have a body");
        let request_line = headers.lines().next().expect("request should have a line");

        (request_line, body)
    }

    /// Serve `replies` in order, one per request, then hand back everything
    /// the client sent. Bodies may be borrowed or owned, so a test can build
    /// one up rather than having to write it as a literal.
    pub fn serve<B: Into<String>>(
        replies: Vec<(&'static str, B)>,
    ) -> (String, JoinHandle<Vec<String>>) {
        let replies: Vec<(&'static str, String)> = replies
            .into_iter()
            .map(|(status_line, body)| (status_line, body.into()))
            .collect();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("should bind loopback");
        listener
            .set_nonblocking(true)
            .expect("should be non-blocking");
        let port = listener
            .local_addr()
            .expect("socket should have an address")
            .port();

        let handle = tokio::spawn(async move {
            let listener = TcpListener::from_std(listener).expect("should adopt the listener");
            let mut received = Vec::new();

            for (status_line, body) in replies {
                let (mut socket, _) = listener.accept().await.expect("should accept a connection");

                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];

                loop {
                    let read = socket
                        .read(&mut chunk)
                        .await
                        .expect("should read a request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);

                    if let Some(end) = headers_end(&request) {
                        let headers = String::from_utf8_lossy(&request[..end]).to_string();
                        if request.len() >= end + content_length(&headers) {
                            break;
                        }
                    }
                }

                let response = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("should write a response");
                socket.flush().await.expect("should flush the response");

                received.push(String::from_utf8_lossy(&request).to_string());
            }

            received
        });

        (format!("http://127.0.0.1:{port}"), handle)
    }

    /// Bind and immediately release a port, so nothing is listening on it.
    pub fn closed_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("should bind loopback");
        let port = listener
            .local_addr()
            .expect("socket should have an address")
            .port();
        drop(listener);

        format!("http://127.0.0.1:{port}")
    }
}

/// Tests that exercise [`Client`] against the stub server in [`test_support`].
#[cfg(test)]
mod http_tests {
    use super::test_support::{closed_port, serve, split};
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn posts_the_conversation_to_the_chat_endpoint() {
        const BODY: &str = r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":"Two pull requests."},"done":true}"#;

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", BODY)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let response = client
            .chat(&ChatRequest::new(
                "llama3.2:3b",
                vec![
                    Message::system("You are Chief."),
                    Message::user("What did I ship?"),
                ],
            ))
            .await
            .expect("the stub should answer");

        assert_eq!(response.message.content, "Two pull requests.");

        let requests = server.await.expect("the stub should finish");
        let (request_line, body) = split(&requests[0]);

        assert!(
            request_line.starts_with("POST /api/chat "),
            "unexpected request line: {request_line}"
        );

        let sent: serde_json::Value = serde_json::from_str(body).expect("body should be JSON");
        assert_eq!(sent["model"], json!("llama3.2:3b"));
        assert_eq!(sent["stream"], json!(false));
        assert_eq!(sent["messages"][0]["role"], json!("system"));
        assert_eq!(sent["messages"][1]["content"], json!("What did I ship?"));
    }

    #[tokio::test]
    async fn explains_that_a_model_is_not_installed() {
        const BODY: &str = r#"{"error":"model 'llama3.2:3b' not found"}"#;

        let (base_url, server) = serve(vec![("HTTP/1.1 404 Not Found", BODY)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat(&ChatRequest::new(
                "llama3.2:3b",
                vec![Message::user("hello")],
            ))
            .await
            .expect_err("a 404 should be an error");

        match error {
            Error::ModelNotFound { model } => assert_eq!(model, "llama3.2:3b"),
            other => panic!("expected ModelNotFound, got {other:?}"),
        }

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn surfaces_an_unexpected_status() {
        let (base_url, server) = serve(vec![("HTTP/1.1 500 Internal Server Error", "boom")]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat(&ChatRequest::new("llama3.2:3b", vec![Message::user("hi")]))
            .await
            .expect_err("a 500 should be an error");

        match error {
            Error::Status { status, body } => {
                assert_eq!(status, 500);
                assert_eq!(body, "boom");
            }
            other => panic!("expected Status, got {other:?}"),
        }

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn says_when_ollama_is_not_running() {
        let client = Client::with_base_url(&closed_port()).expect("loopback should be allowed");

        let error = client
            .chat(&ChatRequest::new("llama3.2:3b", vec![Message::user("hi")]))
            .await
            .expect_err("a closed port should be an error");

        assert!(
            matches!(error, Error::Unreachable { .. }),
            "expected Unreachable, got {error:?}"
        );
    }
}
