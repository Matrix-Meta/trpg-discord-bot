//! 供應商分派層：依 ApiProvider 選擇 adapter，統一處理 HTTP I/O。
pub mod anthropic;
pub mod google;
pub mod openai;

use std::time::Duration;

use crate::ai::message::{ChatRequest, Completion, ProviderError};

// 對外重新匯出，維持 `crate::ai::providers::ApiConfig` 等既有路徑有效。
pub use crate::ai::config::{ApiConfig, ApiProvider};

fn client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| ProviderError::Http(e.to_string()))
}

/// 由 URL 推斷供應商。
pub fn determine_provider_from_url(url: &str) -> ApiProvider {
    if url.contains("openrouter.ai") {
        ApiProvider::OpenRouter
    } else if url.contains("x.ai") || url.contains("grok") {
        ApiProvider::Grok
    } else if url.contains("anthropic") {
        ApiProvider::Anthropic
    } else if url.contains("google") || url.contains("gemini") || url.contains("googleapis") {
        ApiProvider::Google
    } else if url.contains("openai.com") {
        ApiProvider::OpenAI
    } else {
        ApiProvider::Custom
    }
}

/// 統一補全入口：依供應商分派並執行 HTTP。
pub async fn complete(
    provider: &ApiProvider,
    api_url: &str,
    api_key: Option<&str>,
    req: &ChatRequest,
) -> Result<Completion, ProviderError> {
    let cl = client()?;
    match provider {
        ApiProvider::Anthropic => {
            let url = anthropic::messages_url(api_url);
            let body = anthropic::build_body(req);
            let mut b = cl
                .post(&url)
                .header("content-type", "application/json")
                .header("anthropic-version", anthropic::ANTHROPIC_VERSION);
            if let Some(k) = api_key {
                b = b.header("x-api-key", k);
            }
            let text = send(b.json(&body)).await?;
            anthropic::parse_response(&text)
        }
        ApiProvider::Google => {
            let url = google::generate_url(api_url, &req.model, api_key);
            let body = google::build_body(req);
            let b = cl.post(&url).header("content-type", "application/json");
            let text = send(b.json(&body)).await?;
            google::parse_response(&text)
        }
        // OpenAI / OpenRouter / Grok / Custom 共用 OpenAI 相容格式
        _ => {
            let url = openai::chat_url(api_url);
            let body = openai::build_body(req);
            let mut b = cl.post(&url).header("Content-Type", "application/json");
            for (k, v) in openai::extra_headers(provider) {
                b = b.header(k, v);
            }
            if let Some(k) = api_key {
                b = b.header("Authorization", format!("Bearer {}", k));
            }
            let text = send(b.json(&body)).await?;
            openai::parse_response(&text)
        }
    }
}

/// 統一模型清單入口。
pub async fn list_models(
    provider: &ApiProvider,
    api_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, ProviderError> {
    let cl = client()?;
    match provider {
        ApiProvider::Anthropic => {
            let url = anthropic::models_url(api_url);
            let mut b = cl.get(&url).header("anthropic-version", anthropic::ANTHROPIC_VERSION);
            if let Some(k) = api_key {
                b = b.header("x-api-key", k);
            }
            let text = send(b).await?;
            anthropic::parse_models(&text)
        }
        ApiProvider::Google => {
            let url = google::models_url(api_url, api_key);
            let text = send(cl.get(&url)).await?;
            google::parse_models(&text)
        }
        _ => {
            let url = openai::models_url(api_url);
            let mut b = cl.get(&url);
            for (k, v) in openai::extra_headers(provider) {
                b = b.header(k, v);
            }
            if let Some(k) = api_key {
                b = b.header("Authorization", format!("Bearer {}", k));
            }
            let text = send(b).await?;
            openai::parse_models(&text)
        }
    }
}

/// 送出請求並把非 2xx 轉成 ProviderError::Status。
async fn send(builder: reqwest::RequestBuilder) -> Result<String, ProviderError> {
    let resp = builder
        .send()
        .await
        .map_err(|e| ProviderError::Http(e.to_string()))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| ProviderError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(ProviderError::Status { status: status.as_u16(), body: text });
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_providers() {
        assert_eq!(determine_provider_from_url("https://openrouter.ai/api/v1"), ApiProvider::OpenRouter);
        assert_eq!(determine_provider_from_url("https://api.x.ai/v1"), ApiProvider::Grok);
        assert_eq!(determine_provider_from_url("https://api.anthropic.com"), ApiProvider::Anthropic);
        assert_eq!(determine_provider_from_url("https://generativelanguage.googleapis.com"), ApiProvider::Google);
        assert_eq!(determine_provider_from_url("https://api.openai.com/v1"), ApiProvider::OpenAI);
        assert_eq!(determine_provider_from_url("https://example.com/v1"), ApiProvider::Custom);
    }
}
