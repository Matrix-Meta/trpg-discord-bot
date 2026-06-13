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
