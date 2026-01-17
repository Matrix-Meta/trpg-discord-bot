use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use crate::ai::providers::ApiManager;
use crate::utils::config::ConfigManager;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
    pub importance: f32,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConversationContext {
    pub system_prompt: String,
    pub messages: Vec<ConversationMessage>,
    pub total_tokens: usize,
    pub max_context_tokens: usize,
    pub available_tokens: usize,
}

#[derive(Debug)]
pub struct ConversationManager {
    config: Arc<Mutex<ConfigManager>>,
    api_manager: Arc<ApiManager>,
}

impl ConversationManager {
    pub fn new(config: Arc<Mutex<ConfigManager>>, api_manager: Arc<ApiManager>) -> Self {
        Self {
            config,
            api_manager,
        }
    }

    pub async fn build_context(
        &self,
        guild_id: u64,
        user_message: &str,
    ) -> Result<ConversationContext> {
        let guild_config = {
            let config = self.config.lock().await;
            config.get_guild_config(guild_id).await
        };

        let api_config = self.api_manager.get_guild_config(guild_id).await;
        let max_context_tokens = self.get_model_context_window(&api_config.model);

        let available_tokens =
            (max_context_tokens as f32 * guild_config.context_config.token_budget_ratio) as usize;

        log::info!(
            "構建對話上下文: guild_id={}, max_tokens={}, available_tokens={}, ratio={}",
            guild_id,
            max_context_tokens,
            available_tokens,
            guild_config.context_config.token_budget_ratio
        );

        let system_prompt = self.build_system_prompt(guild_id, &guild_config).await?;

        let mut messages = Vec::new();

        messages.push(ConversationMessage {
            role: "system".to_string(),
            content: system_prompt.clone(),
            timestamp: None,
            importance: 1.0,
        });

        messages.push(ConversationMessage {
            role: "user".to_string(),
            content: user_message.to_string(),
            timestamp: Some(Self::get_current_timestamp()),
            importance: 1.0,
        });

        let total_tokens = self.calculate_total_tokens(&messages);
        let user_tokens = self.estimate_tokens(user_message);
        if total_tokens > available_tokens {
            return Err(anyhow::anyhow!(
                "當前訊息過長，估算 tokens={}，可用上限={}。請縮短訊息內容。",
                user_tokens,
                available_tokens
            ));
        }

        log::info!(
            "對話上下文構建完成: messages={}, total_tokens={}",
            messages.len(),
            total_tokens
        );

        Ok(ConversationContext {
            system_prompt,
            messages,
            total_tokens,
            max_context_tokens,
            available_tokens,
        })
    }

    fn get_model_context_window(&self, model: &str) -> usize {
        match model {
            m if m.contains("gpt-4o") => 128000,
            m if m.contains("gpt-4-turbo") => 128000,
            m if m.contains("gpt-4") => 8192,
            m if m.contains("gpt-3.5-turbo") => 16385,
            m if m.contains("claude-3-opus") => 200000,
            m if m.contains("claude-3-sonnet") => 200000,
            m if m.contains("claude-3-haiku") => 200000,
            m if m.contains("claude-2") => 100000,
            m if m.contains("gemini-pro") => 32768,
            m if m.contains("gemini-1.5") => 1000000,
            _ => 8192,
        }
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        let chinese_chars = text.chars().filter(|c| Self::is_cjk_char(*c)).count();
        let total_chars = text.chars().count();
        let non_chinese_chars = total_chars.saturating_sub(chinese_chars);
        let chinese_tokens = (chinese_chars as f32 / 1.5) as usize;
        let english_tokens = non_chinese_chars / 4;
        chinese_tokens + english_tokens
    }

    fn is_cjk_char(c: char) -> bool {
        matches!(
            c,
            '\u{4E00}'..='\u{9FFF}'
                | '\u{3400}'..='\u{4DBF}'
                | '\u{20000}'..='\u{2A6DF}'
                | '\u{2A700}'..='\u{2B73F}'
                | '\u{2B740}'..='\u{2B81F}'
                | '\u{2B820}'..='\u{2CEAF}'
                | '\u{F900}'..='\u{FAFF}'
        )
    }

    fn calculate_total_tokens(&self, messages: &[ConversationMessage]) -> usize {
        messages
            .iter()
            .map(|msg| self.estimate_tokens(&msg.content))
            .sum()
    }

    async fn build_system_prompt(
        &self,
        guild_id: u64,
        guild_config: &crate::models::types::GuildConfig,
    ) -> Result<String> {
        if let Some(custom_prompt) = &guild_config.custom_system_prompt {
            log::info!("使用自定義系統提示詞 for guild {}", guild_id);

            let mut prompt = custom_prompt.clone();

            let dnd_rules = &guild_config.dnd_rules;
            prompt.push_str(&format!(
                "\n\n伺服器 D&D 規則:\n- 大成功: {}\n- 大失敗: {}\n",
                dnd_rules.critical_success, dnd_rules.critical_fail
            ));

            return Ok(prompt);
        }

        let mut prompt = String::from(
            "你是一個專業的 TRPG (桌上角色扮演遊戲) 助手。\n\
             你的任務是幫助玩家和 GM (遊戲主持人) 進行遊戲。\n\
             \n\
             重要指引:\n\
             1. 保持角色扮演的氛圍和沉浸感\n\
             2. 提供有用的遊戲建議和規則解釋\n\
             3. 協助推進劇情發展\n\
             4. 回應要簡潔明瞭,避免過於冗長\n\
             5. 使用繁體中文回應\n",
        );

        let dnd_rules = &guild_config.dnd_rules;
        prompt.push_str(&format!(
            "\n\n伺服器 D&D 規則:\n- 大成功: {}\n- 大失敗: {}\n",
            dnd_rules.critical_success, dnd_rules.critical_fail
        ));

        Ok(prompt)
    }

    fn get_current_timestamp() -> String {
        let now = chrono::Local::now();
        now.format("%Y-%m-%d %H:%M:%S").to_string()
    }
}
