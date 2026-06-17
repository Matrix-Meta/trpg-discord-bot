mod ai;
mod bot;
mod models;
mod utils;

use std::env;
use std::sync::Arc;

use anyhow::anyhow;
use poise::serenity_prelude as serenity;
use tokio::sync::Mutex;

use crate::ai::service::AiService;
use crate::bot::data::BotData;
use crate::utils::config::ConfigManager;

#[tokio::main]
async fn main() -> Result<(), bot::Error> {
    if let Err(e) = utils::logger::DiscordLogger::init(Some("bot.log")) {
        eprintln!("日誌初始化失敗: {}", e);
    }

    dotenvy::dotenv().ok();

    // 啟動 .env 熱載入監視器
    let _env_watcher = utils::env_watcher::EnvWatcher::new(".env")
        .map_err(|e| anyhow!("環境變數監視器初始化失敗: {}", e))?;

    let token =
        env::var("DISCORD_TOKEN").map_err(|_| anyhow!("預期 DISCORD_TOKEN 環境變數，但找不到!"))?;

    let config_manager = ConfigManager::new("config.json")
        .await
        .map_err(|e| anyhow!("設定管理器初始化失敗: {}", e))?;
    let shared_config = Arc::new(Mutex::new(config_manager));
    // 下面開始建立並初始化資料庫
    let skills_db = tokio_rusqlite::Connection::open("skills.db")
        .await
        .map_err(|e| anyhow!("開啟技能資料庫失敗: {}", e))?;
    skills_db
        .call(|conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS skills (
                    guild_id INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    normalized_name TEXT NOT NULL,
                    skill_type TEXT NOT NULL,
                    level TEXT NOT NULL,
                    effect TEXT NOT NULL,
                    occupation TEXT DEFAULT '',
                    race TEXT DEFAULT '',
                    UNIQUE(guild_id, normalized_name)
                )",
                [],
            )?;

            Ok(())
        })
        .await
        .map_err(|e| anyhow!("初始化技能資料庫失敗: {}", e))?;

    let base_settings_db = tokio_rusqlite::Connection::open("base_settings.db")
        .await
        .map_err(|e| anyhow!("開啟基本設定資料庫失敗: {}", e))?;
    // base_settings.db 現在用於存儲導入的數據表，無需預設表結構
    // 導入功能將根據數據類型自動創建對應的表
    base_settings_db
        .call(|conn| {
            // 確保資料庫連接正常
            conn.execute("CREATE TABLE IF NOT EXISTS __temp_check (id INTEGER)", [])
                .map_err(tokio_rusqlite::Error::Rusqlite)?;
            conn.execute("DROP TABLE IF EXISTS __temp_check", [])
                .map_err(tokio_rusqlite::Error::Rusqlite)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow!("初始化基本設定資料庫失敗: {}", e))?;

    let intents = serenity::GatewayIntents::GUILDS
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MESSAGES;

    let setup_config = Arc::clone(&shared_config);

    let setup_skills_db = skills_db.clone();
    let setup_base_settings_db = base_settings_db.clone();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: crate::bot::commands(),
            on_error: |error| {
                Box::pin(async move {
                    log::error!("指令執行錯誤: {}", error);

                    // 嘗試獲取具體的錯誤資訊
                    let error_msg = format!("發生錯誤: {}", error);

                    // 如果有互動回應，向使用者發送錯誤訊息
                    if let poise::FrameworkError::Command { ctx, .. } = error {
                        if let Err(why) = ctx.say(error_msg).await {
                            log::error!("發送錯誤訊息失敗: {}", why);
                        }
                    }
                })
            },
            event_handler: |_ctx, event, _framework, _data| {
                Box::pin(async move {
                    // 在poise中，事件類型是FullEvent，需要使用適當的方法來獲取消息
                    use poise::serenity_prelude::FullEvent;

                    if let FullEvent::Message {
                        new_message: message,
                    } = event
                    {
                        // 只檢查是否標記機器人
                        let is_mentioned = message
                            .mentions
                            .iter()
                            .any(|user| user.id == _ctx.cache.current_user().id);

                        // 只在被提及時記錄日誌
                        if is_mentioned {
                            log::info!(
                                "訊息事件處理: is_mentioned=true, content='{}'",
                                message.content
                            );
                        }

                        if is_mentioned {
                            // 處理與AI的對話
                            log::info!("觸發AI對話處理");
                            handle_message(_ctx, message, _data).await;
                        }
                    }
                    Ok(())
                })
            },
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let config = Arc::clone(&setup_config);
            let skills_db = setup_skills_db.clone();
            let base_settings_db = setup_base_settings_db.clone();
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;

                let ai_service = Arc::new(AiService::new(config.clone()));

                println!("{} 已經上線!", ready.user.name);
                Ok(BotData {
                    config,
                    ai_service,
                    skills_db,
                    base_settings_db,
                })
            })
        })
        .build();

    let mut client = serenity::Client::builder(&token, intents)
        .framework(framework)
        .await
        .map_err(|e| anyhow!("建立 Discord 客戶端失敗: {}", e))?;

    client
        .start()
        .await
        .map_err(|e| anyhow!("機器人啟動失敗: {}", e))?;

    Ok(())
}

