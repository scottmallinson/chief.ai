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

/// How long Ollama should keep the model resident after answering.
///
/// Ollama's own default is five minutes, after which the next question pays to
/// read a couple of gigabytes off disk again — a dead pause before a single
/// token appears, and worse on Windows, where the read goes past a virus
/// scanner. Chief is an application you leave open and ask things through the
/// day, so holding the weights in memory is the right trade.
pub const KEEP_ALIVE: &str = "30m";

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

/// How the model should be run, over and above the conversation itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Options {
    /// The context window, in tokens.
    ///
    /// Deliberately the same for every request Chief makes: Ollama loads a
    /// model *per context size*, so asking for a different one would evict the
    /// copy already in memory and make the next answer wait for a reload.
    pub num_ctx: u32,
    /// The most tokens one reply may run to. This is the ceiling on how long a
    /// question can take: a small model that starts rambling stops instead.
    pub num_predict: u32,
    /// Low, because these answers are about what the tools actually returned.
    /// Invention is the failure mode here, not dullness.
    pub temperature: f32,
}

impl Options {
    /// Room for a conversation plus a page of tool results. Larger costs
    /// memory for the key/value cache and buys nothing Chief asks for.
    const CONTEXT: u32 = 4096;

    /// The defaults every request starts from.
    pub const fn new() -> Self {
        Self {
            num_ctx: Self::CONTEXT,
            num_predict: 512,
            temperature: 0.2,
        }
    }

    /// Cap the reply at `tokens`. Sampling limits are applied per request, so
    /// unlike [`Options::num_ctx`] this can vary without reloading the model.
    #[must_use]
    pub const fn with_answer_length(mut self, tokens: u32) -> Self {
        self.num_predict = tokens;
        self
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// The body posted to `/api/chat`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// Whether to receive the reply a token at a time. Set by whichever of
    /// [`Client::chat`] and [`Client::chat_stream`] is doing the asking.
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    pub options: Options,
    /// How long to keep the model in memory afterwards.
    pub keep_alive: &'static str,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: false,
            tools: Vec::new(),
            options: Options::new(),
            keep_alive: KEEP_ALIVE,
        }
    }

    /// Advertise tools the model may call.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// Run the model differently to the defaults.
    #[must_use]
    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
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

/// One line of a streamed reply: a few more characters of the answer, or the
/// tool calls the model settled on.
#[derive(Debug, Clone, Default, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    message: Option<Message>,
    /// Ollama can report a failure part way through an otherwise successful
    /// response, so this has to be watched for rather than assumed away.
    #[serde(default)]
    error: Option<String>,
}

/// How a model download is progressing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    /// Ollama's own wording, e.g. "pulling manifest" or "verifying sha256 digest".
    pub status: String,
    /// Bytes fetched so far, while a layer is downloading.
    #[serde(default)]
    pub completed: u64,
    /// Bytes in the layer being fetched, when known.
    #[serde(default)]
    pub total: u64,
}

