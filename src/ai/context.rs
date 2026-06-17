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
