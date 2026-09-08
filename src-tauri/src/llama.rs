//! Client for the local llama.cpp server.
//!
//! Inference happens on the user's own machine, in a `llama-server` process
//! Chief ships and supervises itself (see [`crate::engine`]). [`Client`]
//! refuses to talk to anything but a loopback address, so a misconfiguration
//! cannot quietly turn into a hosted model seeing the user's work.
//!
//! `llama-server` speaks the OpenAI chat completions contract, which is what
//! this module models: messages carry `tool_calls` whose arguments are a JSON
//! *string*, streamed replies arrive as server-sent events, and sampling
//! limits ride in the request body while the context window is fixed when the
//! server starts.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The loopback port Chief's own engine listens on.
///
/// Deliberately not llama.cpp's default 8080: a developer running their own
/// `llama-server` should not collide with the one Chief starts, and vice versa.
pub const DEFAULT_PORT: u16 = 11435;

/// Give up if the engine has not accepted a connection by now. Short, because a
/// refused connection means it is not running and we want to say so quickly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the engine may go **silent** before we give up on it.
///
/// Deliberately an inactivity timeout rather than a deadline for the whole
/// answer. A total timeout cannot tell a hung engine from a slow one, so it
/// caps how *long* an answer may be: measured on a 2014 Mac mini, decode fell
/// to 1.76 tokens a second once the context passed 2,900 tokens, and a
/// five-minute ceiling cut a good answer off at 494 tokens with a transport
/// error where the text had been. An engine that has produced nothing for this
/// long is stuck; one still writing is left alone however long it takes.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(90);

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
    /// What the model said. A reply that is nothing but tool calls carries no
    /// content at all, which the OpenAI contract expresses as `null` — the same
    /// thing as an empty answer as far as anything here is concerned.
    #[serde(default, deserialize_with = "text_or_nothing")]
    pub content: String,
    /// Tools the model asked us to run. Populated by the model, never by us.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Which call a [`Role::Tool`] message is answering. The OpenAI contract
    /// matches results to requests by id rather than by name, so a model that
    /// asked for the same tool twice can tell the two answers apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    /// The result of running `call`, in the shape the model expects it back.
    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call.id.clone()),
        }
    }
}

/// The only value `type` ever takes on a tool or a tool call.
fn function_kind() -> &'static str {
    "function"
}

/// Read text content in the forms used by OpenAI-compatible servers.
///
/// Most replies use a string, while multimodal-capable templates may return
/// an array of `{ "type": "text", "text": "..." }` parts instead.
fn text_or_nothing<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    text_from_value(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn text_from_value(value: Value) -> Result<String, String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(text) => Ok(text),
        Value::Array(parts) => parts
            .into_iter()
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| "content parts must contain text".to_string())
            })
            .collect(),
        other => Err(format!("expected text content, got {other}")),
    }
}

/// Read tool arguments however they arrive.
///
/// OpenAI's contract — and llama.cpp's usual output — is a JSON *string*, but
/// some of its template paths hand back the object itself. Both mean the same
/// thing, and the string is the form that has to go back on the wire, so
/// anything that is not already one is re-encoded.
fn arguments_as_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Null => String::new(),
        Value::String(text) => text,
        object => object.to_string(),
    })
}

/// The same, for a field that may be absent altogether.
fn optional_arguments_as_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text),
        Some(object) => Some(object.to_string()),
    })
}

/// A request from the model to run one of our tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The identifier the result is returned under. Every server we speak to
    /// sends one, but a model template that forgets is not worth failing over,
    /// so a missing id becomes an empty string and is simply echoed back.
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "function_kind", skip_deserializing)]
    kind: &'static str,
    pub function: ToolCallFunction,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, function: ToolCallFunction) -> Self {
        Self {
            id: id.into(),
            kind: function_kind(),
            function,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// Arguments as the model wrote them.
    ///
    /// A *string* of JSON, not an object: that is the OpenAI contract
    /// `llama-server` implements, and it is what lets a streamed call arrive a
    /// few characters at a time. Read it with [`ToolCallFunction::arguments`].
    #[serde(default, deserialize_with = "arguments_as_text")]
    pub arguments: String,
}

impl ToolCallFunction {
    /// The arguments as JSON, or why they could not be read.
    ///
    /// A model with nothing to pass sends an empty string; that is a call with
    /// no arguments rather than a malformed one.
    pub fn arguments(&self) -> Result<Value, String> {
        if self.arguments.trim().is_empty() {
            return Ok(Value::Null);
        }

        serde_json::from_str(&self.arguments)
            .map_err(|error| format!("could not read the arguments: {error}"))
    }
}

/// A tool we advertise to the model.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    pub function: ToolFunction,
}

