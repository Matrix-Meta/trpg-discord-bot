# AI 模組重寫與 TLS／依賴清理 實作計畫

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 清除自簽憑證與冗餘依賴、TLS 收斂為 native-tls，並把 AI 模組重寫為原生多供應商（OpenAI 相容 / Anthropic 原生 / Google Gemini 原生）架構。

**Architecture:** AI 模組拆成 `message`（供應商無關中介型別）、`config`（ApiConfig/ApiProvider）、`providers/{openai,anthropic,google,mod}`（各供應商 adapter + 列舉分派器）、`context`（單輪上下文）、`service`（編排 + 分塊）。呼叫端透過 `providers::complete()` / `providers::list_models()` 兩個分派函式，不再關心格式差異。不新增 async-trait 依賴，改用列舉分派 + 自由函式。

**Tech Stack:** Rust 2024、poise 0.6、serenity 0.12（native-tls）、reqwest 0.12（native-tls）、tokio-rusqlite、serde / serde_json。

---

## 設計約束與相容性

- 既有 `config.json` 內 `api_configs[*].provider` 字串為 `"OpenAI"`/`"OpenRouter"`/`"Anthropic"`/`"Google"`/`"Custom"`。**不可改名**這些變體；只新增 `Grok`。
- 既有外部引用路徑 `crate::ai::providers::ApiConfig`、`crate::ai::providers::ApiProvider` 被 `src/utils/config.rs` 與 `src/models/types.rs` 使用。重構後在 `src/ai/providers/mod.rs` 以 `pub use` 重新匯出，使這些路徑持續有效，避免動到那兩個檔案。
- 維持單輪對話；維持非串流；不改 `/chat`、`/prompt` 指令使用者介面。

## 檔案結構

重構後 `src/ai/`：

- `mod.rs` — 宣告子模組（modify）
- `message.rs` — `Role`、`ChatMessage`、`ChatRequest`、`Usage`、`Completion`、`ProviderError`（create）
- `config.rs` — `ApiConfig`、`ApiProvider`、`get_api_key_from_env`、`get_default_model_for_provider`（create，內容由舊 providers.rs 遷入並擴充 Grok）
- `providers/mod.rs` — `complete()`、`list_models()` 分派器、`determine_provider_from_url()`、re-export（create）
- `providers/openai.rs` — OpenAI 相容 adapter（create）
- `providers/anthropic.rs` — Anthropic 原生 adapter（create）
- `providers/google.rs` — Google Gemini 原生 adapter（create）
- `context.rs` — `ConversationManager`、`ConversationContext`（create，取代舊 prompt.rs）
- `service.rs` — `AiService`（modify／重寫）
- `commands/chat.rs` — 更新 import 與測試呼叫路徑（modify）
- `commands/prompt.rs` — 不動

刪除：`src/ai/providers.rs`（舊）、`src/ai/prompt.rs`（舊）。

> 註：每個 Task 結束時 `git add` 列出的路徑為該 Task 實際觸及檔案。整個重構過程中，中介狀態可能無法編譯（例如刪除舊 providers.rs 後到 mod.rs 更新前）；因此 Task 2–9 的「驗證編譯」步驟集中在 Task 10 完成後整體 `cargo check`。各 Task 仍獨立 commit 以利追蹤與回溯。

---

## Task 1: Part A — 機械清理（憑證 / TLS / rusqlite）

> **執行後修訂**：原計畫假設「移除 serenity 的 `rustls_backend` feature」即可收斂 TLS，實測無效——重複依賴（rustls / reqwest 0.11 / hyper 0.14）來自 serenity 0.12.4 內部，且 **poise 0.6 的 `default` feature 會強制啟用 `serenity/rustls_backend`**。經使用者確認後改採升級路線：serenity `0.12`→`0.12.5`、poise `0.6.1`→`0.6.2` 並關閉 poise default features（保留 `cache`/`chrono`/`handle_panics`）。serenity 0.12.5 將 reqwest 需求放寬為 `>=0.11.22`，得以與專案的 reqwest 0.12 統一。結果：rustls / reqwest 0.11 / hyper 0.14 / tokio-rustls 等重複依賴全數消除，native-tls + openssl 為唯一 TLS 堆疊。下方 Step 3 的原始指示已被此修訂取代。

