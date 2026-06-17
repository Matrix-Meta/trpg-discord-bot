//! Anthropic 原生 adapter：/v1/messages, x-api-key, anthropic-version。
use serde_json::{Value, json};

use crate::ai::message::{ChatRequest, Completion, ProviderError, Usage, reject_html};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 正規化 messages 端點 URL。使用者通常設定 base（如 https://api.anthropic.com）。
pub fn messages_url(api_url: &str) -> String {
    if api_url.contains("/v1/messages") {
        api_url.to_string()
    } else {
        format!("{}/v1/messages", api_url.trim_end_matches('/'))
    }
}

pub fn models_url(api_url: &str) -> String {
    if api_url.contains("/v1/models") {
        api_url.to_string()
    } else {
        format!("{}/v1/models", api_url.trim_end_matches('/'))
    }
}

/// system 為頂層欄位；messages 僅含 user/assistant。
pub fn build_body(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({"role": m.role.as_anthropic_str(), "content": m.content}))
        .collect();
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
    });
    if let Some(sys) = &req.system {
        body["system"] = json!(sys);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

pub fn parse_response(text: &str) -> Result<Completion, ProviderError> {
    reject_html(text)?;
    let v: Value = serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let blocks = v["content"].as_array().ok_or(ProviderError::Empty)?;
    let mut out = String::new();
    for b in blocks {
        if b["type"] == "text" {
            if let Some(t) = b["text"].as_str() {
                out.push_str(t);
            }
        }
    }
    if out.is_empty() {
        return Err(ProviderError::Empty);
    }
    let usage = v.get("usage").map(|u| Usage {
        input_tokens: u["input_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0) as u32,
    });
    Ok(Completion { text: out, usage })
}

pub fn parse_models(text: &str) -> Result<Vec<String>, ProviderError> {
    reject_html(text)?;
    let v: Value = serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let mut out = Vec::new();
    if let Some(arr) = v["data"].as_array() {
        for item in arr {
            if let Some(id) = item["id"].as_str() {
                out.push(id.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::message::{ChatMessage, Role};

    fn sample_req() -> ChatRequest {
        ChatRequest {
            model: "claude-3-5-haiku-latest".to_string(),
            system: Some("你是助手".to_string()),
            messages: vec![ChatMessage { role: Role::User, content: "嗨".to_string() }],
            temperature: Some(0.7),
            max_tokens: 256,
        }
    }

    #[test]
    fn body_separates_system() {
        let body = sample_req();
        let b = build_body(&body);
        assert_eq!(b["system"], "你是助手");
        assert_eq!(b["max_tokens"], 256);
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        assert!(b["messages"].as_array().unwrap().iter().all(|m| m["role"] != "system"));
    }

    #[test]
    fn url_appends_v1_messages() {
        assert_eq!(messages_url("https://api.anthropic.com"), "https://api.anthropic.com/v1/messages");
        assert_eq!(messages_url("https://api.anthropic.com/v1/messages"), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn parse_concatenates_text_blocks() {
        let json = r#"{"content":[{"type":"text","text":"你"},{"type":"text","text":"好"}],"usage":{"input_tokens":5,"output_tokens":2}}"#;
        let c = parse_response(json).unwrap();
        assert_eq!(c.text, "你好");
        assert_eq!(c.usage.unwrap().input_tokens, 5);
    }
}
