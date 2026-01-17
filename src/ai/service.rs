use std::sync::Arc;

use crate::ai::prompt::{ConversationContext, ConversationManager};
use crate::ai::providers::{
    ApiConfig, ApiManager, ChatCompletionRequest, ChatMessage, call_llm_api, get_api_key_from_env,
};
use crate::utils::config::ConfigManager;

#[derive(Debug)]
pub struct AiService {
    conversation_manager: ConversationManager,
}

impl AiService {
    pub fn new(
        config: Arc<tokio::sync::Mutex<ConfigManager>>,
        api_manager: Arc<ApiManager>,
    ) -> Self {
        Self {
            conversation_manager: ConversationManager::new(config, api_manager),
        }
    }

    pub async fn respond(
        &self,
        ctx: &poise::serenity_prelude::Context,
        msg: &poise::serenity_prelude::Message,
        user_message: &str,
        api_config: &ApiConfig,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let guild_id = msg.guild_id.ok_or("Guild not found")?.get();

        let conversation_context = self
            .conversation_manager
            .build_context(guild_id, user_message)
            .await?;

        log::info!(
            "對話上下文已構建: messages={}, tokens={}",
            conversation_context.messages.len(),
            conversation_context.total_tokens
        );

        let api_messages: Vec<ChatMessage> = conversation_context
            .messages
            .iter()
            .map(|msg| ChatMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
            })
            .collect();

        let effective_api_key = api_config
            .api_key
            .clone()
            .or_else(|| get_api_key_from_env(&api_config.provider));

        log::info!(
            "嘗試從環境變數獲取API金鑰，provider={:?}",
            api_config.provider
        );

        if effective_api_key.is_none() {
            log::warn!("伺服器 {} 沒有有效的API金鑰", guild_id);
            msg.channel_id
                .say(
                    &ctx.http,
                    "錯誤：未找到 API 金鑰。請確保已在 .env 文件中設置相應的 API 金鑰環境變數。",
                )
                .await?;
            return Err("missing api key".into());
        }

        let max_output_tokens = self.calculate_output_tokens(&conversation_context);

        let request = ChatCompletionRequest {
            model: api_config.model.clone(),
            messages: api_messages,
            temperature: Some(0.7),
            max_tokens: Some(max_output_tokens),
        };

        log::info!(
            "API請求準備就緒: model={}, messages={}, tokens={}, max_output_tokens={}",
            api_config.model,
            request.messages.len(),
            conversation_context.total_tokens,
            max_output_tokens
        );

        let _typing = msg.channel_id.start_typing(&ctx.http);
        log::info!("已開始顯示 typing 指示器");

        log::info!(
            "正在調用API: URL={}, Provider={:?}",
            api_config.api_url,
            api_config.provider
        );
        let response = call_llm_api(
            &api_config.api_url,
            effective_api_key.as_deref(),
            &request,
            &api_config.provider,
        )
        .await?;

        log::info!(
            "API回應成功，字節長度: {}, 字符長度: {}",
            response.len(),
            response.chars().count()
        );

        let limited_response = limit_response_chars(&response, 2000);
        log::info!("限制後的回應字符長度: {}", limited_response.chars().count());

        Ok(split_discord_chunks(&limited_response))
    }

    fn calculate_output_tokens(&self, context: &ConversationContext) -> u32 {
        let available = context
            .available_tokens
            .saturating_sub(context.total_tokens);
        let max_output = available.max(256).min(2048) as u32;
        max_output
    }
}

fn split_discord_chunks(content: &str) -> Vec<String> {
    const MAX_MESSAGE_LENGTH: usize = 2000;
    content
        .chars()
        .collect::<Vec<char>>()
        .chunks(MAX_MESSAGE_LENGTH)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}

fn limit_response_chars(content: &str, max_chars: usize) -> String {
    content.chars().take(max_chars).collect()
}