impl PullProgress {
    /// Progress through the current layer, 0.0 to 1.0, when it can be known.
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0).then(|| (self.completed as f64 / self.total as f64).clamp(0.0, 1.0))
    }

    /// Whether Ollama considers the download finished.
    pub fn is_done(&self) -> bool {
        self.status == "success"
    }
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

    /// Ask the model to continue a conversation, waiting for the whole reply.
    ///
    /// Use this where nobody is watching the answer appear — the background
    /// summariser, or a warm-up. When a person is waiting, prefer
    /// [`Client::chat_stream`].
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, Error> {
        let response = self.ask(request, false).await?;

        response
            .json::<ChatResponse>()
            .await
            .map_err(|error| Error::Decode(error.to_string()))
    }

    /// Ask the model to continue a conversation, handing over the answer as it
    /// is written.
    ///
    /// A three billion parameter model on a laptop writes at reading speed but
    /// takes tens of seconds to finish, so waiting for the whole reply is the
    /// difference between an assistant that feels immediate and one that feels
    /// broken. The complete [`Message`] is still returned — the orchestrator
    /// needs the tool calls, and the command needs something to return — so
    /// `on_token` is purely for showing progress.
    pub async fn chat_stream<F>(
        &self,
        request: &ChatRequest,
        mut on_token: F,
    ) -> Result<Message, Error>
    where
        F: FnMut(&str),
    {
        let response = self.ask(request, true).await?;
        let mut answer = Message::new(Role::Assistant, String::new());

        read_lines(response, |line| {
            let chunk: ChatChunk =
                serde_json::from_str(line).map_err(|error| Error::Decode(error.to_string()))?;

            if let Some(message) = chunk.error {
                return Err(Error::Status {
                    status: 200,
                    body: message,
                });
            }

            let Some(part) = chunk.message else {
                return Ok(());
            };

            if !part.content.is_empty() {
                on_token(&part.content);
                answer.content.push_str(&part.content);
            }

            answer.tool_calls.extend(part.tool_calls);

            Ok(())
        })
        .await?;

        Ok(answer)
    }

    /// Load the model into memory without asking it anything.
    ///
    /// Ollama reads the weights on first use, which on a cold start is a long
    /// silence before the first answer. Doing it while the window is still
    /// opening means the user's first question does not pay for it.
    pub async fn preload(&self, model: &str) -> Result<(), Error> {
        self.chat(&ChatRequest::new(model, Vec::new())).await?;

        Ok(())
    }

    /// Post a chat request and hand back the response once it is known to be
    /// good, so both the buffered and the streamed path fail the same way.
    async fn ask(&self, request: &ChatRequest, stream: bool) -> Result<reqwest::Response, Error> {
        let url = self.endpoint("/api/chat")?;
        let request = ChatRequest {
            stream,
            ..request.clone()
        };

        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|error| self.transport_error(error))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            return Err(if status == reqwest::StatusCode::NOT_FOUND {
                Error::ModelNotFound {
                    model: request.model,
                }
            } else {
                Error::Status {
                    status: status.as_u16(),
                    body,
                }
            });
        }

        Ok(response)
    }

    /// Ollama's version, which doubles as a health check.
    pub async fn version(&self) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Version {
            version: String,
        }

        let url = self.endpoint("/api/version")?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| self.transport_error(error))?;

        Ok(self.read::<Version>(response).await?.version)
    }

    /// The models installed on this machine.
    pub async fn installed_models(&self) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct Tags {
            #[serde(default)]
            models: Vec<Model>,
        }

        #[derive(Deserialize)]
        struct Model {
            #[serde(default)]
            name: String,
        }

        let url = self.endpoint("/api/tags")?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| self.transport_error(error))?;

        let tags = self.read::<Tags>(response).await?;

        Ok(tags.models.into_iter().map(|model| model.name).collect())
    }

    /// Download a model, reporting progress as Ollama streams it.
    ///
    /// The transfer is between Ollama and its registry; nothing about the
    /// user's own work is involved.
    pub async fn pull<F>(&self, model: &str, mut on_progress: F) -> Result<(), Error>
    where
        F: FnMut(PullProgress),
    {
        let url = self.endpoint("/api/pull")?;
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({ "model": model, "stream": true }))
            .send()
            .await
            .map_err(|error| self.transport_error(error))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            return Err(Error::Status {
                status: status.as_u16(),
                body,
            });
        }

        read_lines(response, |line| report(line, &mut on_progress)).await
    }

    fn endpoint(&self, path: &str) -> Result<reqwest::Url, Error> {
        self.base_url
            .join(path)
            .map_err(|_| Error::InvalidUrl(self.base_url.to_string()))
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, Error> {
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            return Err(Error::Status {
                status: status.as_u16(),
                body,
            });
        }

        response
            .json::<T>()
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

/// Read a newline-delimited JSON response, handing each line to `on_line`.
///
/// Ollama streams both downloads and answers this way, and a chunk off the wire
/// can end mid-line or, worse, mid-character. Bytes are buffered and only
/// decoded once a whole line is in hand, so an accented word does not arrive as
/// a replacement character.
async fn read_lines<F>(response: reqwest::Response, mut on_line: F) -> Result<(), Error>
where
    F: FnMut(&str) -> Result<(), Error>,
{
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut pending: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| Error::Transport(error.to_string()))?;
        pending.extend_from_slice(&chunk);

        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            deliver(&line, &mut on_line)?;
        }
    }

    // A last line the server did not terminate.
    deliver(&pending, &mut on_line)
}

/// Hand one line over, unless it is blank.
fn deliver<F>(line: &[u8], on_line: &mut F) -> Result<(), Error>
where
    F: FnMut(&str) -> Result<(), Error>,
{
    let line = String::from_utf8_lossy(line);
    let line = line.trim();

    if line.is_empty() {
        return Ok(());
    }

    on_line(line)
}

