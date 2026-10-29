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
    #[error("{0}")]
    Config(String),
}

/// Which wire format a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `POST {base}/chat/completions` — OpenAI, OpenRouter, Ollama, LM Studio,
    /// vLLM, DeepSeek, Moonshot, and anything else that copies the schema.
    ChatCompletions,
    /// `POST {base}/responses` — OpenAI's newer Responses API.
    Responses,
    /// `POST {base}/messages` — Anthropic Messages.
    Anthropic,
}

impl Protocol {
    /// Parses what the UI sends, accepting the pre-0.8 spelling as well so an
    /// existing provider row keeps working.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "anthropic" => Protocol::Anthropic,
            "responses" => Protocol::Responses,
            // "open_ai" is what 0.7 stored; "chat_completions" is the new name.
            _ => Protocol::ChatCompletions,
        }
    }

    /// The label shown in the protocol dropdown.
    pub fn label(self) -> &'static str {
        match self {
            Protocol::ChatCompletions => "OpenAI Chat Completions",
            Protocol::Responses => "OpenAI Responses",
            Protocol::Anthropic => "Anthropic Messages",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    /// Base URL **including** the API version, e.g. `https://api.openai.com/v1`.
    /// `normalize_base_url` appends `/v1` for rows saved by earlier versions.
    pub base_url: String,
    /// The model used for this request. Chosen from `models` at call time.
    pub model: String,
    /// Every model the user added for this provider, in their order.
    #[serde(default)]
    pub models: Vec<String>,
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

/// Gives a 0.7-era base URL the version segment it relied on.
///
/// Providers saved by 0.7 stored `https://api.openai.com` and the request
/// builder appended `/v1`; 0.8 stores what the user typed (Hatoba's convention).
/// Only legacy rows go through this (see [`stored_base_url`]): applied to every
/// URL it would corrupt endpoints whose last segment is not a version, such as
/// Gemini's `…/v1beta/openai`.
pub fn normalize_base_url(base: &str) -> String {
    let trimmed = trim_base(base).to_string();
    let last = trimmed.rsplit('/').next().unwrap_or("");
    // Already versioned: /v1, /v1beta, /api/v1, /openai/v1 ...
    let versioned = last.starts_with('v')
        && last.len() <= 8
        && last[1..].chars().next().is_some_and(|c| c.is_ascii_digit());
    if versioned {
        trimmed
    } else {
        format!("{trimmed}/v1")
    }
}

/// The base URL a stored provider row should use.
///
/// A row without a `models` list was written by 0.7, whose URLs lacked the
/// version segment; every newer row holds exactly what the user typed.
pub fn stored_base_url(payload: &Value) -> String {
    let raw = payload
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if payload.get("models").is_none() && !raw.is_empty() {
        normalize_base_url(raw)
    } else {
        trim_base(raw).to_string()
    }
}

/// Whether two base URLs point at the same server: scheme, host and port.
///
/// A stored API key is only ever sent back to the server it was saved for.
/// Editing a provider's address to another host must not quietly hand the old
/// key to the new one.
pub fn same_origin(a: &str, b: &str) -> bool {
    match (reqwest::Url::parse(a.trim()), reqwest::Url::parse(b.trim())) {
        (Ok(a), Ok(b)) => {
            a.scheme() == b.scheme()
                && a.host_str().map(str::to_ascii_lowercase)
                    == b.host_str().map(str::to_ascii_lowercase)
                && a.port_or_known_default() == b.port_or_known_default()
        }
        _ => false,
    }
}

/// Joins the base URL and an endpoint path, exactly as given.
fn endpoint(base: &str, path: &str) -> String {
    format!("{}/{}", trim_base(base), path.trim_start_matches('/'))
}

/// Attaches the API key only when there is one.
///
/// A blank key is normal for a local server (Ollama, LM Studio, vLLM); sending
/// `Authorization: Bearer ` to those is harmless but some proxies reject it, so
/// the header is omitted entirely.
fn with_auth(
    req: reqwest::RequestBuilder,
    protocol: Protocol,
    key: &str,
) -> reqwest::RequestBuilder {
    if key.is_empty() {
        return req;
    }
    match protocol {
        Protocol::Anthropic => req
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        _ => req.bearer_auth(key),
    }
}

/// Sends one turn to the provider and returns what it wants to do next.
pub async fn complete(
    config: &ProviderConfig,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ModelReply, AiError> {
    match config.protocol {
        Protocol::ChatCompletions => openai_complete(config, messages, tools).await,
        Protocol::Responses => responses_complete(config, messages, tools).await,
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

    let url = endpoint(&config.base_url, "chat/completions");
    let resp = with_auth(client()?.post(&url), config.protocol, &config.api_key)
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

    let url = endpoint(&config.base_url, "messages");
    let resp = with_auth(client()?.post(&url), config.protocol, &config.api_key)
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

/// OpenAI's Responses API (`POST {base}/responses`), non-streaming.
///
/// The message shape differs from Chat Completions: the system prompt is a
/// top-level `instructions` string, tool calls and their results are items in a
/// flat `input` array, and the reply arrives as an `output` array.
async fn responses_complete(
    config: &ProviderConfig,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
) -> Result<ModelReply, AiError> {
    let body = responses_body(config, messages, tools);

    let url = endpoint(&config.base_url, "responses");
    let resp = with_auth(client()?.post(&url), config.protocol, &config.api_key)
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
    parse_responses_reply(&parsed)
}

/// Builds the Responses API request body.
///
/// Pure so the wire shape can be asserted in a unit test without a live
/// endpoint.
fn responses_body(config: &ProviderConfig, messages: &[ChatMessage], tools: &[ToolSpec]) -> Value {
    let instructions: String = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut input: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => continue,
            Role::User => input.push(json!({
                "role": "user",
                "content": [{ "type": "input_text", "text": m.content }]
            })),
            Role::Assistant => {
                if !m.content.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": m.content }]
                    }));
                }
                for c in &m.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": c.id,
                        "name": c.name,
                        // Responses wants the arguments as a JSON string.
                        "arguments": c.arguments.to_string(),
                    }));
                }
            }
            Role::Tool => input.push(json!({
                "type": "function_call_output",
                "call_id": m.tool_call_id.clone().unwrap_or_default(),
                "output": m.content,
            })),
        }
    }

    let mut body = json!({
        "model": config.model,
        "input": input,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                        "strict": false,
                    })
                })
                .collect(),
        );
    }

    body
}