impl Tool {
    pub fn function(function: ToolFunction) -> Self {
        Self {
            kind: function_kind(),
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

/// How the model should be sampled, over and above the conversation itself.
///
/// The context window is not here: unlike Ollama, which loads a model per
/// context size on demand, `llama-server` is told once on the command line
/// (see [`crate::engine`]) and keeps that arena for its lifetime. Nothing a
/// request asks for can evict the weights.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Options {
    /// The most tokens one reply may run to. This is the ceiling on how long a
    /// question can take: a small model that starts rambling stops instead.
    pub max_tokens: u32,
    /// Low, because these answers are about what the tools actually returned.
    /// Invention is the failure mode here, not dullness.
    pub temperature: f32,
}

impl Options {
    /// The defaults every request starts from.
    pub const fn new() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.2,
        }
    }

    /// Cap the reply at `tokens`.
    #[must_use]
    pub const fn with_answer_length(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// The body posted to `/v1/chat/completions`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    /// Which model to answer with. `llama-server` serves exactly one, under the
    /// alias the engine gave it, so this identifies rather than chooses.
    pub model: String,
    pub messages: Vec<Message>,
    /// Whether to receive the reply a token at a time. Set by whichever of
    /// [`Client::chat`] and [`Client::chat_stream`] is doing the asking.
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// Sampling limits, which the OpenAI contract carries at the top level.
    #[serde(flatten)]
    pub options: Options,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: false,
            tools: Vec::new(),
            options: Options::new(),
        }
    }

    /// Advertise tools the model may call.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// Sample the model differently to the defaults.
    #[must_use]
    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }
}

/// A complete (non-streamed) reply.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub model: String,
    pub choices: Vec<Choice>,
}

impl ChatResponse {
    /// The answer itself. A reply with no choices is a server that has
    /// answered nothing, which is a decoding failure rather than an empty
    /// answer we should hand on as if it were one.
    pub fn message(self) -> Result<Message, Error> {
        self.choices
            .into_iter()
            .next()
            .map(|choice| choice.message)
            .ok_or_else(|| Error::Decode("the engine returned no choices".to_string()))
    }
}

/// What an answer cut short by the token ceiling is marked with.
const ANSWER_CUT: &str = "\n\n[Cut short — this answer reached its length limit.]";

/// What an answer the engine stopped delivering is marked with.
const ANSWER_INTERRUPTED: &str = "\n\n[Cut short — the model engine stopped responding.]";

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub message: Message,
    /// Why the model stopped. `length` means it hit the ceiling rather than
    /// finishing, which is the difference between an answer and half of one.
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// One server-sent event of a streamed reply.
#[derive(Debug, Clone, Default, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    /// `llama-server` can report a failure part way through an otherwise
    /// successful response, so this has to be watched for rather than assumed
    /// away.
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// The few more characters of an answer that one event carries.
#[derive(Debug, Clone, Default, Deserialize)]
struct Delta {
    #[serde(default, deserialize_with = "text_or_nothing")]
    content: String,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

/// Part of a tool call. The OpenAI contract streams these in fragments: the
/// name arrives once and the arguments accumulate a few characters at a time,
/// with `index` saying which call each fragment belongs to.
#[derive(Debug, Clone, Default, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: FunctionDelta,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "optional_arguments_as_text")]
    arguments: Option<String>,
}

/// Tool calls being assembled out of a stream.
#[derive(Debug, Default)]
struct PartialToolCalls(Vec<ToolCall>);

impl PartialToolCalls {
    /// Fold one fragment in, growing the list if this is a call we have not
    /// seen before. A server that sends a whole call in one event and one that
    /// dribbles it out both end up here.
    fn absorb(&mut self, delta: ToolCallDelta) {
        if self.0.len() <= delta.index {
            self.0.resize_with(delta.index + 1, || {
                ToolCall::new(
                    String::new(),
                    ToolCallFunction {
                        name: String::new(),
                        arguments: String::new(),
                    },
                )
            });
        }

        let call = &mut self.0[delta.index];

        if let Some(id) = delta.id {
            call.id = id;
        }
        if let Some(name) = delta.function.name {
            call.function.name = name;
        }
        if let Some(arguments) = delta.function.arguments {
            call.function.arguments.push_str(&arguments);
        }
    }

    /// The finished calls, minus any slot the stream never filled in.
    fn finish(self) -> Vec<ToolCall> {
        self.0
            .into_iter()
            .filter(|call| !call.function.name.is_empty())
            .collect()
    }
}

/// Whether the engine can answer a question right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    /// Answering.
    Ready,
    /// Running, but still reading the weights into memory.
    Loading,
    /// Not running, or not reachable.
    Down,
}