**Files:**
- Delete: `SleepyNeko-Studios-Infra-Root-CA.crt`
- Modify: `src/main.rs:19-30`
- Modify: `Cargo.toml`

- [ ] **Step 1: 移除 main.rs 的憑證偵測邏輯**

刪除 `src/main.rs` 第 19–30 行整段（從註解 `// 1. 設定自簽憑證環境變數` 到對應的 `}`），即：

```rust
    // 1. 設定自簽憑證環境變數 (支援託管商 CA)
    // 檢查當前目錄下是否有 CRT 檔案，如果有則設定 SSL_CERT_FILE
    let cert_path = std::path::Path::new("SleepyNeko-Studios-Infra-Root-CA.crt");
    if cert_path.exists() {
        // 只有當環境變數尚未設定時才覆蓋，保留外部設定的彈性
        if env::var("SSL_CERT_FILE").is_err() {
            println!("偵測到本地 CA 憑證，設定 SSL_CERT_FILE...");
            unsafe {
                env::set_var("SSL_CERT_FILE", cert_path);
            }
        }
    }
```

刪除後 `main` 函式的第一個有效敘述應為 `if let Err(e) = utils::logger::DiscordLogger::init(...)`。

- [ ] **Step 2: 刪除憑證檔案**

Run: `git rm SleepyNeko-Studios-Infra-Root-CA.crt`
Expected: `rm 'SleepyNeko-Studios-Infra-Root-CA.crt'`

- [ ] **Step 3: 收斂 serenity TLS backend 為 native-tls**

在 `Cargo.toml` 把：

```toml
serenity = { version = "0.12", default-features = false, features = ["native_tls_backend", "client", "gateway", "rustls_backend"] }
```

改為：

```toml
serenity = { version = "0.12", default-features = false, features = ["native_tls_backend", "client", "gateway"] }
```

- [ ] **Step 4: 移除冗餘 rusqlite 直接依賴**

在 `Cargo.toml` 刪除這兩行（檔案結尾的 SQLite 區塊）：

```toml
# SQLite 相關
rusqlite = { version = "0.32", features = ["bundled"] }
```

保留 `tokio-rusqlite = "0.6"` 與 `libsqlite3-sys = { version = "0.30.1", features = ["bundled"] }`。