/// Reads a Responses payload into the shared reply shape.
fn parse_responses_reply(parsed: &Value) -> Result<ModelReply, AiError> {
    let output = parsed
        .get("output")
        .and_then(|o| o.as_array())
        .ok_or_else(|| AiError::Parse("响应里没有 output".into()))?;

    let mut out_text = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item.get("type").and_then(|v| v.as_str()) {
            Some("message") => {
                if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                    for part in parts {
                        if part.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                                out_text.push_str(t);
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let args = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or_else(|| json!({}));
                tool_calls.push(ToolCall {
                    id: item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("call")
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    arguments: args,
                });
            }
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

// ---- Model listing and connection test -----------------------------------

/// Rejects addresses that are not plain HTTP(S), cannot be parsed, or smuggle
/// credentials in the userinfo section (which would leak them in logs).
pub fn validate_base_url(raw: &str) -> Result<String, AiError> {
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Err(AiError::Config("请填写基础 URL".into()));
    }
    let parsed =
        reqwest::Url::parse(&trimmed).map_err(|_| AiError::Config("基础 URL 格式不正确".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AiError::Config(
            "基础 URL 必须以 http:// 或 https:// 开头".into(),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(AiError::Config("基础 URL 缺少主机名".into()));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AiError::Config("基础 URL 不能包含用户名或密码".into()));
    }
    Ok(trimmed)
}

/// Lists the models the provider offers (`GET {base}/models`).
pub async fn list_models(config: &ProviderConfig) -> Result<Vec<String>, AiError> {
    let url = endpoint(&config.base_url, "models");
    let resp = with_auth(client()?.get(&url), config.protocol, &config.api_key)
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
            "获取模型列表失败 {status}: {}",
            truncate(&text, 300)
        )));
    }

    let parsed: Value = serde_json::from_str(&text).map_err(|e| AiError::Parse(e.to_string()))?;
    // Both OpenAI (`{"data":[{"id":...}]}`) and a bare array are accepted; some
    // local servers return the latter.
    let items = parsed
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| parsed.as_array())
        .ok_or_else(|| AiError::Parse("模型列表格式无法识别".into()))?;

    let mut out: Vec<String> = Vec::new();
    for item in items {
        let id = item
            .get("id")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str());
        if let Some(id) = id {
            if !id.is_empty() && !out.iter().any(|m| m == id) {
                out.push(id.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Sends the smallest possible request to prove the provider answers.
///
/// With a model it is a real one-token completion; without one it only checks
/// that `/models` responds, which is enough to prove URL + key work.
pub async fn test_connection(config: &ProviderConfig) -> Result<String, AiError> {
    if config.model.trim().is_empty() {
        let models = list_models(config).await?;
        return Ok(format!("连接正常，可列出 {} 个模型", models.len()));
    }

    let probe = vec![ChatMessage::user("ping")];
    let reply = complete(config, &probe, &[]).await?;
    let got = if reply.text.trim().is_empty() {
        "（模型返回了空内容，但请求成功）".to_string()
    } else {
        truncate(reply.text.trim(), 80)
    };
    Ok(format!("连接正常，模型回复：{got}"))
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
    fn base_url_gains_a_version_when_missing() {
        // 0.7 rows stored no version; the request builder used to add /v1.
        assert_eq!(
            normalize_base_url("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/"),
            "https://api.openai.com/v1"
        );
        // Already versioned stays untouched.
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434/v1"),
            "http://localhost:11434/v1"
        );
        // A version-ish suffix other than /v1 is respected.
        assert_eq!(
            normalize_base_url("https://x.dev/v1beta"),
            "https://x.dev/v1beta"
        );
    }

    #[test]
    fn endpoint_joins_without_doubling_slashes() {
        assert_eq!(
            endpoint("https://api.openai.com/v1/", "chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        // The URL is used as typed: no version segment is invented.
        assert_eq!(
            endpoint(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "chat/completions"
            ),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
    }

    #[test]
    fn only_legacy_rows_gain_a_version_segment() {
        // 0.7 row: no `models` list, URL without a version.
        let legacy = json!({ "base_url": "https://api.openai.com", "model": "gpt-4o" });
        assert_eq!(stored_base_url(&legacy), "https://api.openai.com/v1");
        // Current row: kept exactly as typed, even when it does not end in /vN.
        let current = json!({
            "base_url": "https://generativelanguage.googleapis.com/v1beta/openai/",
            "models": ["gemini-2.5-flash"],
        });
        assert_eq!(
            stored_base_url(&current),
            "https://generativelanguage.googleapis.com/v1beta/openai"
        );
    }

    #[test]
    fn same_origin_compares_scheme_host_and_port() {
        assert!(same_origin(
            "https://api.openai.com/v1",
            "https://API.openai.com/v2/"
        ));
        // An explicit default port is the same origin.
        assert!(same_origin(
            "https://api.openai.com/v1",
            "https://api.openai.com:443/v1"
        ));
        assert!(!same_origin(
            "https://api.openai.com/v1",
            "https://evil.example/v1"
        ));
        assert!(!same_origin(
            "https://api.openai.com/v1",
            "http://api.openai.com/v1"
        ));
        assert!(!same_origin(
            "http://localhost:11434/v1",
            "http://localhost:1234/v1"
        ));
        assert!(!same_origin("not a url", "https://api.openai.com"));
    }

    #[test]
    fn base_url_validation_rejects_bad_shapes() {
        assert!(validate_base_url("https://api.openai.com/v1").is_ok());
        // Local servers over plain http are legitimate.
        assert!(validate_base_url("http://localhost:11434/v1").is_ok());
        assert!(validate_base_url("").is_err());
        assert!(validate_base_url("api.openai.com/v1").is_err());
        assert!(validate_base_url("ftp://example.com").is_err());
        // Credentials in the URL would end up in logs.
        assert!(validate_base_url("https://user:pw@example.com/v1").is_err());
    }

    #[test]
    fn protocol_parsing_accepts_the_old_spelling() {
        assert_eq!(Protocol::parse("open_ai"), Protocol::ChatCompletions);
        assert_eq!(
            Protocol::parse("chat_completions"),
            Protocol::ChatCompletions
        );
        assert_eq!(Protocol::parse("responses"), Protocol::Responses);
        assert_eq!(Protocol::parse("anthropic"), Protocol::Anthropic);
        // Anything unknown falls back rather than panicking.
        assert_eq!(Protocol::parse("nonsense"), Protocol::ChatCompletions);
    }

    fn cfg(protocol: Protocol) -> ProviderConfig {
        ProviderConfig {
            id: "p".into(),
            name: "t".into(),
            protocol,
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o".into(),
            models: vec!["gpt-4o".into()],
            api_key: "k".into(),
        }
    }

    #[test]
    fn responses_body_uses_instructions_and_input_items() {
        let messages = vec![
            ChatMessage::system("be brief"),
            ChatMessage::user("hi"),
            ChatMessage::assistant(
                "",
                vec![ToolCall {
                    id: "c1".into(),
                    name: "run_command".into(),
                    arguments: json!({"command": "ls"}),
                }],
            ),
            ChatMessage::tool("c1", "out"),
        ];
        let body = responses_body(&cfg(Protocol::Responses), &messages, &[]);

        // System goes out of band, per the Responses API.
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["model"], "gpt-4o");
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        // A tool call is a flattened item, and args are a JSON *string*.
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "c1");
        assert!(input[1]["arguments"].is_string());
        // The result references the call id.
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "c1");
        assert_eq!(input[2]["output"], "out");
        // No system message may appear inside `input`.
        assert!(input
            .iter()
            .all(|i| i.get("role").and_then(|r| r.as_str()) != Some("system")));
    }

    #[test]
    fn responses_body_declares_tools_flat() {
        let tools = vec![ToolSpec {
            name: "read_file".into(),
            description: "read".into(),
            parameters: json!({"type": "object"}),
        }];
        let body = responses_body(&cfg(Protocol::Responses), &[ChatMessage::user("x")], &tools);
        let t = &body["tools"][0];
        assert_eq!(t["type"], "function");
        assert_eq!(t["name"], "read_file");
        // Unlike Chat Completions there is no nested "function" object.
        assert!(t.get("function").is_none());
        assert_eq!(t["strict"], false);
    }

    #[test]
    fn responses_reply_collects_text_and_tool_calls() {
        let payload = json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "hello "}]},
                {"type": "message", "content": [{"type": "output_text", "text": "world"}]},
                {"type": "function_call", "call_id": "c9", "name": "run_command",
                 "arguments": "{\"command\":\"df -h\"}"},
            ],
            "usage": {"input_tokens": 11, "output_tokens": 7}
        });
        let reply = parse_responses_reply(&payload).unwrap();
        assert_eq!(reply.text, "hello world");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "c9");
        assert_eq!(reply.tool_calls[0].arguments["command"], "df -h");
        assert_eq!(reply.input_tokens, Some(11));
        assert_eq!(reply.output_tokens, Some(7));
    }

    #[test]
    fn truncate_keeps_short_text_whole() {
        assert_eq!(truncate("hello", 10), "hello");
        assert!(truncate(&"x".repeat(500), 10).chars().count() <= 11);
    }
}
