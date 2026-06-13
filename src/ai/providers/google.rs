//! Google Gemini 原生 adapter：generateContent, systemInstruction, key 走 query string。
use serde_json::{Value, json};

use crate::ai::message::{ChatRequest, Completion, ProviderError, Usage, reject_html};

const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com";

/// 取得 base（去掉尾端斜線；若使用者只給空字串則用預設）。
fn base_of(api_url: &str) -> String {
    let trimmed = api_url.trim_end_matches('/');
    if trimmed.is_empty() {
        DEFAULT_BASE.to_string()
    } else if let Some(idx) = trimmed.find("/v1beta") {
        trimmed[..idx].to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn generate_url(api_url: &str, model: &str, api_key: Option<&str>) -> String {
    let key = api_key.unwrap_or("");
    format!(
        "{}/v1beta/models/{}:generateContent?key={}",
        base_of(api_url),
        model,
        key
    )
}

pub fn models_url(api_url: &str, api_key: Option<&str>) -> String {
    let key = api_key.unwrap_or("");
    format!("{}/v1beta/models?key={}", base_of(api_url), key)
}

/// systemInstruction 獨立；contents 用 role user/model。
pub fn build_body(req: &ChatRequest) -> Value {
    let contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({"role": m.role.as_google_str(), "parts": [{"text": m.content}]}))
        .collect();
    let mut body = json!({
        "contents": contents,
        "generationConfig": { "maxOutputTokens": req.max_tokens },
    });
    if let Some(sys) = &req.system {
        body["systemInstruction"] = json!({"parts": [{"text": sys}]});
    }
    if let Some(t) = req.temperature {
        body["generationConfig"]["temperature"] = json!(t);
    }
    body
}

pub fn parse_response(text: &str) -> Result<Completion, ProviderError> {
    reject_html(text)?;
    let v: Value = serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let parts = v["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or(ProviderError::Empty)?;
    let mut out = String::new();
    for p in parts {
        if let Some(t) = p["text"].as_str() {
            out.push_str(t);
        }
    }
    if out.is_empty() {
        return Err(ProviderError::Empty);
    }
    let usage = v.get("usageMetadata").map(|u| Usage {
        input_tokens: u["promptTokenCount"].as_u64().unwrap_or(0) as u32,
        output_tokens: u["candidatesTokenCount"].as_u64().unwrap_or(0) as u32,
    });
    Ok(Completion { text: out, usage })
}

pub fn parse_models(text: &str) -> Result<Vec<String>, ProviderError> {
    reject_html(text)?;
    let v: Value = serde_json::from_str(text).map_err(|e| ProviderError::Parse(e.to_string()))?;
    let mut out = Vec::new();
    if let Some(arr) = v["models"].as_array() {
        for item in arr {
            if let Some(name) = item["name"].as_str() {
                // name 形如 "models/gemini-1.5-flash"；取末段
                out.push(name.rsplit('/').next().unwrap_or(name).to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::message::{ChatMessage, Role};

    #[test]
    fn url_with_model_and_key() {
        let url = generate_url("https://generativelanguage.googleapis.com", "gemini-1.5-flash", Some("K"));
        assert_eq!(url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent?key=K");
    }

    #[test]
    fn body_has_system_instruction_and_model_role() {
        let req = ChatRequest {
            model: "gemini-1.5-flash".to_string(),
            system: Some("你是助手".to_string()),
            messages: vec![ChatMessage { role: Role::User, content: "嗨".to_string() }],
            temperature: None,
            max_tokens: 128,
        };
        let b = build_body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "你是助手");
        assert_eq!(b["contents"][0]["role"], "user");
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 128);
    }

    #[test]
    fn parse_candidates_text() {
        let json = r#"{"candidates":[{"content":{"parts":[{"text":"哈囉"}]}}],"usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":3}}"#;
        let c = parse_response(json).unwrap();
        assert_eq!(c.text, "哈囉");
        assert_eq!(c.usage.unwrap().output_tokens, 3);
    }

    #[test]
    fn parse_models_strips_prefix() {
        let json = r#"{"models":[{"name":"models/gemini-1.5-flash"}]}"#;
        assert_eq!(parse_models(json).unwrap(), vec!["gemini-1.5-flash"]);
    }
}