- [ ] **Step 5: 驗證編譯與依賴收斂**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished` 且無 error/warning。

Run: `cargo tree --duplicates 2>/dev/null | grep -E "rustls|reqwest 0.11|hyper v0.14|tokio-rustls" | head`
Expected: 無輸出（rustls/舊 reqwest/舊 hyper 重複項已消除）。

> 若 `main.rs` 出現 `unused import: std::env` 警告，將 `use std::env;` 保留與否依實際使用判斷：`main` 仍用 `env::var("DISCORD_TOKEN")`，故保留。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: 移除自簽憑證、收斂 TLS 為 native-tls、移除冗餘 rusqlite 依賴

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: 共用中介型別 `message.rs`

**Files:**
- Create: `src/ai/message.rs`

- [ ] **Step 1: 撰寫 message.rs（含測試）**

```rust
//! 供應商無關的中介型別：所有 adapter 以這些型別為輸入／輸出。
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_strings() {
        assert_eq!(Role::System.as_openai_str(), "system");
        assert_eq!(Role::Assistant.as_google_str(), "model");
        assert_eq!(Role::User.as_google_str(), "user");
        assert_eq!(Role::System.as_anthropic_str(), "user");
        assert_eq!(Role::Assistant.as_anthropic_str(), "assistant");
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/message.rs
git commit -m "feat(ai): 新增供應商無關中介型別 message.rs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: `config.rs` — ApiConfig / ApiProvider（新增 Grok）

**Files:**
- Create: `src/ai/config.rs`

- [ ] **Step 1: 撰寫 config.rs（由舊 providers.rs 遷入並擴充）**

```rust
//! API 設定型別與供應商列舉。維持與既有 config.json 的反序列化相容（新增 Grok）。
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_name")]
    pub name: String,
    pub api_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub enabled: bool,
    pub provider: ApiProvider,
}

fn default_api_name() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApiProvider {
    OpenAI,
    OpenRouter,
    Grok,
    Anthropic,
    Google,
    Custom,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            api_url: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: None,
            model: "gpt-3.5-turbo".to_string(),
            enabled: false,
            provider: ApiProvider::OpenAI,
        }
    }
}

pub fn get_api_key_from_env(provider: &ApiProvider) -> Option<String> {
    match provider {
        ApiProvider::OpenAI => env::var("OPENAI_API_KEY").ok(),
        ApiProvider::OpenRouter => env::var("OPENROUTER_API_KEY").ok(),
        ApiProvider::Grok => env::var("XAI_API_KEY").ok(),
        ApiProvider::Anthropic => env::var("ANTHROPIC_API_KEY").ok(),
        ApiProvider::Google => env::var("GOOGLE_API_KEY").ok(),
        ApiProvider::Custom => env::var("CUSTOM_API_KEY").ok(),
    }
}

pub fn env_var_name(provider: &ApiProvider) -> &'static str {
    match provider {
        ApiProvider::OpenAI => "OPENAI_API_KEY",
        ApiProvider::OpenRouter => "OPENROUTER_API_KEY",
        ApiProvider::Grok => "XAI_API_KEY",
        ApiProvider::Anthropic => "ANTHROPIC_API_KEY",
        ApiProvider::Google => "GOOGLE_API_KEY",
        ApiProvider::Custom => "CUSTOM_API_KEY",
    }
}

pub fn get_default_model_for_provider(provider: &ApiProvider) -> String {
    match provider {
        ApiProvider::OpenRouter => "google/gemma-2-9b-it".to_string(),
        ApiProvider::OpenAI => "gpt-4o-mini".to_string(),
        ApiProvider::Grok => "grok-2-latest".to_string(),
        ApiProvider::Anthropic => "claude-3-5-haiku-latest".to_string(),
        ApiProvider::Google => "gemini-1.5-flash".to_string(),
        ApiProvider::Custom => "gpt-3.5-turbo".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_openai() {
        let c = ApiConfig::default();
        assert_eq!(c.provider, ApiProvider::OpenAI);
        assert_eq!(c.model, "gpt-3.5-turbo");
    }

    #[test]
    fn legacy_provider_strings_deserialize() {
        // 既有 config.json 內既有變體必須仍可反序列化
        for s in ["OpenAI", "OpenRouter", "Anthropic", "Google", "Custom"] {
            let json = format!("\"{}\"", s);
            let p: ApiProvider = serde_json::from_str(&json).unwrap();
            let _ = env_var_name(&p);
        }
    }

    #[test]
    fn grok_uses_xai_key_name() {
        assert_eq!(env_var_name(&ApiProvider::Grok), "XAI_API_KEY");
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/config.rs
git commit -m "feat(ai): config.rs 拆出 ApiConfig/ApiProvider 並新增 Grok

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: OpenAI 相容 adapter

**Files:**
- Create: `src/ai/providers/openai.rs`

- [ ] **Step 1: 撰寫 openai.rs（建請求 / 解析，純函式可測）**

```rust
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
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/providers/openai.rs
git commit -m "feat(ai): 新增 OpenAI 相容 adapter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Anthropic 原生 adapter

**Files:**
- Create: `src/ai/providers/anthropic.rs`

- [ ] **Step 1: 撰寫 anthropic.rs**

```rust
//! Anthropic 原生 adapter：/v1/messages, x-api-key, anthropic-version。
use serde_json::{Value, json};

use crate::ai::message::{ChatRequest, Completion, ProviderError, Usage};

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
    if text.starts_with("<!DOCTYPE html") || text.contains("<html") {
        return Err(ProviderError::HtmlResponse);
    }
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
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/providers/anthropic.rs
git commit -m "feat(ai): 新增 Anthropic 原生 adapter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Google Gemini 原生 adapter

**Files:**
- Create: `src/ai/providers/google.rs`

- [ ] **Step 1: 撰寫 google.rs**

```rust
//! Google Gemini 原生 adapter：generateContent, systemInstruction, key 走 query string。
use serde_json::{Value, json};

use crate::ai::message::{ChatRequest, Completion, ProviderError, Usage};

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
    if text.starts_with("<!DOCTYPE html") || text.contains("<html") {
        return Err(ProviderError::HtmlResponse);
    }
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
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/providers/google.rs
git commit -m "feat(ai): 新增 Google Gemini 原生 adapter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: `providers/mod.rs` — 分派器 + 網路 I/O + re-export

**Files:**
- Create: `src/ai/providers/mod.rs`

- [ ] **Step 1: 撰寫 providers/mod.rs**

```rust
//! 供應商分派層：依 ApiProvider 選擇 adapter，統一處理 HTTP I/O。
pub mod anthropic;
pub mod google;
pub mod openai;

use std::time::Duration;

use crate::ai::config::ApiProvider;
use crate::ai::message::{ChatRequest, Completion, ProviderError};

// 對外重新匯出，維持 `crate::ai::providers::ApiConfig` 等既有路徑有效。
pub use crate::ai::config::{
    ApiConfig, ApiProvider as Provider, get_api_key_from_env, get_default_model_for_provider,
};
// 既有程式以 `crate::ai::providers::ApiProvider` 引用，保留同名匯出。
pub use crate::ai::config::ApiProvider as ApiProviderRe;

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
```

> 註：`ApiProviderRe` 與 `Provider` 別名僅為相容墊片；若 Task 10 後發現未使用，於 Task 10 清除。主要對外路徑是 `crate::ai::providers::{ApiConfig, ApiProvider}` —— 確保 `pub use crate::ai::config::{ApiConfig, ApiProvider}` 存在。請在本檔補一行：`pub use crate::ai::config::ApiProvider;`

- [ ] **Step 2: 修正 re-export，確保 `ApiProvider` 對外可見**

把 Step 1 的 re-export 區塊調整為：

```rust
pub use crate::ai::config::{
    ApiConfig, ApiProvider, get_api_key_from_env, get_default_model_for_provider,
};
```

並刪除 `Provider`、`ApiProviderRe` 兩個別名（避免重複定義與未使用警告）。同時把本檔內部使用 `ApiProvider` 的 `use crate::ai::config::ApiProvider;` 移除（已由 `pub use` 引入同層名稱）—— 若編譯報重複匯入，保留 `pub use` 那一處即可。

- [ ] **Step 3: Commit**

```bash
git add src/ai/providers/mod.rs
git commit -m "feat(ai): providers 分派層與統一 HTTP I/O

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: `context.rs` — 單輪上下文（取代 prompt.rs）

**Files:**
- Create: `src/ai/context.rs`
- Delete: `src/ai/prompt.rs`（於 Task 10 mod.rs 更新時生效）

- [ ] **Step 1: 撰寫 context.rs**

```rust
//! 單輪對話上下文建構：產出 system 提示與單則 user 訊息。
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use crate::ai::config::ApiConfig;
use crate::ai::message::{ChatMessage, Role};
use crate::utils::config::ConfigManager;

#[derive(Debug, Clone)]
pub struct ConversationContext {
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>, // 僅含當前 user 訊息
    pub total_tokens: usize,
    pub available_tokens: usize,
}

#[derive(Debug)]
pub struct ConversationManager {
    config: Arc<Mutex<ConfigManager>>,
}

impl ConversationManager {
    pub fn new(config: Arc<Mutex<ConfigManager>>) -> Self {
        Self { config }
    }

    pub async fn build_context(
        &self,
        guild_id: u64,
        user_message: &str,
        api_config: &ApiConfig,
    ) -> Result<ConversationContext> {
        let guild_config = {
            let config = self.config.lock().await;
            config.get_guild_config(guild_id).await
        };

        let max_context_tokens = Self::model_context_window(&api_config.model);
        let available_tokens =
            (max_context_tokens as f32 * guild_config.context_config.token_budget_ratio) as usize;

        let system_prompt = Self::build_system_prompt(&guild_config);
        let messages = vec![ChatMessage { role: Role::User, content: user_message.to_string() }];

        let total_tokens = Self::estimate_tokens(&system_prompt) + Self::estimate_tokens(user_message);
        if total_tokens > available_tokens {
            return Err(anyhow::anyhow!(
                "當前訊息過長，估算 tokens={}，可用上限={}。請縮短訊息內容。",
                total_tokens,
                available_tokens
            ));
        }

        Ok(ConversationContext { system_prompt, messages, total_tokens, available_tokens })
    }

    fn model_context_window(model: &str) -> usize {
        let m = model.to_lowercase();
        if m.contains("gemini-1.5") || m.contains("gemini-2") {
            1_000_000
        } else if m.contains("claude") {
            200_000
        } else if m.contains("gpt-4o") || m.contains("gpt-4.1") || m.contains("gpt-4-turbo") {
            128_000
        } else if m.contains("grok") {
            131_072
        } else if m.contains("gpt-3.5") {
            16_385
        } else {
            128_000 // 合理預設
        }
    }

    fn estimate_tokens(text: &str) -> usize {
        let cjk = text.chars().filter(|c| Self::is_cjk(*c)).count();
        let other = text.chars().count().saturating_sub(cjk);
        (cjk as f32 / 1.5) as usize + other / 4
    }

    fn is_cjk(c: char) -> bool {
        matches!(c,
            '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}' | '\u{2A700}'..='\u{2EBEF}')
    }

    fn build_system_prompt(guild_config: &crate::models::types::GuildConfig) -> String {
        let mut prompt = if let Some(custom) = &guild_config.custom_system_prompt {
            custom.clone()
        } else {
            String::from(
                "你是一個專業的 TRPG (桌上角色扮演遊戲) 助手。\n\
                 你的任務是幫助玩家和 GM (遊戲主持人) 進行遊戲。\n\
                 \n\
                 重要指引:\n\
                 1. 保持角色扮演的氛圍和沉浸感\n\
                 2. 提供有用的遊戲建議和規則解釋\n\
                 3. 協助推進劇情發展\n\
                 4. 回應要簡潔明瞭,避免過於冗長\n\
                 5. 使用繁體中文回應\n",
            )
        };
        let r = &guild_config.dnd_rules;
        prompt.push_str(&format!(
            "\n\n伺服器 D&D 規則:\n- 大成功: {}\n- 大失敗: {}\n",
            r.critical_success, r.critical_fail
        ));
        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_defaults() {
        assert_eq!(ConversationManager::model_context_window("gpt-4o"), 128_000);
        assert_eq!(ConversationManager::model_context_window("claude-3-5-haiku-latest"), 200_000);
        assert_eq!(ConversationManager::model_context_window("gemini-1.5-flash"), 1_000_000);
        assert_eq!(ConversationManager::model_context_window("some-unknown-model"), 128_000);
    }

    #[test]
    fn token_estimate_nonzero() {
        assert!(ConversationManager::estimate_tokens("你好世界") > 0);
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/context.rs
git commit -m "feat(ai): 單輪上下文 context.rs，系統提示獨立並更新模型視窗表

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: `service.rs` 重寫 — 編排 + 修正分塊

**Files:**
- Modify（重寫）: `src/ai/service.rs`

- [ ] **Step 1: 重寫 service.rs**

```rust
//! AI 服務編排：建上下文 → 取金鑰 → 算 max_tokens → 分派 complete → 分塊送出。
use std::sync::Arc;

use crate::ai::config::{ApiConfig, get_api_key_from_env};
use crate::ai::context::{ConversationContext, ConversationManager};
use crate::ai::message::ChatRequest;
use crate::ai::providers;
use crate::utils::config::ConfigManager;

const DISCORD_MSG_LIMIT: usize = 2000;

#[derive(Debug)]
pub struct AiService {
    conversation_manager: ConversationManager,
}

impl AiService {
    pub fn new(config: Arc<tokio::sync::Mutex<ConfigManager>>) -> Self {
        Self { conversation_manager: ConversationManager::new(config) }
    }

    pub async fn respond(
        &self,
        ctx: &poise::serenity_prelude::Context,
        msg: &poise::serenity_prelude::Message,
        user_message: &str,
        api_config: &ApiConfig,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let guild_id = msg.guild_id.ok_or("Guild not found")?.get();

        let context = self
            .conversation_manager
            .build_context(guild_id, user_message, api_config)
            .await?;

        let effective_api_key = api_config
            .api_key
            .clone()
            .or_else(|| get_api_key_from_env(&api_config.provider));

        if effective_api_key.is_none() {
            log::warn!("伺服器 {} 沒有有效的 API 金鑰", guild_id);
            msg.channel_id
                .say(&ctx.http, "錯誤：未找到 API 金鑰。請確保已在 .env 文件中設置相應的 API 金鑰環境變數。")
                .await?;
            return Err("missing api key".into());
        }

        let max_tokens = Self::calculate_output_tokens(&context);
        let request = ChatRequest {
            model: api_config.model.clone(),
            system: Some(context.system_prompt.clone()),
            messages: context.messages.clone(),
            temperature: Some(0.7),
            max_tokens,
        };

        let _typing = msg.channel_id.start_typing(&ctx.http);
        log::info!(
            "調用 AI: provider={:?}, model={}, url={}",
            api_config.provider, api_config.model, api_config.api_url
        );

        let completion = providers::complete(
            &api_config.provider,
            &api_config.api_url,
            effective_api_key.as_deref(),
            &request,
        )
        .await?;

        if let Some(u) = &completion.usage {
            log::info!("AI 用量: input={}, output={}", u.input_tokens, u.output_tokens);
        }

        let text = completion.text.trim();
        if text.is_empty() {
            return Ok(vec!["（AI 沒有回傳任何內容）".to_string()]);
        }
        Ok(split_discord_chunks(text))
    }

    fn calculate_output_tokens(context: &ConversationContext) -> u32 {
        let available = context.available_tokens.saturating_sub(context.total_tokens);
        available.clamp(256, 2048) as u32
    }
}

/// 依 Discord 2000 字元（以 char 計）上限切分為多則。
fn split_discord_chunks(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(DISCORD_MSG_LIMIT)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_one_chunk() {
        let chunks = split_discord_chunks("你好");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "你好");
    }

    #[test]
    fn long_text_splits() {
        let s = "あ".repeat(4500); // 4500 chars
        let chunks = split_discord_chunks(&s);
        assert_eq!(chunks.len(), 3); // 2000 + 2000 + 500
        assert_eq!(chunks[0].chars().count(), 2000);
        assert_eq!(chunks[2].chars().count(), 500);
    }

    #[test]
    fn no_truncation_total_preserved() {
        let s = "x".repeat(5000);
        let total: usize = split_discord_chunks(&s).iter().map(|c| c.chars().count()).sum();
        assert_eq!(total, 5000); // 不再截到 2000
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ai/service.rs
git commit -m "feat(ai): 重寫 service，走 provider 分派並修正 Discord 分塊

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: 串接 — `ai/mod.rs`、`main.rs`、`bot/data.rs`、`commands/chat.rs`，刪除舊檔

**Files:**
- Modify: `src/ai/mod.rs`
- Delete: `src/ai/providers.rs`、`src/ai/prompt.rs`
- Modify: `src/main.rs`
- Modify: `src/bot/data.rs`
- Modify: `src/ai/commands/chat.rs`

- [ ] **Step 1: 重寫 `src/ai/mod.rs`**

把現有內容替換為：

```rust
pub mod commands;
pub mod config;
pub mod context;
pub mod message;
pub mod providers;
pub mod service;
```

- [ ] **Step 2: 刪除舊檔**

Run:
```bash
git rm src/ai/providers.rs src/ai/prompt.rs
```
Expected: 兩檔被刪除。

- [ ] **Step 3: 更新 `bot/data.rs` —— 移除 ApiManager 欄位**

ApiManager 原只是 ConfigManager 的薄包裝；改為指令直接用 `config`。把 `src/bot/data.rs` 改為：

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::ai::service::AiService;
use crate::utils::config::ConfigManager;

#[derive(Clone, Debug)]
pub struct BotData {
    pub config: Arc<Mutex<ConfigManager>>,
    pub ai_service: Arc<AiService>,
    pub skills_db: tokio_rusqlite::Connection,
    #[allow(dead_code)] // 將在未來實現
    pub base_settings_db: tokio_rusqlite::Connection,
}
```

- [ ] **Step 4: 更新 `main.rs`**

- 移除 `use crate::ai::service::AiService;` 之外，刪掉 `ApiManager` 相關：
  - 刪除 `let api_manager = Arc::new(crate::ai::providers::ApiManager::new(...));`、`let shared_api_manager = ...;` 與 setup 內 `api_manager` clone。
- `AiService::new` 改為單參數：

```rust
let ai_service = Arc::new(AiService::new(config.clone()));
```

- `BotData { ... }` 建構移除 `api_manager` 欄位，保留 `config`、`ai_service`、`skills_db`、`base_settings_db`。
- `handle_message` 內取 api_config 改為直接讀 config：

把
```rust
let api_config = data.api_manager.get_guild_config(guild_id).await;
```
改為
```rust
let api_config = data.config.lock().await.get_guild_api_config(guild_id).await;
```

其餘 `handle_message` 邏輯不變（`api_config.enabled`、`respond(...)` 簽名一致）。

- [ ] **Step 5: 更新 `commands/chat.rs` 的 import 與呼叫**

- 頂部 import 改為：

```rust
use crate::ai::config::{
    ApiConfig, ApiProvider, env_var_name, get_api_key_from_env, get_default_model_for_provider,
};
use crate::ai::message::{ChatMessage, ChatRequest, Role};
use crate::ai::providers::{self, determine_provider_from_url};
use crate::bot::{Context, Error};
use poise::{ChoiceParameter, CreateReply, serenity_prelude as serenity};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tokio::time::timeout;
```

- 所有 `let data = ctx.data(); let api_manager = &data.api_manager;` 段落改為直接用 config。新增小輔助（放在檔案底部）以保留原本以「名稱」操作設定的行為：

```rust
async fn guild_configs(ctx: &Context<'_>, guild_id: u64)
    -> std::collections::HashMap<String, ApiConfig> {
    ctx.data().config.lock().await.get_guild_api_configs(guild_id).await
}
async fn active_config(ctx: &Context<'_>, guild_id: u64) -> ApiConfig {
    ctx.data().config.lock().await.get_guild_api_config(guild_id).await
}
```

  並把原 `api_manager.get_guild_configs(guild_id)` → `guild_configs(&ctx, guild_id)`、
  `api_manager.get_guild_config(guild_id)` → `active_config(&ctx, guild_id)`、
  `api_manager.add_guild_config(guild_id, cfg)` → `ctx.data().config.lock().await.add_guild_api_config(guild_id, cfg).await.ok();`、
  `api_manager.remove_guild_config(guild_id, name)` → `ctx.data().config.lock().await.remove_guild_api_config(guild_id, name).await.unwrap_or(false)`、
  `api_manager.set_active_api(guild_id, name)` → `ctx.data().config.lock().await.set_active_api(guild_id, name).await.unwrap_or(false)`。

- Add 分支的測試呼叫改用新 API。把舊的：

```rust
let test_request = ChatCompletionRequest { model: ..., messages: vec![ChatMessage {role:"user".to_string(), content:"測試".to_string()}], temperature: None, max_tokens: Some(10) };
let call_result = timeout(Duration::from_secs(10), crate::ai::providers::call_llm_api(&api_url, effective_api_key.as_deref(), &test_request, &test_provider)).await;
```

改為：

```rust
let test_request = ChatRequest {
    model: model.clone().unwrap_or_else(|| default_model.clone()),
    system: None,
    messages: vec![ChatMessage { role: Role::User, content: "測試".to_string() }],
    temperature: None,
    max_tokens: 10,
};
let call_result = timeout(
    Duration::from_secs(10),
    providers::complete(&test_provider, &api_url, effective_api_key.as_deref(), &test_request),
)
.await;
```

- ListModels 分支：`get_models_list(&url, key, &provider)` → `providers::list_models(&provider, &url, key)`。

- `save_api_key_to_env` 的 match 改用 `env_var_name(provider)`：

```rust
async fn save_api_key_to_env(provider: &ApiProvider, key: &str) {
    let env_path = Path::new(".env");
    let env_content = if env_path.exists() { std::fs::read_to_string(env_path).unwrap_or_default() } else { String::new() };
    let var = env_var_name(provider);
    let mut lines: Vec<String> = env_content.lines().map(|s| s.to_string()).collect();
    let mut found = false;
    for line in &mut lines {
        if line.starts_with(&format!("{}=", var)) { *line = format!("{}={}", var, key); found = true; break; }
    }
    if !found { lines.push(format!("{}={}", var, key)); }
    if let Ok(mut file) = OpenOptions::new().write(true).create(true).truncate(true).open(env_path) {
        let _ = file.write_all(lines.join("\n").as_bytes());
    }
}
```

- 刪除 chat.rs 內原本的本地 `determine_provider_from_url`（改用 `providers::determine_provider_from_url`），並在 `determine_provider_from_url` 呼叫處保持不變（已 import）。

- [ ] **Step 6: 整體編譯**

Run: `cargo check 2>&1 | tail -20`
Expected: `Finished`，無 error。修正所有編譯錯誤（型別／路徑），不得留 warning。

> 常見需修點：`models/types.rs` 與 `utils/config.rs` 仍寫 `crate::ai::providers::ApiConfig` —— 因 `providers/mod.rs` 已 `pub use`，應自動解析；若報錯，確認 re-export 行存在。

- [ ] **Step 7: 跑全部測試**

Run: `cargo test 2>&1 | tail -25`
Expected: 全數 PASS（含 message/config/openai/anthropic/google/providers/context/service 各模組測試）。

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(ai): 串接新 AI 模組、移除 ApiManager 薄包裝、刪除舊 providers/prompt

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 11: Part C — 更新 `.env.example`

**Files:**
- Modify: `.env.example`

- [ ] **Step 1: 改寫 .env.example**

把整個檔案內容替換為：

```
# 將此檔案複製為 .env 並填入你的 Discord bot token
DISCORD_TOKEN=your_discord_token_here

# 各供應商 API 金鑰（現在直連各家原生端點，不再經 OpenAI 相容代理）
# 只需填入你實際使用的供應商
OPENAI_API_KEY=your_openai_api_key_here
ANTHROPIC_API_KEY=your_anthropic_api_key_here
GOOGLE_API_KEY=your_google_api_key_here

# OpenAI 相容端點（透過 /chat add 指定 URL 使用）
OPENROUTER_API_KEY=your_openrouter_api_key_here
XAI_API_KEY=your_xai_grok_api_key_here
CUSTOM_API_KEY=your_custom_api_key_here
```

- [ ] **Step 2: Commit**

```bash
git add .env.example
git commit -m "docs: 更新 .env.example，修正供應商說明並新增 XAI_API_KEY

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 12: 最終驗收

**Files:** （無變更，純驗證）

- [ ] **Step 1: 零警告編譯**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished`，無 warning。

- [ ] **Step 2: Clippy（若可用）**

Run: `cargo clippy --all-targets 2>&1 | tail -15`
Expected: 無 error（warning 視情況修正明顯項）。

- [ ] **Step 3: 全測試**

Run: `cargo test 2>&1 | tail -20`
Expected: 全數 PASS。

- [ ] **Step 4: 依賴收斂確認**

Run: `cargo tree --duplicates 2>/dev/null | grep -E "rustls|reqwest 0.11|hyper v0.14" | head`
Expected: 無輸出。

- [ ] **Step 5: 確認憑證已不在 repo**

Run: `git ls-files | grep -i SleepyNeko`
Expected: 無輸出。

---

## 自我檢視（撰寫者備註）

- **Spec 覆蓋**：A1 憑證→T1；A2 TLS→T1；A3 rusqlite→T1；B Provider 抽象→T2–T9；原生三格式→T4/T5/T6；單輪→T8；非串流+分塊修正→T9；指令介面不變→T10；C .env→T11。
- **型別一致**：`ChatRequest`/`Completion`/`ProviderError`/`Role` 在 T2 定義，後續 T4–T9 一致引用；`providers::complete`/`list_models`/`determine_provider_from_url` 在 T7 定義，T9/T10 使用。
- **相容墊片**：`crate::ai::providers::{ApiConfig, ApiProvider}` 經 T7 `pub use` 保留，避免改動 `utils/config.rs`、`models/types.rs`。
- **已知取捨**：移除 `ApiManager` 薄包裝（原僅轉呼叫 ConfigManager），指令層改直接用 `config`，減少一層間接並消除 `BotData.api_manager`。此為 spec「targeted improvement」範圍內。
