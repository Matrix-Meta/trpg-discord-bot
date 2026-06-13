# AI 模組重寫與 TLS／依賴清理 — 設計文件

- 日期：2026-06-13
- 範圍：Part A 機械清理、Part B AI 模組完全重寫、Part C `.env.example` 更新
- 狀態：已通過設計確認，待寫實作計畫

---

## 背景

現況體檢（`trpg-discord-bot`，Rust，poise + serenity）：

- 程式碼健康（零警告編譯、低 `unwrap`、技術債極低），整體不需重寫。
- 但 AI 模組有具體缺陷：
  - **「多供應商」名實不符**：`ApiProvider` 列出 OpenAI/Anthropic/Google/Custom，但 `call_llm_api` 全部用 OpenAI `chat/completions` 格式 + `Bearer` 認證送出，直連 Anthropic（需 `x-api-key` + `/v1/messages`）或 Google Gemini（`generateContent`）官方端點會失敗。
  - **回應截斷矛盾**：`service.rs` 先截到 2000 字元再用 2000 字元分塊，等於回應永遠只有 1 塊、上限 2000 字元。
  - **模型視窗表過時**：硬編碼只到 `gpt-4o`/`claude-3`/`gemini-1.5`，新模型一律 fallback 8192。
- 依賴與設定面：
  - `serenity` 同時啟用 `native_tls_backend` 與 `rustls_backend`，導致 `reqwest 0.11+0.12`、`rustls 0.21+0.22`、`hyper 0.14+1.7` 等雙份依賴。
  - `Cargo.toml` 直接依賴 `rusqlite 0.32`，但程式碼只用 `tokio_rusqlite`（其已內含 rusqlite），為冗餘。
  - 隨附自簽憑證 `SleepyNeko-Studios-Infra-Root-CA.crt`，`main.rs` 偵測檔案並設定 `SSL_CERT_FILE`。

## 目標

1. 清除自簽憑證與相關程式碼。
2. TLS backend 收斂為 native-tls，消除雙份依賴。
3. 移除冗餘的 `rusqlite` 直接依賴。
4. 完全重寫 AI 模組，達成**原生多供應商**支援。
5. 更新 `.env.example`，消除誤導文字。

## 非目標（YAGNI）

- 不做串流輸出（維持非串流）。
- 不恢復對話記憶／自建記憶資料庫（維持單輪對話）。
- 不改動 `/chat`、`/prompt` 指令的使用者介面行為。
- 不做與本次目標無關的其他重構。

---

## Part A — 機械清理

### A1. 移除自簽憑證

- 刪除檔案 `SleepyNeko-Studios-Infra-Root-CA.crt`。
- 移除 `src/main.rs` 開頭偵測 `SleepyNeko-Studios-Infra-Root-CA.crt` 並設定 `SSL_CERT_FILE` 的整段（含 `unsafe { env::set_var(...) }`）。
- 自訂 CA 仍可由使用者自行透過標準 `SSL_CERT_FILE` 環境變數提供（native-tls 尊重系統信任庫），不需專案內建。

### A2. TLS backend 收斂為 native-tls

- `Cargo.toml` 的 `serenity` features 移除 `rustls_backend`，保留 `native_tls_backend`（與 `client`、`gateway`）。
- `reqwest` 維持 `default-features = false` + `native-tls`（不含 rustls）。
- 驗收：`cargo tree --duplicates` 不再出現 `rustls 0.21/0.22`、`reqwest 0.11`、`hyper 0.14`、`tokio-rustls` 等因雙 backend 產生的重複項。

### A3. 移除冗餘 rusqlite 直接依賴

- 刪除 `Cargo.toml` 中 `rusqlite = { version = "0.32", features = ["bundled"] }`。
- 全程式統一使用 `tokio_rusqlite`（含 `params`、`Error` 等）。
- 驗收：`cargo check` 通過、零警告。

---

## Part B — AI 模組完全重寫

### 架構：Provider trait 抽象

把「建請求 / 送出 / 解析回應 / 取模型清單」封裝進各供應商 adapter，呼叫端不關心格式差異。

```
src/ai/
  mod.rs
  config.rs          ApiConfig、ApiProvider enum、env 金鑰查找
  message.rs         共用 ChatMessage / ChatRequest / Completion(含 usage)
  providers/
    mod.rs           Provider trait + 依 enum 分派
    openai.rs        OpenAI 相容 (OpenAI / OpenRouter / Grok / DeepSeek / Custom)
    anthropic.rs     原生 (/v1/messages, x-api-key, anthropic-version)
    google.rs        原生 Gemini (generateContent, systemInstruction)
  manager.rs         ApiManager（per-guild 設定 CRUD，委派 ConfigManager）
  context.rs         ConversationManager（單輪上下文、系統提示、token 估算）
  service.rs         AiService.respond（編排 + 正確分塊）
  commands/
    chat.rs
    prompt.rs
```

### 資料模型

`message.rs`（供應商無關的中介型別）：

```rust
pub struct ChatMessage { pub role: Role, pub content: String } // Role: System|User|Assistant
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,   // 系統提示獨立欄位（Anthropic/Google 需要）
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
}
pub struct Usage { pub input_tokens: u32, pub output_tokens: u32 }
pub struct Completion { pub text: String, pub usage: Option<Usage> }
```

### Provider trait