/// What can go wrong talking to a local model.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("'{0}' is not a valid URL")]
    InvalidUrl(String),
    #[error("refusing to send your work to '{0}' — inference must stay on this machine")]
    NotLoopback(String),
    #[error("Chief's local model engine is not running at {base_url}.")]
    Unreachable { base_url: String },
    #[error("the model is still loading. Give it a few seconds and ask again.")]
    Loading,
    #[error("the model engine stopped responding part-way through the answer.")]
    Timeout,
    /// The answer stopped arriving, but not because the engine went quiet: the
    /// connection itself went away. Its own variant because the two want
    /// different words, and because what had already been written is worth
    /// keeping either way.
    #[error("the answer was cut short: the connection to the model engine ended.")]
    Interrupted,
    #[error("the model engine returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("could not read the model engine's response: {0}")]
    Decode(String),
    #[error("request to the model engine failed: {0}")]
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

/// An HTTP client bound to a local `llama-server`.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: reqwest::Url,
}

impl Client {
    /// Connect to the engine on its default local port.
    pub fn new() -> Result<Self, Error> {
        Self::with_base_url(&format!("http://127.0.0.1:{DEFAULT_PORT}"))
    }

    /// Connect to a `llama-server` at `base_url`, which must be loopback.
    pub fn with_base_url(base_url: &str) -> Result<Self, Error> {
        let base_url =
            reqwest::Url::parse(base_url).map_err(|_| Error::InvalidUrl(base_url.to_string()))?;

        if !is_loopback(&base_url) {
            return Err(Error::NotLoopback(base_url.to_string()));
        }

        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // `read_timeout` and not `timeout`: the latter is a deadline for
            // the whole exchange, body included, so on a streamed answer it is
            // a limit on how much the model may say rather than on how long it
            // may stall. This one restarts with every byte that arrives.
            .read_timeout(SILENCE_TIMEOUT)
            // Nothing here should ever leave the machine, so a proxy would be
            // both useless and a way for data to escape.
            .no_proxy()
            .build()
            .map_err(|error| Error::Transport(error.to_string()))?;

        Ok(Self { http, base_url })
    }

    /// Where this client is pointed, for anything that needs to say so.
    pub fn base_url(&self) -> &str {
        self.base_url.as_str()
    }

    /// Ask the model to continue a conversation, waiting for the whole reply.
    ///
    /// Use this where nobody is watching the answer appear — the background
    /// summariser. When a person is waiting, prefer [`Client::chat_stream`].
    pub async fn chat(&self, request: &ChatRequest) -> Result<Message, Error> {
        let response = self.ask(request, false).await?;
        let body = response
            .text()
            .await
            .map_err(|error| Error::Decode(error.to_string()))?;

        serde_json::from_str::<ChatResponse>(&body)
            .map_err(|error| Error::Decode(format!("{error}; response body: {body}")))?
            .message()
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
        let mut calls = PartialToolCalls::default();
        let mut ran_out = false;

        let delivered = read_events(response, |data| {
            let chunk: ChatChunk = serde_json::from_str(data)
                .map_err(|error| Error::Decode(format!("{error}; event data: {data}")))?;

            if let Some(problem) = chunk.error {
                return Err(Error::Status {
                    status: 200,
                    body: describe_error(&problem),
                });
            }

            for choice in chunk.choices {
                if !choice.delta.content.is_empty() {
                    let part = choice.delta.content;
                    on_token(&part);
                    answer.content.push_str(&part);
                }

                for fragment in choice.delta.tool_calls {
                    calls.absorb(fragment);
                }

                // "length" means the model stopped because it reached the
                // ceiling, not because it had finished.
                if choice.finish_reason.as_deref() == Some("length") {
                    ran_out = true;
                }
            }

            Ok(())
        })
        .await;

        answer.tool_calls = calls.finish();

        // An answer that stopped arriving is not the same as no answer.
        //
        // Measured in the product: a five-minute deadline killed a reply the
        // reader had already watched arrive, and the whole thing was replaced
        // by `error decoding response body` — hundreds of words of real answer
        // discarded in favour of a transport error. Whatever did arrive is kept
        // and marked, on the same principle as the length ceiling above.
        //
        // Only prose is salvageable, and only when the delivery failed rather
        // than the request. A tool call that stopped mid-way is a truncated
        // JSON argument list, and running it would be worse than failing; an
        // error the engine itself reported in the stream is it telling us the
        // answer is void, so passing off what arrived before it as an answer
        // would be inventing one. Both still raise.
        if let Err(interrupted) = delivered {
            let delivery_failed = matches!(interrupted, Error::Interrupted | Error::Timeout);

            if !delivery_failed || answer.content.is_empty() || !answer.tool_calls.is_empty() {
                return Err(interrupted);
            }

            on_token(ANSWER_INTERRUPTED);
            answer.content.push_str(ANSWER_INTERRUPTED);

            return Ok(answer);
        }

        // An answer that stopped mid-sentence because it hit the ceiling used
        // to just stop — measured in the product, a list of pull requests ended
        // halfway through a URL with nothing to say why. The reader cannot tell
        // that from a model that finished, so it is said, and said in the
        // stream so it arrives where the answer stopped.
        if ran_out && answer.tool_calls.is_empty() {
            on_token(ANSWER_CUT);
            answer.content.push_str(ANSWER_CUT);
        }

        Ok(answer)
    }

