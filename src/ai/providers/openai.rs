//! OpenAI 相容 adapter：涵蓋 OpenAI / OpenRouter / Grok / Custom。
use serde_json::{Value, json};

use crate::ai::config::ApiProvider;
use crate::ai::message::{ChatRequest, Completion, ProviderError, Usage};

/// 正規化 chat/completions 端點 URL。
pub fn chat_url(api_url: &str) -> String {
    if api_url.contains("chat/completions") {
        api_url.to_string()
    } else if api_url.ends_with("/v1") {
        format!("{}/chat/completions", api_url)
    } else {
        api_url.to_string()
    }
}

/// 正規化 models 端點 URL。
pub fn models_url(api_url: &str) -> String {
    if api_url.contains("chat/completions") {
        api_url.replace("chat/completions", "models")
    } else if api_url.ends_with("/v1") {
        format!("{}/models", api_url)
    } else {
        api_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{}/models", prefix))
            .unwrap_or_else(|| format!("{}/models", api_url))
    }
}

/// OpenRouter 需要額外標頭。
pub fn extra_headers(provider: &ApiProvider) -> Vec<(&'static str, String)> {
    match provider {
        ApiProvider::OpenRouter => vec![
            ("HTTP-Referer", "https://github.com/Matrix-Meta/trpg-discord-bot".to_string()),
            ("X-Title", "TRPG Discord Bot".to_string()),
        ],
        _ => vec![],
    }
}

/// 把中介請求轉成 OpenAI chat/completions JSON body。system 併入 messages 開頭。
pub fn build_body(req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(json!({"role": "system", "content": sys}));
    }
    for m in &req.messages {
        messages.push(json!({"role": m.role.as_openai_str(), "content": m.content}));
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": req.max_tokens,
    });
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

/// 解析回應 JSON 為 Completion。
pub fn parse_response(text: &str) -> Result<Completion, ProviderError> {
    if text.starts_with("<!DOCTYPE html") || text.contains("<html") {
        return Err(ProviderError::HtmlResponse);
    }
    let v: Value = serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or(ProviderError::Empty)?
        .to_string();
    let usage = v.get("usage").map(|u| Usage {
        input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
    });
    Ok(Completion { text: content, usage })
}

/// 解析 models 清單回應。
pub fn parse_models(text: &str) -> Result<Vec<String>, ProviderError> {
    if text.starts_with("<!DOCTYPE html") || text.contains("<html") {
        return Err(ProviderError::HtmlResponse);
    }
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
            model: "gpt-4o-mini".to_string(),
            system: Some("你是助手".to_string()),
            messages: vec![ChatMessage { role: Role::User, content: "嗨".to_string() }],
            temperature: Some(0.7),
            max_tokens: 256,
        }
    }

    #[test]
    fn body_puts_system_first() {
        let body = build_body(&sample_req());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(body["max_tokens"], 256);
    }

    #[test]
    fn url_normalization() {
        assert_eq!(chat_url("https://api.openai.com/v1"), "https://api.openai.com/v1/chat/completions");
        assert_eq!(models_url("https://api.openai.com/v1/chat/completions"), "https://api.openai.com/v1/models");
    }

    #[test]
    fn parse_ok() {
        let json = r#"{"choices":[{"message":{"content":"你好"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#;
        let c = parse_response(json).unwrap();
        assert_eq!(c.text, "你好");
        assert_eq!(c.usage.unwrap().output_tokens, 2);
    }

    #[test]
    fn parse_html_errors() {
        assert!(matches!(parse_response("<html></html>"), Err(ProviderError::HtmlResponse)));
    }

    #[test]
    fn parse_models_list() {
        let json = r#"{"data":[{"id":"gpt-4o"},{"id":"gpt-4o-mini"}]}"#;
        assert_eq!(parse_models(json).unwrap(), vec!["gpt-4o", "gpt-4o-mini"]);
    }
}
