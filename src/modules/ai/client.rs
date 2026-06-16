use crate::event::AppEvent;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SYSTEM_PROMPT: &str = "You are an expert Bitcoin developer helping analyze PSBTs and multisig wallets. \
     Be concise and precise.";

/// Build the shared `reqwest::Client` used for all AI requests.
/// A single client reuses its connection pool/TLS session across requests
/// and enforces a request timeout so a dead network can't hang `ai_loading`
/// forever.
pub fn build_client() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client config is static and valid")
}

#[derive(Debug, Error)]
pub enum AiError {
    #[error("No API key — set PSBT_INSPECTOR_API_KEY in your .env file or shell environment")]
    NoApiKey,
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Unexpected response: {0}")]
    ParseResponse(String),
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    stream: bool,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

// ── Streaming event types ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<StreamDelta>,
    error: Option<StreamError>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct StreamError {
    message: String,
}

/// Result of parsing one SSE line.
#[derive(Debug, PartialEq)]
enum SseLine {
    /// A `content_block_delta` text chunk to forward to the UI.
    Text(String),
    /// `message_stop` — the response is complete.
    Done,
    /// An `error` event from the API.
    Error(String),
    /// Any other event (`message_start`, `ping`, etc.) — ignored.
    Other,
}

/// Parse one line of an SSE stream. Non-`data:` lines (blank lines, `event:`
/// lines) are `Other`.
fn parse_sse_line(line: &str) -> SseLine {
    let Some(data) = line.strip_prefix("data:") else {
        return SseLine::Other;
    };
    let data = data.trim();
    let Ok(event) = serde_json::from_str::<StreamEvent>(data) else {
        return SseLine::Other;
    };
    match event.event_type.as_str() {
        "content_block_delta" => match event.delta {
            Some(StreamDelta {
                delta_type: Some(ref t),
                text: Some(text),
            }) if t == "text_delta" => SseLine::Text(text),
            _ => SseLine::Other,
        },
        "message_stop" => SseLine::Done,
        "error" => SseLine::Error(
            event
                .error
                .map(|e| e.message)
                .unwrap_or_else(|| "unknown error".into()),
        ),
        _ => SseLine::Other,
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Call the Anthropic Messages API with streaming enabled, forwarding each
/// text chunk as `AppEvent::AiChunk(generation, _)` as it arrives. The caller
/// is responsible for sending `AiDone`/`AiError` based on the returned
/// `Result` — this function itself never sends those.
pub async fn ask(
    client: &Client,
    api_key: &str,
    model: &str,
    context: &str,
    question: &str,
    tx: &mpsc::UnboundedSender<AppEvent>,
    generation: u64,
) -> Result<(), AiError> {
    if api_key.trim().is_empty() {
        return Err(AiError::NoApiKey);
    }

    let user_content = build_user_content(context, question);

    let request_body = MessagesRequest {
        model,
        max_tokens: MAX_TOKENS,
        system: SYSTEM_PROMPT,
        stream: true,
        messages: vec![Message {
            role: "user",
            content: &user_content,
        }],
    };

    let response = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&request_body)
        .send()
        .await?
        .error_for_status()?;

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..pos]).trim_end().to_string();
            buf.drain(..=pos);
            match parse_sse_line(&line) {
                SseLine::Text(text) => {
                    let _ = tx.send(AppEvent::AiChunk(generation, text));
                }
                SseLine::Done => return Ok(()),
                SseLine::Error(msg) => return Err(AiError::ParseResponse(msg)),
                SseLine::Other => {}
            }
        }
    }
    Ok(())
}

fn build_user_content(context: &str, question: &str) -> String {
    format!("{}\n\nQuestion: {}", context, question)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_api_key_returns_no_api_key_error() {
        let client = build_client();
        let (tx, _rx) = mpsc::unbounded_channel();
        let err = ask(&client, "", "model", "ctx", "q", &tx, 1)
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::NoApiKey));
    }

    #[tokio::test]
    async fn whitespace_api_key_returns_no_api_key_error() {
        let client = build_client();
        let (tx, _rx) = mpsc::unbounded_channel();
        let err = ask(&client, "   ", "model", "ctx", "q", &tx, 1)
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::NoApiKey));
    }

    #[test]
    fn request_body_serialises_model_and_tokens() {
        let req = MessagesRequest {
            model: "claude-sonnet-4-5",
            max_tokens: MAX_TOKENS,
            system: SYSTEM_PROMPT,
            stream: true,
            messages: vec![Message {
                role: "user",
                content: "test",
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("claude-sonnet-4-5"), "json: {json}");
        assert!(json.contains("max_tokens"), "json: {json}");
        assert!(json.contains("\"stream\":true"), "json: {json}");
    }

    #[test]
    fn user_content_combines_context_and_question() {
        let content = build_user_content("some context", "my question");
        assert!(content.contains("some context"));
        assert!(content.contains("Question: my question"));
    }

    #[test]
    fn user_content_places_question_after_context() {
        let content = build_user_content("ctx", "q_marker");
        let ctx_pos = content.find("ctx").unwrap();
        let q_pos = content.find("q_marker").unwrap();
        assert!(q_pos > ctx_pos);
    }

    #[test]
    fn sse_content_block_delta_yields_text() {
        let line =
            r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(parse_sse_line(line), SseLine::Text("Hello".into()));
    }

    #[test]
    fn sse_message_stop_yields_done() {
        let line = r#"data: {"type":"message_stop"}"#;
        assert_eq!(parse_sse_line(line), SseLine::Done);
    }

    #[test]
    fn sse_error_event_yields_error() {
        let line = r#"data: {"type":"error","error":{"message":"overloaded"}}"#;
        assert_eq!(parse_sse_line(line), SseLine::Error("overloaded".into()));
    }

    #[test]
    fn sse_ping_yields_other() {
        let line = r#"data: {"type":"ping"}"#;
        assert_eq!(parse_sse_line(line), SseLine::Other);
    }

    #[test]
    fn sse_non_data_line_yields_other() {
        assert_eq!(parse_sse_line("event: content_block_delta"), SseLine::Other);
        assert_eq!(parse_sse_line(""), SseLine::Other);
    }

    #[test]
    fn sse_content_block_delta_without_text_delta_yields_other() {
        let line = r#"data: {"type":"content_block_delta","delta":{"type":"input_json_delta","text":null}}"#;
        assert_eq!(parse_sse_line(line), SseLine::Other);
    }
}