    /// Whether the engine is up, still loading, or not there at all.
    ///
    /// `llama-server` answers `/health` with 503 while it reads the weights,
    /// which is a state worth telling the user about rather than reporting as
    /// a failure.
    pub async fn health(&self) -> Health {
        let Ok(url) = self.endpoint("/health") else {
            return Health::Down;
        };

        match self.http.get(url).send().await {
            Ok(response) if response.status().is_success() => Health::Ready,
            Ok(response) if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
                Health::Loading
            }
            Ok(_) | Err(_) => Health::Down,
        }
    }

    /// Post a chat request and hand back the response once it is known to be
    /// good, so both the buffered and the streamed path fail the same way.
    async fn ask(&self, request: &ChatRequest, stream: bool) -> Result<reqwest::Response, Error> {
        let url = self.endpoint("/v1/chat/completions")?;
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

            return Err(if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                Error::Loading
            } else {
                Error::Status {
                    status: status.as_u16(),
                    body,
                }
            });
        }

        Ok(response)
    }

    fn endpoint(&self, path: &str) -> Result<reqwest::Url, Error> {
        self.base_url
            .join(path)
            .map_err(|_| Error::InvalidUrl(self.base_url.to_string()))
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

/// Pull the human half out of an error object, which arrives either as a bare
/// string or as OpenAI's `{ "message": ..., "type": ... }`.
fn describe_error(problem: &Value) -> String {
    problem
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| problem.to_string())
}

/// Read a server-sent event stream, handing the data of each event to
/// `on_event` until the server says `[DONE]`.
///
/// A chunk off the wire can end mid-line or, worse, mid-character, so bytes are
/// buffered and only decoded once a whole line is in hand — otherwise an
/// accented word arrives as a replacement character.
async fn read_events<F>(response: reqwest::Response, mut on_event: F) -> Result<(), Error>
where
    F: FnMut(&str) -> Result<(), Error>,
{
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut pending: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        // Not `Transport(error.to_string())`. A body that stops arriving used
        // to reach the user as the raw string `error decoding response body`,
        // which says nothing about what happened and is exactly the sort of
        // transport wording this module exists to keep off the screen.
        let chunk = chunk.map_err(|error| {
            if error.is_timeout() {
                Error::Timeout
            } else {
                Error::Interrupted
            }
        })?;
        pending.extend_from_slice(&chunk);

        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            if deliver(&line, &mut on_event)?.is_break() {
                return Ok(());
            }
        }
    }

    // A last line the server did not terminate.
    let _ = deliver(&pending, &mut on_event)?;

    Ok(())
}

