//! 供應商無關的中介型別：所有 adapter 以這些型別為輸入／輸出。
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // System/Assistant 供未來多輪對話與 adapter 角色映射使用
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    /// OpenAI 相容格式的角色字串。
    pub fn as_openai_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }

    /// Anthropic / Google 只接受 user / assistant(model)。
    pub fn as_anthropic_str(&self) -> &'static str {
        match self {
            Role::Assistant => "assistant",
            _ => "user",
        }
    }

    /// Google Gemini 用 "model" 表示 assistant。
    pub fn as_google_str(&self) -> &'static str {
        match self {
            Role::Assistant => "model",
            _ => "user",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// 供應商無關的請求。`system` 獨立於 `messages`，由各 adapter 自行併入或分離。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP 請求失敗: {0}")]
    Http(String),
    #[error("API 回傳錯誤狀態 {status}: {body}")]
    Status { status: u16, body: String },
    #[error("收到 HTML 回應而非 JSON，API 端點 URL 可能不正確")]
    HtmlResponse,
    #[error("回應解析失敗: {0}")]
    Parse(String),
    #[error("API 回應為空")]
    Empty,
}

/// 若回應實為 HTML（通常是 API 端點 URL 錯誤導向錯誤頁），回傳 HtmlResponse。
/// 各 adapter 的 parse 函式應在解析 JSON 前統一呼叫此守衛。
pub fn reject_html(text: &str) -> Result<(), ProviderError> {
    if text.starts_with("<!DOCTYPE html") || text.contains("<html") {
        return Err(ProviderError::HtmlResponse);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_html_detects_html() {
        assert!(matches!(reject_html("<html></html>"), Err(ProviderError::HtmlResponse)));
        assert!(matches!(reject_html("<!DOCTYPE html><body>"), Err(ProviderError::HtmlResponse)));
        assert!(reject_html(r#"{"ok":true}"#).is_ok());
    }

    #[test]
    fn role_strings() {
        assert_eq!(Role::System.as_openai_str(), "system");
        assert_eq!(Role::Assistant.as_google_str(), "model");
        assert_eq!(Role::User.as_google_str(), "user");
        assert_eq!(Role::System.as_anthropic_str(), "user");
        assert_eq!(Role::Assistant.as_anthropic_str(), "assistant");
    }
}