async fn handle_message(ctx: &serenity::Context, msg: &serenity::Message, data: &BotData) {
    // 檢查此頻道是否在伺服器中（不處理私訊）
    if msg.guild_id.is_none() {
        if let Err(e) = msg
            .channel_id
            .say(&ctx.http, "抱歉，AI對話功能僅在伺服器中可用。")
            .await
        {
            log::error!("發送訊息失敗: {:?}", e);
        }
        return;
    }

    let guild_id = msg.guild_id.unwrap().get();
    let channel_id = msg.channel_id.get();

    log::info!(
        "收到訊息，Guild ID: {}, Channel ID: {}, Author: {}",
        guild_id,
        channel_id,
        msg.author.name
    );

    // 獲取該伺服器的API配置
    let api_config = data.config.lock().await.get_guild_api_config(guild_id).await;
    log::info!(
        "API Config for guild {}: enabled={}, has_api_key={}, provider={:?}",
        guild_id,
        api_config.enabled,
        api_config.api_key.is_some(),
        api_config.provider
    );

    if !api_config.enabled {
        log::info!("伺服器 {} 的AI功能未啟用", guild_id);
        if let Err(e) = msg
            .channel_id
            .say(
                &ctx.http,
                "此伺服器尚未啟用AI對話功能。請使用 `/chat add` 指令設定API。",
            )
            .await
        {
            log::error!("發送訊息失敗: {:?}", e);
        }
        return;
    }

    // 準備用戶消息內容
    let mut user_message = remove_bot_mention(&msg.content, ctx.cache.current_user().id);

    // 如果當前消息是對其他消息的回覆，將被回覆的消息內容加入上下文
    if let Some(referenced) = &msg.referenced_message {
        let replied_context = format!(
            "[回覆 {}: {}]\n{}",
            referenced.author.name, referenced.content, user_message
        );
        user_message = replied_context;
    }

    // 使用 ConversationManager 構建對話上下文
    match data
        .ai_service
        .respond(ctx, msg, &user_message, &api_config)
        .await
    {
        Ok(chunks) => {
            for (i, chunk) in chunks.iter().enumerate() {
                log::info!("發送回應部分 {}: 字符長度 {}", i + 1, chunk.chars().count());
                if let Err(e) = msg.channel_id.say(&ctx.http, chunk).await {
                    log::error!("發送訊息失敗: {:?}", e);
                }
            }
        }
        Err(e) => {
            log::error!("API調用失敗: {:?}", e);
            if let Err(e) = msg
                .channel_id
                .say(&ctx.http, format!("API調用失敗: {:?}", e))
                .await
            {
                log::error!("發送錯誤訊息失敗: {:?}", e);
            }
        }
    }
}

// 判斷字符是否為中文字符
fn remove_bot_mention(content: &str, bot_id: serenity::UserId) -> String {
    let bot_mention = format!("<@{}>", bot_id);
    let bot_mention_nick = format!("<@!{}>", bot_id); // With nickname
    content
        .replace(&bot_mention, "")
        .replace(&bot_mention_nick, "")
        .trim_start()
        .to_string()
}