/// Hand one event's data over, unless the line carries none.
///
/// Server-sent events separate records with blank lines and may carry comment
/// or field lines we have no use for, so anything that is not `data:` is
/// skipped. `[DONE]` ends the stream.
fn deliver<F>(line: &[u8], on_event: &mut F) -> Result<std::ops::ControlFlow<()>, Error>
where
    F: FnMut(&str) -> Result<(), Error>,
{
    let line = String::from_utf8_lossy(line);

    let Some(data) = line.trim().strip_prefix("data:") else {
        return Ok(std::ops::ControlFlow::Continue(()));
    };

    let data = data.trim();

    if data == "[DONE]" {
        return Ok(std::ops::ControlFlow::Break(()));
    }

    if data.is_empty() {
        return Ok(std::ops::ControlFlow::Continue(()));
    }

    on_event(data)?;

    Ok(std::ops::ControlFlow::Continue(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_loopback_addresses() {
        for url in [
            "http://localhost:11435",
            "http://127.0.0.1:11435",
            "http://[::1]:11435",
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
            "http://192.168.1.10:11435",
            "http://llama.example.com",
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
    fn bounds_how_long_an_answer_can_take() {
        let request = ChatRequest::new("chief", vec![Message::user("hello")]);
        let body = serde_json::to_value(&request).expect("should serialize");

        assert_eq!(body["max_tokens"], json!(512));
        assert_eq!(
            body["temperature"].as_f64().expect("should be a number") as f32,
            0.2
        );
    }

    #[test]
    fn does_not_ask_the_engine_for_a_context_size() {
        // The context window is a launch flag, not a per-request setting: the
        // server holds one arena for its lifetime and nothing a request says
        // can make it reload the weights.
        let request = ChatRequest::new("chief", vec![Message::user("hello")]);
        let body = serde_json::to_value(&request).expect("should serialize");

        assert!(body.get("num_ctx").is_none());
        assert!(body.get("n_ctx").is_none());
    }

    #[test]
    fn a_shorter_answer_only_changes_the_answer_length() {
        let brief = Options::new().with_answer_length(80);

        assert_eq!(brief.max_tokens, 80);
        assert_eq!(brief.temperature, Options::new().temperature);
    }

    #[test]
    fn omits_the_tools_array_when_there_is_nothing_to_offer() {
        let request = ChatRequest::new("chief", vec![Message::user("hello")]);
        let body = serde_json::to_value(&request).expect("should serialize");

        assert_eq!(body["stream"], json!(false));
        assert!(body.get("tools").is_none(), "empty tools should be omitted");
    }

    #[test]
    fn serializes_tools_in_the_openai_function_format() {
        let request =
            ChatRequest::new("chief", vec![Message::user("what is open?")]).with_tools(vec![
                Tool::function(ToolFunction {
                    name: "fetch_github_prs".to_string(),
                    description: "List the user's open pull requests".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": { "state": { "type": "string" } },
                        "required": ["state"],
                    }),
                }),
            ]);

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
    fn returns_a_tool_result_under_the_id_that_asked_for_it() {
        let call = ToolCall::new(
            "call_7",
            ToolCallFunction {
                name: "fetch_github_prs".to_string(),
                arguments: String::new(),
            },
        );

        let message = Message::tool_result(&call, r#"{"pullRequests":[]}"#);
        let body = serde_json::to_value(&message).expect("should serialize");

        assert_eq!(body["role"], json!("tool"));
        assert_eq!(body["tool_call_id"], json!("call_7"));
        assert_eq!(body["content"], json!(r#"{"pullRequests":[]}"#));
    }

    #[test]
    fn reads_arguments_that_arrived_as_a_json_string() {
        let function = ToolCallFunction {
            name: "fetch_github_prs".to_string(),
            arguments: r#"{"state":"open"}"#.to_string(),
        };

        let arguments = function.arguments().expect("should parse");

        assert_eq!(arguments["state"], json!("open"));
    }

    #[test]
    fn treats_empty_arguments_as_a_call_with_none() {
        let function = ToolCallFunction {
            name: "fetch_github_prs".to_string(),
            arguments: "  ".to_string(),
        };

        assert_eq!(function.arguments(), Ok(Value::Null));
    }

    #[test]
    fn explains_arguments_that_are_not_json() {
        let function = ToolCallFunction {
            name: "fetch_github_prs".to_string(),
            arguments: "{state: open".to_string(),
        };

        assert!(function.arguments().is_err());
    }

    #[test]
    fn reads_a_reply_that_asks_for_a_tool() {
        let raw = json!({
            "model": "chief",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "fetch_github_prs",
                            "arguments": "{\"state\":\"open\"}",
                        },
                    }],
                },
                "finish_reason": "tool_calls",
            }],
        });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");
        let message = response.message().expect("should have a choice");
        let call = &message.tool_calls[0];

        assert_eq!(call.id, "call_1");
        assert_eq!(call.function.name, "fetch_github_prs");
        assert_eq!(call.function.arguments().expect("valid")["state"], "open");
    }

    #[test]
    fn reads_a_reply_whose_only_content_is_tool_calls() {
        // A model that says nothing before calling a tool sends `null`, not "".
        let raw = json!({
            "model": "chief",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "fetch_github_prs", "arguments": "{}" },
                    }],
                },
                "finish_reason": "tool_calls",
            }],
        });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");
        let message = response.message().expect("should have a choice");

        assert_eq!(message.content, "");
        assert_eq!(message.tool_calls.len(), 1);
    }

    #[test]
    fn reads_arguments_that_arrived_as_an_object() {
        // Some of llama.cpp's template paths hand back the object rather than
        // the string the contract asks for.
        let raw = json!({
            "id": "call_1",
            "type": "function",
            "function": { "name": "fetch_github_prs", "arguments": { "state": "open" } },
        });

        let call: ToolCall = serde_json::from_value(raw).expect("should deserialize");

        assert_eq!(call.function.arguments, r#"{"state":"open"}"#);
        assert_eq!(call.function.arguments().expect("valid")["state"], "open");
    }

    #[test]
    fn reads_a_plain_text_reply() {
        let raw = json!({
            "model": "chief",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "You merged two pull requests." },
                "finish_reason": "stop",
            }],
        });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");
        let message = response.message().expect("should have a choice");

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, "You merged two pull requests.");
        assert!(message.tool_calls.is_empty());
    }

    #[test]
    fn reads_text_parts_in_a_reply() {
        let raw = json!({
            "model": "chief",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "Hello" },
                        { "type": "text", "text": " there" },
                    ],
                },
            }],
        });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");
        let message = response.message().expect("should have a choice");

        assert_eq!(message.content, "Hello there");
    }

    #[test]
    fn a_reply_with_no_choices_is_a_failure_not_an_empty_answer() {
        let raw = json!({ "model": "chief", "choices": [] });

        let response: ChatResponse = serde_json::from_value(raw).expect("should deserialize");

        assert!(matches!(response.message(), Err(Error::Decode(_))));
    }

    #[test]
    fn assembles_a_tool_call_out_of_fragments() {
        let mut calls = PartialToolCalls::default();

        for fragment in [
            json!({ "index": 0, "id": "call_1", "function": { "name": "fetch_github_prs", "arguments": "" } }),
            json!({ "index": 0, "function": { "arguments": "{\"state\"" } }),
            json!({ "index": 0, "function": { "arguments": ":\"open\"}" } }),
        ] {
            calls.absorb(serde_json::from_value(fragment).expect("should deserialize"));
        }

        let finished = calls.finish();

        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].id, "call_1");
        assert_eq!(finished[0].function.name, "fetch_github_prs");
        assert_eq!(finished[0].function.arguments, r#"{"state":"open"}"#);
    }

    #[test]
    fn keeps_two_calls_in_the_same_stream_apart() {
        let mut calls = PartialToolCalls::default();

        for fragment in [
            json!({ "index": 0, "id": "a", "function": { "name": "one", "arguments": "{}" } }),
            json!({ "index": 1, "id": "b", "function": { "name": "two", "arguments": "{" } }),
            json!({ "index": 1, "function": { "arguments": "}" } }),
        ] {
            calls.absorb(serde_json::from_value(fragment).expect("should deserialize"));
        }

        let finished = calls.finish();

        assert_eq!(finished.len(), 2);
        assert_eq!(finished[0].function.name, "one");
        assert_eq!(finished[1].function.name, "two");
        assert_eq!(finished[1].function.arguments, "{}");
    }
}

