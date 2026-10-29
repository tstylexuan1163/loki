//! Model providers: OpenAI-compatible chat completions and Anthropic messages.
//!
//! Only these two wire formats are implemented; between them they cover the
//! OpenAI API, OpenRouter, Ollama, LM Studio, vLLM, and Anthropic. The rest of
//! the app speaks [`ChatMessage`] and never touches provider JSON.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("请求失败：{0}")]
    Http(String),
    #[error("模型返回了错误：{0}")]
    Provider(String),
    #[error("返回内容无法解析：{0}")]
    Parse(String),
    #[error("没有配置模型服务商")]
    NotConfigured,
}

/// Which wire format a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `POST /v1/chat/completions` — OpenAI, OpenRouter, Ollama, LM Studio, vLLM.
    OpenAi,
    /// `POST /v1/messages` — Anthropic.
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    /// Base URL without the trailing `/v1`, e.g. `https://api.openai.com`.
    pub base_url: String,
    pub model: String,
    /// Stored encrypted in the vault; never returned to the WebView.
    pub api_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON object the model produced.
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Set on assistant messages that asked for tools.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    /// Set on tool results: which call this answers.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: Some(call_id.into()),
        }
    }
}

/// What the model asked for on one turn.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReply {
    /// Text the model produced alongside any tool calls.
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// A tool the model may call, described in JSON Schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema object for the arguments.
    pub parameters: Value,
}

fn client() -> Result<reqwest::Client, AiError> {
    reqwest::Client::builder()
        // A model that has not answered in two minutes is not going to.
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| AiError::Http(e.to_string()))
}

fn trim_base(base: &str) -> &str {
    base.trim_end_matches('/')
}

/// Sends one turn to the provider and returns what it wants to do next.
pub async fn complete(
    config: &ProviderConfig,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ModelReply, AiError> {
    match config.protocol {
        Protocol::OpenAi => openai_complete(config, messages, tools).await,
        Protocol::Anthropic => anthropic_complete(config, messages, tools).await,
    }
}

async fn openai_complete(
    config: &ProviderConfig,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ModelReply, AiError> {
    let mut wire = Vec::with_capacity(messages.len());
    for m in messages {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        let mut entry = json!({ "role": role, "content": m.content });
        if !m.tool_calls.is_empty() {
            entry["tool_calls"] = Value::Array(
                m.tool_calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                // OpenAI wants the arguments as a JSON *string*.
                                "arguments": c.arguments.to_string(),
                            }
                        })
                    })
                    .collect(),
            );
        }
        if let Some(id) = &m.tool_call_id {
            entry["tool_call_id"] = json!(id);
        }
        wire.push(entry);
    }

    let mut body = json!({
        "model": config.model,
        "messages": wire,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        );
        body["tool_choice"] = json!("auto");
    }

    let url = format!("{}/v1/chat/completions", trim_base(&config.base_url));
    let resp = client()?
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| AiError::Http(e.to_string()))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| AiError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(AiError::Provider(format!(
            "{status}: {}",
            truncate(&text, 400)
        )));
    }

    let parsed: Value = serde_json::from_str(&text).map_err(|e| AiError::Parse(e.to_string()))?;
    let choice = parsed
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| AiError::Parse("响应里没有 choices".into()))?;
    let message = choice.get("message").cloned().unwrap_or(json!({}));

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for c in calls {
            let id = c
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("call")
                .to_string();
            let f = c.get("function").cloned().unwrap_or(json!({}));
            let name = f
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // The arguments arrive as a JSON string and may be malformed; a bad
            // call is reported to the model as a tool error, not a crash.
            let arguments = f
                .get("arguments")
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| json!({}));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    Ok(ModelReply {
        text: message
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        tool_calls,
        input_tokens: parsed
            .pointer("/usage/prompt_tokens")
            .and_then(|v| v.as_u64()),
        output_tokens: parsed
            .pointer("/usage/completion_tokens")
            .and_then(|v| v.as_u64()),
    })
}

async fn anthropic_complete(
    config: &ProviderConfig,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ModelReply, AiError> {
    // Anthropic takes the system prompt out of band, and tool results are user
    // messages of type `tool_result`.
    let system: String = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut wire: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => continue,
            Role::User => wire.push(json!({ "role": "user", "content": m.content })),
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": m.content }));
                }
                for c in &m.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": c.arguments,
                    }));
                }
                wire.push(json!({ "role": "assistant", "content": blocks }));
            }
            Role::Tool => wire.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                }]
            })),
        }
    }

    let mut body = json!({
        "model": config.model,
        "max_tokens": 4096,
        "messages": wire,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect(),
        );
    }

    let url = format!("{}/v1/messages", trim_base(&config.base_url));
    let resp = client()?
        .post(&url)
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| AiError::Http(e.to_string()))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| AiError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(AiError::Provider(format!(
            "{status}: {}",
            truncate(&text, 400)
        )));
    }

    let parsed: Value = serde_json::from_str(&text).map_err(|e| AiError::Parse(e.to_string()))?;
    let blocks = parsed
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| AiError::Parse("响应里没有 content".into()))?;

    let mut out_text = String::new();
    let mut tool_calls = Vec::new();
    for b in blocks {
        match b.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                    out_text.push_str(t);
                }
            }
            Some("tool_use") => tool_calls.push(ToolCall {
                id: b
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("call")
                    .to_string(),
                name: b
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                arguments: b.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
            _ => {}
        }
    }

    Ok(ModelReply {
        text: out_text,
        tool_calls,
        input_tokens: parsed
            .pointer("/usage/input_tokens")
            .and_then(|v| v.as_u64()),
        output_tokens: parsed
            .pointer("/usage/output_tokens")
            .and_then(|v| v.as_u64()),
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        assert_eq!(
            trim_base("https://api.openai.com/"),
            "https://api.openai.com"
        );
        assert_eq!(
            trim_base("http://localhost:11434"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn messages_serialize_with_expected_roles() {
        let m = ChatMessage::assistant(
            "let me look",
            vec![ToolCall {
                id: "c1".into(),
                name: "run_command".into(),
                arguments: json!({"command": "ls"}),
            }],
        );
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["toolCalls"][0]["name"], "run_command");
        // Tool results carry the id they answer.
        let t = ChatMessage::tool("c1", "out");
        assert_eq!(serde_json::to_value(&t).unwrap()["toolCallId"], "c1");
    }

    #[test]
    fn truncate_keeps_short_text_whole() {
        assert_eq!(truncate("hello", 10), "hello");
        assert!(truncate(&"x".repeat(500), 10).chars().count() <= 11);
    }
}