/// Read one line of Ollama's pull stream, passing on anything meaningful.
///
/// Ollama reports a failure inside the body of an otherwise successful
/// response, so an `error` field has to be treated as one here.
fn report<F>(line: &str, on_progress: &mut F) -> Result<(), Error>
where
    F: FnMut(PullProgress),
{
    let value: Value =
        serde_json::from_str(line).map_err(|error| Error::Decode(error.to_string()))?;

    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Err(Error::Status {
            status: 200,
            body: message.to_string(),
        });
    }

    let progress: PullProgress =
        serde_json::from_value(value).map_err(|error| Error::Decode(error.to_string()))?;

    on_progress(progress);

    Ok(())
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
    fn asks_ollama_to_keep_the_model_in_memory() {
        let request = ChatRequest::new("llama3.2:3b", vec![Message::user("hello")]);
        let body = serde_json::to_value(&request).expect("should serialize");

        assert_eq!(body["keep_alive"], json!(KEEP_ALIVE));
    }

    #[test]
    fn bounds_how_long_an_answer_can_take() {
        let request = ChatRequest::new("llama3.2:3b", vec![Message::user("hello")]);
        let body = serde_json::to_value(&request).expect("should serialize");

        assert_eq!(body["options"]["num_predict"], json!(512));
        assert_eq!(body["options"]["num_ctx"], json!(4096));
    }

    #[test]
    fn a_shorter_answer_does_not_change_the_context_size() {
        // Ollama loads a model per context size, so a differing num_ctx would
        // evict the copy the chat is using.
        let brief = Options::new().with_answer_length(80);

        assert_eq!(brief.num_predict, 80);
        assert_eq!(brief.num_ctx, Options::new().num_ctx);
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
    fn progress_is_unknown_until_a_size_is_reported() {
        let starting = PullProgress {
            status: "pulling manifest".to_string(),
            completed: 0,
            total: 0,
        };

        assert_eq!(starting.fraction(), None);
        assert!(!starting.is_done());
    }

    #[test]
    fn progress_never_exceeds_one() {
        let overshoot = PullProgress {
            status: "pulling abc".to_string(),
            completed: 120,
            total: 100,
        };

        assert_eq!(overshoot.fraction(), Some(1.0));
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

    /// A streamed answer, as Ollama writes it: one JSON object per line.
    const STREAMED_ANSWER: &str = concat!(
        r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":"Two "},"done":false}"#,
        "\n",
        r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":"pull requests."},"done":false}"#,
        "\n",
        r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":""},"done":true}"#,
        "\n"
    );

    #[tokio::test]
    async fn hands_over_the_answer_as_it_is_written() {
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", STREAMED_ANSWER)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut tokens = Vec::new();
        let answer = client
            .chat_stream(
                &ChatRequest::new("llama3.2:3b", vec![Message::user("What did I ship?")]),
                |token| tokens.push(token.to_string()),
            )
            .await
            .expect("the stub should stream to the end");

        assert_eq!(
            tokens,
            ["Two ", "pull requests."],
            "each part should arrive"
        );
        assert_eq!(
            answer.content, "Two pull requests.",
            "and the whole answer should still be returned"
        );

        let requests = server.await.expect("the stub should finish");
        let (request_line, body) = split(&requests[0]);

        assert!(
            request_line.starts_with("POST /api/chat "),
            "unexpected request line: {request_line}"
        );

        let sent: serde_json::Value = serde_json::from_str(body).expect("body should be JSON");
        assert_eq!(sent["stream"], json!(true));
    }

    #[tokio::test]
    async fn reassembles_a_reply_split_between_chunks() {
        // The body arrives in one piece here; what matters is that a line
        // ending only at the very end is still delivered.
        const UNTERMINATED: &str = r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":"Done."},"done":true}"#;

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", UNTERMINATED)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let answer = client
            .chat_stream(
                &ChatRequest::new("llama3.2:3b", vec![Message::user("hi")]),
                |_| {},
            )
            .await
            .expect("a final line without a newline should still be read");

        assert_eq!(answer.content, "Done.");
        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn collects_the_tool_calls_out_of_a_stream() {
        const ASKS_FOR_A_TOOL: &str = concat!(
            r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"fetch_github_prs","arguments":{"state":"open"}}}]},"done":false}"#,
            "\n",
            r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":""},"done":true}"#,
            "\n"
        );

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", ASKS_FOR_A_TOOL)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut tokens = Vec::new();
        let answer = client
            .chat_stream(
                &ChatRequest::new("llama3.2:3b", vec![Message::user("What is open?")]),
                |token| tokens.push(token.to_string()),
            )
            .await
            .expect("the stub should stream to the end");

        assert!(tokens.is_empty(), "a tool call is not something to show");
        assert_eq!(answer.tool_calls.len(), 1);
        assert_eq!(answer.tool_calls[0].function.name, "fetch_github_prs");

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn surfaces_a_failure_reported_inside_the_answer_stream() {
        const BROKEN: &str = concat!(
            r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":"Tw"},"done":false}"#,
            "\n",
            r#"{"error":"model runner has stopped"}"#,
            "\n"
        );

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", BROKEN)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat_stream(
                &ChatRequest::new("llama3.2:3b", vec![Message::user("hi")]),
                |_| {},
            )
            .await
            .expect_err("an error in the stream should surface");

        match error {
            Error::Status { body, .. } => {
                assert!(body.contains("runner has stopped"), "got {body}")
            }
            other => panic!("expected Status, got {other:?}"),
        }

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn says_a_streamed_model_is_not_installed() {
        let (base_url, server) = serve(vec![(
            "HTTP/1.1 404 Not Found",
            r#"{"error":"model 'nope' not found"}"#,
        )]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat_stream(&ChatRequest::new("nope", vec![Message::user("hi")]), |_| {})
            .await
            .expect_err("a 404 should be an error");

        match error {
            Error::ModelNotFound { model } => assert_eq!(model, "nope"),
            other => panic!("expected ModelNotFound, got {other:?}"),
        }

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn loads_the_model_without_asking_it_anything() {
        const LOADED: &str =
            r#"{"model":"llama3.2:3b","message":{"role":"assistant","content":""},"done":true}"#;

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", LOADED)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        client
            .preload("llama3.2:3b")
            .await
            .expect("the stub should answer");

        let requests = server.await.expect("the stub should finish");
        let (_, body) = split(&requests[0]);
        let sent: serde_json::Value = serde_json::from_str(body).expect("body should be JSON");

        assert_eq!(sent["model"], json!("llama3.2:3b"));
        assert_eq!(sent["messages"], json!([]), "there is nothing to ask");
        assert_eq!(sent["keep_alive"], json!(KEEP_ALIVE));
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
    async fn reports_ollamas_version_as_a_health_check() {
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", r#"{"version":"0.5.1"}"#)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let version = client.version().await.expect("the stub should answer");

        assert_eq!(version, "0.5.1");

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = split(&requests[0]);
        assert!(
            request_line.starts_with("GET /api/version "),
            "unexpected request line: {request_line}"
        );
    }

    #[tokio::test]
    async fn lists_the_models_on_this_machine() {
        const TAGS: &str = r#"{"models":[{"name":"llama3.2:3b"},{"name":"mistral:latest"}]}"#;

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", TAGS)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let models = client
            .installed_models()
            .await
            .expect("the stub should answer");

        assert_eq!(models, ["llama3.2:3b", "mistral:latest"]);

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = split(&requests[0]);
        assert!(
            request_line.starts_with("GET /api/tags "),
            "unexpected request line: {request_line}"
        );
    }

    #[tokio::test]
    async fn reports_progress_while_pulling_a_model() {
        const STREAM: &str = concat!(
            r#"{"status":"pulling manifest"}"#,
            "\n",
            r#"{"status":"pulling abc","digest":"abc","total":100,"completed":40}"#,
            "\n",
            r#"{"status":"success"}"#,
            "\n"
        );

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", STREAM)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut seen = Vec::new();
        client
            .pull("llama3.2:3b", |progress| seen.push(progress))
            .await
            .expect("the stub should stream to the end");

        assert_eq!(seen.len(), 3, "every line should be reported: {seen:?}");
        assert_eq!(seen[0].status, "pulling manifest");
        assert_eq!(seen[1].fraction(), Some(0.4));
        assert!(seen[2].is_done());

        let requests = server.await.expect("the stub should finish");
        let (request_line, body) = split(&requests[0]);

        assert!(
            request_line.starts_with("POST /api/pull "),
            "unexpected request line: {request_line}"
        );
        assert!(
            body.contains(r#""model":"llama3.2:3b""#),
            "the model should be named: {body}"
        );
    }

    #[tokio::test]
    async fn surfaces_a_failure_reported_inside_the_pull_stream() {
        const STREAM: &str = concat!(
            r#"{"status":"pulling manifest"}"#,
            "\n",
            r#"{"error":"model 'nope' not found"}"#,
            "\n"
        );

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", STREAM)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .pull("nope", |_| {})
            .await
            .expect_err("an error in the stream should surface");

        match error {
            Error::Status { body, .. } => assert!(body.contains("not found"), "got {body}"),
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