```rust
#[async_trait-like 或以 BoxFuture]
trait Provider {
    async fn complete(&self, base_url: &str, api_key: Option<&str>, req: &ChatRequest)
        -> Result<Completion, ProviderError>;
    async fn list_models(&self, base_url: &str, api_key: Option<&str>)
        -> Result<Vec<String>, ProviderError>;
}
```

- 以 `match ApiProvider` 取得對應 adapter（回傳 `Box<dyn Provider>` 或直接分派函式）。
- 不另加 `async-trait` crate 的話，可用列舉分派 + 各自的 `async fn`（避免新依賴）。實作時擇一，優先不新增依賴。

### `ApiProvider` enum（serde 加法相容）

`OpenAi`、`OpenAiCompatible`、`Anthropic`、`Google`、`Custom`。

- 舊 `config.json` 內 `"OpenAI"`/`"Anthropic"`/`"Google"`/`"Custom"` 仍可反序列化（保留既有變體名稱或加 `#[serde(alias)]`）。
- `OpenAiCompatible` 涵蓋 OpenRouter / Grok(xAI) / DeepSeek / 其他相容端點。

### 三套原生格式

**OpenAI 相容（openai.rs）** — 涵蓋 OpenAI、OpenRouter、Grok、DeepSeek、Custom：
- `POST {base}/chat/completions`，`Authorization: Bearer {key}`。
- `system` 併入 `messages` 開頭（role=system）。
- OpenRouter 額外帶 `HTTP-Referer`、`X-Title` 標頭。
- 解析 `choices[0].message.content`；讀 `usage.prompt_tokens`/`completion_tokens`。
- `list_models`：`GET {base}/models`。

**Anthropic 原生（anthropic.rs）**：
- `POST {base}/v1/messages`，標頭 `x-api-key: {key}` + `anthropic-version: 2023-06-01`。
- body：`system` 為頂層欄位，`messages` 只含 user/assistant，`max_tokens` 必填。
- 解析 `content[].text`（type=text 串接）；讀 `usage.input_tokens`/`output_tokens`。
- `list_models`：`GET {base}/v1/models`（帶相同認證標頭）。

**Google Gemini 原生（google.rs）**：
- `POST {base}/v1beta/models/{model}:generateContent?key={key}`。
- body：`contents`（role user/model + parts.text），`systemInstruction`。
- 解析 `candidates[0].content.parts[].text`；讀 `usageMetadata`。
- `list_models`：`GET {base}/v1beta/models?key={key}`。

### 對話上下文（context.rs，維持單輪）

- `build_context` 仍只放系統提示 + 當前使用者訊息（系統提示改放 `ChatRequest.system`）。
- 保留「被回覆訊息」併入上下文的行為（由 `main.rs` handle_message 注入 user_message）。
- 保留 per-guild 自訂系統提示詞與 D&D 規則附加。
- 模型視窗表更新：以「字串包含」對應常見系列，無命中時預設 128_000；token 估算（中文/1.5、英文/4）僅用於送出前的長度防呆。
- 送出後以 API 回傳 `usage`（若有）記錄實際用量到 log。

### 服務編排（service.rs）

- 流程：build_context → 取 effective key（config 或 env）→ 算 max_tokens → 依 provider 分派 complete → 分塊送出。
- **修正分塊**：移除先截 2000 再分塊的矛盾邏輯，改為對完整回應依 Discord 2000 字元（以 `char` 計）上限切成多則送出；空回應給予明確提示。

### 指令層（commands/chat.rs、prompt.rs）

- 使用者介面與子指令維持不變（`add/remove/toggle/list/switch/list-models`、`set/reset/view/context`）。
- `add` 仍在儲存前送測試請求驗證；測試與正式呼叫都走新 Provider 抽象。
- provider 偵測：依 `api_url` 推斷（含 anthropic.com → Anthropic、googleapis.com → Google、openrouter.ai/x.ai → OpenAiCompatible、其餘可選 Custom），並允許使用者覆寫。

---

## Part C — `.env.example`

- 移除「Anthropic/Google 透過 OpenAI 相容端點」的誤導註解。
- 說明各金鑰現直連原生端點。
- 補上 `XAI_API_KEY`（Grok）。保留 `OPENROUTER_API_KEY`、`OPENAI_API_KEY`、`ANTHROPIC_API_KEY`、`GOOGLE_API_KEY`、`CUSTOM_API_KEY`。

---

## 錯誤處理

- 各 adapter 統一回傳 `ProviderError`（HTTP 失敗含 status + body、JSON 解析失敗、HTML 回應偵測、空回應）。
- 沿用既有對使用者的錯誤訊息風格（繁中、embed 或頻道訊息）。

## 測試

- 單元測試（不打網路）：
  - 各 adapter 的「請求建構」：URL 組裝、標頭、body 形狀（OpenAI/Anthropic/Google 各一）。
  - 各 adapter 的「回應解析」：以固定 JSON 字串驗證 `Completion.text` 與 `usage`。
  - provider 偵測：由 URL 推斷正確 `ApiProvider`。
  - 分塊：>2000 字元正確切多則、邊界、空字串。
- 保留既有 `providers` 測試的等價案例（遷移到新結構）。
- 驗收：`cargo check` 零警告、`cargo test` 通過。

## 風險與相容性

- `config.json` 既有 `api_configs` 需可反序列化 → 用變體別名／加法變更確保向後相容。
- 不新增重量級依賴（async-trait 視實作需要再決定，優先以列舉分派避免新依賴）。
- native-tls 環境需系統有 OpenSSL／對應 TLS 庫（部署環境既有，符合現況）。