/// A throwaway HTTP server on loopback, so the requests [`Client`] actually
/// puts on the wire are covered without a running `llama-server`.
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

    /// One complete chat reply, as `llama-server` writes it.
    pub fn answer(content: &str) -> String {
        serde_json::json!({
            "model": "chief",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop",
            }],
        })
        .to_string()
    }

    /// A server-sent event stream, in the shape `llama-server` writes it.
    pub fn events(chunks: &[serde_json::Value]) -> String {
        let mut stream = String::new();

        for chunk in chunks {
            stream.push_str(&format!("data: {chunk}\n\n"));
        }

        stream.push_str("data: [DONE]\n\n");
        stream
    }

    /// One streamed delta of an answer.
    pub fn delta(content: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [{ "index": 0, "delta": { "content": content } }],
        })
    }

    /// Serve `replies` in order, one per request, then hand back everything
    /// the client sent. Bodies may be borrowed or owned, so a test can build
    /// one up rather than having to write it as a literal.
    pub fn serve<B: Into<String>>(
        replies: Vec<(&'static str, B)>,
    ) -> (String, JoinHandle<Vec<String>>) {
        let (host, listener) = reserve();

        (host, serve_on(listener, replies))
    }

    /// Take a port and say where it is, without answering anything yet.
    ///
    /// For the tests whose replies have to *name* the server — a discovery
    /// document pointing at its own authorization server, a redirect to
    /// itself. Those cannot build a body before they know the port, and a
    /// bind-then-drop to learn one is a race with whatever binds next.
    pub fn reserve() -> (String, std::net::TcpListener) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("should bind loopback");
        listener
            .set_nonblocking(true)
            .expect("should be non-blocking");
        let port = listener
            .local_addr()
            .expect("socket should have an address")
            .port();

        (format!("http://127.0.0.1:{port}"), listener)
    }

    /// Answer `replies` on a port [`reserve`] already took.
    pub fn serve_on<B: Into<String>>(
        listener: std::net::TcpListener,
        replies: Vec<(&'static str, B)>,
    ) -> JoinHandle<Vec<String>> {
        let replies: Vec<(&'static str, String)> = replies
            .into_iter()
            .map(|(status_line, body)| (status_line, body.into()))
            .collect();

        tokio::spawn(async move {
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
        })
    }

    /// A server that promises more body than it sends, and then hangs up.
    ///
    /// This is what an answer interrupted part-way through looks like from the
    /// client: some of it arrived, the rest never will, and the connection is
    /// gone. Real causes are the engine being killed, the machine sleeping, or
    /// a read timeout firing mid-stream.
    pub fn serve_truncated(body: &'static str, short_by: usize) -> (String, JoinHandle<()>) {
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
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len() + short_by
            );

            socket
                .write_all(response.as_bytes())
                .await
                .expect("should write a response");
            socket.flush().await.expect("should flush the response");
            drop(socket);
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
    use super::test_support::{answer, closed_port, delta, events, serve, serve_truncated, split};
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn posts_the_conversation_to_the_openai_chat_endpoint() {
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", answer("Two pull requests."))]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let message = client
            .chat(&ChatRequest::new(
                "chief",
                vec![
                    Message::system("You are Chief."),
                    Message::user("What did I ship?"),
                ],
            ))
            .await
            .expect("the stub should answer");

        assert_eq!(message.content, "Two pull requests.");

        let requests = server.await.expect("the stub should finish");
        let (request_line, body) = split(&requests[0]);

        assert!(
            request_line.starts_with("POST /v1/chat/completions "),
            "unexpected request line: {request_line}"
        );

        let sent: serde_json::Value = serde_json::from_str(body).expect("body should be JSON");
        assert_eq!(sent["model"], json!("chief"));
        assert_eq!(sent["stream"], json!(false));
        assert_eq!(sent["messages"][0]["role"], json!("system"));
        assert_eq!(sent["messages"][1]["content"], json!("What did I ship?"));
    }

    /// The failure this was written for: a five-minute deadline killed a reply
    /// the reader had already watched arrive, and the screen replaced hundreds
    /// of words of real answer with `error decoding response body`.
    #[tokio::test]
    async fn keeps_an_answer_the_engine_stopped_delivering() {
        const STREAM: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Two \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"pull requests\"}}]}\n\n",
        );

        let (base_url, server) = serve_truncated(STREAM, 512);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut shown = String::new();
        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("What is waiting on me?")]),
                |token| shown.push_str(token),
            )
            .await
            .expect("what did arrive should be kept, not thrown away");

        assert!(
            reply.content.starts_with("Two pull requests"),
            "the words that arrived should survive: {:?}",
            reply.content
        );
        assert!(
            reply.content.contains("stopped responding"),
            "and the reader should be told it stopped early: {:?}",
            reply.content
        );
        assert_eq!(
            shown, reply.content,
            "the note belongs in the stream too, where the answer stopped"
        );

        server.await.expect("the stub should finish");
    }

    /// Salvage is for an answer, not for nothing at all.
    #[tokio::test]
    async fn still_fails_when_the_engine_delivered_nothing_before_it_stopped() {
        let (base_url, server) = serve_truncated("", 512);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let outcome = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("What is waiting on me?")]),
                |_| {},
            )
            .await;

        assert!(
            matches!(outcome, Err(Error::Interrupted | Error::Timeout)),
            "an empty interrupted stream is a failure, got {outcome:?}"
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn hands_over_the_answer_as_it_is_written() {
        let stream = events(&[delta("Two "), delta("pull requests."), delta("")]);
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut tokens = Vec::new();
        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("What did I ship?")]),
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
            reply.content, "Two pull requests.",
            "and the whole answer should still be returned"
        );

        let requests = server.await.expect("the stub should finish");
        let (request_line, body) = split(&requests[0]);

        assert!(
            request_line.starts_with("POST /v1/chat/completions "),
            "unexpected request line: {request_line}"
        );

        let sent: serde_json::Value = serde_json::from_str(body).expect("body should be JSON");
        assert_eq!(sent["stream"], json!(true));
    }

    #[tokio::test]
    async fn stops_reading_once_the_stream_says_it_is_done() {
        // Anything after `[DONE]` is not part of the answer, and a server that
        // holds the socket open afterwards should not hold up the reply.
        let stream = format!(
            "{}data: {}\n\n",
            events(&[delta("Done.")]),
            delta("ignored")
        );
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("hi")]),
                |_| {},
            )
            .await
            .expect("the stub should stream to the end");

        assert_eq!(reply.content, "Done.");
        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn says_when_an_answer_was_cut_short_by_the_length_limit() {
        // Measured in the product: a list of pull requests stopped halfway
        // through a URL at exactly max_tokens, and nothing said why. A reader
        // cannot tell that from a model that finished.
        let stream = events(&[
            delta("A very long answer that runs"),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }],
            }),
        ]);
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut streamed = String::new();
        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("hi")]),
                |token| streamed.push_str(token),
            )
            .await
            .expect("the stub should stream");

        assert!(reply.content.contains("Cut short"), "{}", reply.content);
        assert!(
            streamed.contains("Cut short"),
            "the note has to arrive in the stream too, where the answer stopped"
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn an_answer_that_finished_is_not_marked() {
        let stream = events(&[
            delta("All done."),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            }),
        ]);
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("hi")]),
                |_| {},
            )
            .await
            .expect("the stub should stream");

        assert_eq!(reply.content, "All done.");
        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn reads_a_final_event_the_server_did_not_terminate() {
        let stream = format!("data: {}\n\ndata: {}", delta("Do"), delta("ne."));
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("hi")]),
                |_| {},
            )
            .await
            .expect("a final line without a newline should still be read");

        assert_eq!(reply.content, "Done.");
        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn collects_the_tool_calls_out_of_a_stream() {
        let stream = events(&[
            json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "fetch_github_prs", "arguments": "" },
                        }],
                    },
                }],
            }),
            json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": { "arguments": "{\"state\":\"open\"}" },
                        }],
                    },
                }],
            }),
        ]);

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let mut tokens = Vec::new();
        let reply = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("What is open?")]),
                |token| tokens.push(token.to_string()),
            )
            .await
            .expect("the stub should stream to the end");

        assert!(tokens.is_empty(), "a tool call is not something to show");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].function.name, "fetch_github_prs");
        assert_eq!(
            reply.tool_calls[0].function.arguments,
            r#"{"state":"open"}"#
        );

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn surfaces_a_failure_reported_inside_the_answer_stream() {
        let stream = format!(
            "data: {}\n\ndata: {}\n\n",
            delta("Tw"),
            json!({ "error": { "message": "the model runner has stopped", "code": 500 } })
        );

        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", stream)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat_stream(
                &ChatRequest::new("chief", vec![Message::user("hi")]),
                |_| {},
            )
            .await
            .expect_err("an error in the stream should surface");

        match error {
            Error::Status { body, .. } => {
                assert_eq!(body, "the model runner has stopped")
            }
            other => panic!("expected Status, got {other:?}"),
        }

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn says_when_the_model_is_still_loading() {
        let (base_url, server) = serve(vec![(
            "HTTP/1.1 503 Service Unavailable",
            r#"{"error":{"message":"Loading model","code":503}}"#,
        )]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat(&ChatRequest::new("chief", vec![Message::user("hi")]))
            .await
            .expect_err("a 503 should be an error");

        assert!(matches!(error, Error::Loading), "got {error:?}");

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn surfaces_an_unexpected_status() {
        let (base_url, server) = serve(vec![("HTTP/1.1 500 Internal Server Error", "boom")]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        let error = client
            .chat(&ChatRequest::new("chief", vec![Message::user("hi")]))
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
    async fn reports_the_engine_as_ready_when_health_is_good() {
        let (base_url, server) = serve(vec![("HTTP/1.1 200 OK", r#"{"status":"ok"}"#)]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        assert_eq!(client.health().await, Health::Ready);

        let requests = server.await.expect("the stub should finish");
        let (request_line, _) = split(&requests[0]);
        assert!(
            request_line.starts_with("GET /health "),
            "unexpected request line: {request_line}"
        );
    }

    #[tokio::test]
    async fn reports_the_engine_as_loading_while_it_reads_the_weights() {
        let (base_url, server) = serve(vec![(
            "HTTP/1.1 503 Service Unavailable",
            r#"{"error":{"message":"Loading model"}}"#,
        )]);
        let client = Client::with_base_url(&base_url).expect("loopback should be allowed");

        assert_eq!(client.health().await, Health::Loading);

        server.await.expect("the stub should finish");
    }

    #[tokio::test]
    async fn reports_the_engine_as_down_when_nothing_is_listening() {
        let client = Client::with_base_url(&closed_port()).expect("loopback should be allowed");

        assert_eq!(client.health().await, Health::Down);
    }

    #[tokio::test]
    async fn says_when_the_engine_is_not_running() {
        let client = Client::with_base_url(&closed_port()).expect("loopback should be allowed");

        let error = client
            .chat(&ChatRequest::new("chief", vec![Message::user("hi")]))
            .await
            .expect_err("a closed port should be an error");

        assert!(
            matches!(error, Error::Unreachable { .. }),
            "expected Unreachable, got {error:?}"
        );
    }
}
