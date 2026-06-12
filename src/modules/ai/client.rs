use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;
const SYSTEM_PROMPT: &str = "You are an expert Bitcoin developer helping analyze PSBTs and multisig wallets. \
     Be concise and precise.";

#[derive(Debug, Error)]
pub enum AiError {
    #[error(
        "No API key — set api_key in ~/.config/psbt-inspector/config.toml \
         or PSBT_INSPECTOR_API_KEY env var"
    )]
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
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Call the Anthropic Messages API and return the full response text.
/// Caller sends the result as a single AiChunk followed by AiDone.
pub async fn ask(
    api_key: &str,
    model: &str,
    context: &str,
    question: &str,
) -> Result<String, AiError> {
    if api_key.trim().is_empty() {
        return Err(AiError::NoApiKey);
    }

    let user_content = build_user_content(context, question);

    let request_body = MessagesRequest {
        model,
        max_tokens: MAX_TOKENS,
        system: SYSTEM_PROMPT,
        messages: vec![Message {
            role: "user",
            content: &user_content,
        }],
    };

    let client = Client::new();
    let response = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&request_body)
        .send()
        .await?
        .error_for_status()?;

    let resp: MessagesResponse = response.json().await?;
    extract_text(resp).ok_or_else(|| AiError::ParseResponse("empty response body".into()))
}

fn build_user_content(context: &str, question: &str) -> String {
    format!("{}\n\nQuestion: {}", context, question)
}

fn extract_text(response: MessagesResponse) -> Option<String> {
    let text: String = response
        .content
        .into_iter()
        .filter(|b| b.block_type == "text")
        .filter_map(|b| b.text)
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() { None } else { Some(text) }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_api_key_returns_no_api_key_error() {
        let err = ask("", "model", "ctx", "q").await.unwrap_err();
        assert!(matches!(err, AiError::NoApiKey));
    }

    #[tokio::test]
    async fn whitespace_api_key_returns_no_api_key_error() {
        let err = ask("   ", "model", "ctx", "q").await.unwrap_err();
        assert!(matches!(err, AiError::NoApiKey));
    }

    #[test]
    fn request_body_serialises_model_and_tokens() {
        let req = MessagesRequest {
            model: "claude-sonnet-4-5",
            max_tokens: MAX_TOKENS,
            system: SYSTEM_PROMPT,
            messages: vec![Message {
                role: "user",
                content: "test",
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("claude-sonnet-4-5"), "json: {json}");
        assert!(json.contains("max_tokens"), "json: {json}");
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
    fn extract_text_from_single_text_block() {
        let resp = MessagesResponse {
            content: vec![ContentBlock {
                block_type: "text".into(),
                text: Some("Claude says hi".into()),
            }],
        };
        assert_eq!(extract_text(resp), Some("Claude says hi".into()));
    }

    #[test]
    fn extract_text_joins_multiple_text_blocks() {
        let resp = MessagesResponse {
            content: vec![
                ContentBlock {
                    block_type: "text".into(),
                    text: Some("Hello ".into()),
                },
                ContentBlock {
                    block_type: "text".into(),
                    text: Some("world".into()),
                },
            ],
        };
        assert_eq!(extract_text(resp), Some("Hello world".into()));
    }

    #[test]
    fn extract_text_ignores_non_text_blocks() {
        let resp = MessagesResponse {
            content: vec![
                ContentBlock {
                    block_type: "tool_use".into(),
                    text: None,
                },
                ContentBlock {
                    block_type: "text".into(),
                    text: Some("answer".into()),
                },
            ],
        };
        assert_eq!(extract_text(resp), Some("answer".into()));
    }

    #[test]
    fn extract_text_returns_none_for_empty_response() {
        let resp = MessagesResponse { content: vec![] };
        assert_eq!(extract_text(resp), None);
    }

    #[test]
    fn parses_response_json_from_string() {
        let json = r#"{"content":[{"type":"text","text":"Bitcoin is cool"}]}"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(extract_text(resp), Some("Bitcoin is cool".into()));
    }
}
