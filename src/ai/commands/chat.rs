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

#[derive(ChoiceParameter, Clone, Copy, Debug)]
pub enum ApiAction {
    #[name = "add"]
    Add,
    #[name = "remove"]
    Remove,
    #[name = "toggle"]
    Toggle,
    #[name = "list-models"]
    ListModels,
    #[name = "list"]
    List,
    #[name = "switch"]
    Switch,
}

/// API 設定指令
#[poise::command(slash_command)]
pub async fn chat(
    ctx: Context<'_>,
    #[description = "操作 add、remove、toggle、list、switch 或 list-models"] action: ApiAction,
    #[description = "API URL"] api_url: Option<String>,
    #[description = "API 金鑰"] api_key: Option<String>,
    #[description = "模型名稱"] model: Option<String>,
    #[description = "API設定名稱"] name: Option<String>,
) -> Result<(), Error> {
    log::info!("執行 API 指令: {:?} for guild {:?}", action, ctx.guild_id());

    let guild_id = match ctx.guild_id() {
        Some(id) => id.get(),
        None => {
            let embed = serenity::CreateEmbed::default()
                .colour(serenity::Colour::RED)
                .description("此指令僅能在伺服器中使用");
            ctx.send(CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    };

    match action {
        ApiAction::Add => {
            let api_url = if let Some(url) = api_url {
                url
            } else {
                let embed = serenity::CreateEmbed::default()
                    .colour(serenity::Colour::RED)
                    .description("請提供 API URL");
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            };

            let test_provider = determine_provider_from_url(&api_url);
            let default_model = get_default_model_for_provider(&test_provider);
            let effective_api_key = api_key
                .clone()
                .or_else(|| get_api_key_from_env(&test_provider));

            let test_request = ChatRequest {
                model: model.clone().unwrap_or_else(|| default_model.clone()),
                system: None,
                messages: vec![ChatMessage {
                    role: Role::User,
                    content: "測試".to_string(),
                }],
                temperature: None,
                max_tokens: 10,
            };

            log::info!(
                "API 測試: URL={} Model={} Key(Present)={}",
                api_url,
                model.clone().unwrap_or_else(|| default_model.clone()),
                effective_api_key.is_some()
            );

            let call_result = timeout(
                Duration::from_secs(10),
                providers::complete(
                    &test_provider,
                    &api_url,
                    effective_api_key.as_deref(),
                    &test_request,
                ),
            )
            .await;

            match call_result {
                Ok(Ok(_)) => {
                    let provider = determine_provider_from_url(&api_url);
                    let selected_model =
                        model.unwrap_or_else(|| get_default_model_for_provider(&provider));

                    let has_command_key = api_key.is_some();
                    let has_env_key = get_api_key_from_env(&provider).is_some();

                    if let Some(ref key) = api_key {
                        save_api_key_to_env(&provider, key).await;
                    }

                    let mut api_name = api_url.clone();
                    let all_configs = guild_configs(&ctx, guild_id).await;
                    if all_configs.contains_key(&api_name) {
                        let mut counter = 1;
                        while all_configs.contains_key(&format!("{}{}", api_name, counter)) {
                            counter += 1;
                        }
                        api_name = format!("{}{}", api_name, counter);
                    }

                    let api_config = ApiConfig {
                        name: api_name,
                        api_url,
                        api_key: None,
                        model: selected_model,
                        enabled: true,
                        provider: provider.clone(),
                    };

                    ctx.data()
                        .config
                        .lock()
                        .await
                        .add_guild_api_config(guild_id, api_config)
                        .await
                        .ok();

                    let feedback_msg = if has_command_key {
                        "API 連線測試成功，已儲存設定（API 金鑰已保存到 .env 文件中）"
                    } else if has_env_key {
                        "API 連線測試成功，已儲存設定（將使用 .env 文件中的 API 金鑰）"
                    } else {
                        "API 連線測試成功，但沒有提供 API 金鑰。請在 .env 文件中設置相應的 API 金鑰環境變數。"
                    };

                    let embed = serenity::CreateEmbed::default()
                        .title("API 設定成功")
                        .description(feedback_msg)
                        .colour(serenity::Colour::DARK_GREEN);
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
                Ok(Err(e)) => {
                    let log_model = model.clone().unwrap_or_else(|| {
                        let provider = determine_provider_from_url(&api_url);
                        get_default_model_for_provider(&provider)
                    });

                    log::error!(
                        "API 測試失敗: URL={}, Model={}, Error={}",
                        api_url,
                        log_model,
                        e
                    );

                    let embed = serenity::CreateEmbed::default()
                        .title("API 設定失敗")
                        .description(format!("API 連線測試失敗: {}", e))
                        .colour(serenity::Colour::RED);
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
                Err(_) => {
                    let log_model = model.clone().unwrap_or_else(|| {
                        let provider = determine_provider_from_url(&api_url);
                        get_default_model_for_provider(&provider)
                    });

                    log::warn!("API 測試超時: URL={}, Model={}", api_url, log_model);

                    let embed = serenity::CreateEmbed::default()
                        .title("API 設定失敗")
                        .description("API 連線測試超時")
                        .colour(serenity::Colour::RED);
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
            }
        }
        ApiAction::Remove => {
            let all_configs = guild_configs(&ctx, guild_id).await;

            if name.is_none() && all_configs.len() > 1 {
                let embed = serenity::CreateEmbed::default()
                    .title("多個API設定")
                    .description("此伺服器有多個API設定。請使用 `/chat list` 查看所有設定，並指定要刪除的設定名稱。\n範例: /chat remove name:設定名稱")
                    .colour(serenity::Colour::ORANGE);
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            let api_name_to_remove = if let Some(ref specified_name) = name {
                specified_name.clone()
            } else {
                let active_config = active_config(&ctx, guild_id).await;
                active_config.name
            };

            let success = ctx
                .data()
                .config
                .lock()
                .await
                .remove_guild_api_config(guild_id, &api_name_to_remove)
                .await
                .unwrap_or(false);

            if success {
                let embed = serenity::CreateEmbed::default()
                    .title("API 設定已移除")
                    .description(format!(
                        "已清除此伺服器的 '{}' API 設定",
                        api_name_to_remove
                    ))
                    .colour(serenity::Colour::DARK_GREEN);
                ctx.send(CreateReply::default().embed(embed)).await?;
            } else {
                let embed = serenity::CreateEmbed::default()
                    .title("API 設定移除失敗")
                    .description(format!("沒有找到名為 '{}' 的 API 設定", api_name_to_remove))
                    .colour(serenity::Colour::RED);
                ctx.send(CreateReply::default().embed(embed)).await?;
            }
        }
        ApiAction::Toggle => {
            let all_configs = guild_configs(&ctx, guild_id).await;

            if all_configs.is_empty() {
                let embed = serenity::CreateEmbed::default()
                    .title("錯誤")
                    .description("此伺服器沒有設定任何API配置")
                    .colour(serenity::Colour::RED);
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            let target_name = if let Some(ref specified_name) = name {
                specified_name.clone()
            } else {
                let active_config = active_config(&ctx, guild_id).await;
                active_config.name
            };

            if let Some(mut config) = all_configs.get(&target_name).cloned() {
                let was_enabled = config.enabled;
                config.enabled = !was_enabled;
                ctx.data()
                    .config
                    .lock()
                    .await
                    .add_guild_api_config(guild_id, config)
                    .await
                    .ok();

                let status = if !was_enabled {
                    "已啟用"
                } else {
                    "已停用"
                };
                let embed = serenity::CreateEmbed::default()
                    .title("API 狀態切換")
                    .description(format!("API '{}' 已{}", target_name, status))
                    .colour(serenity::Colour::BLURPLE);
                ctx.send(CreateReply::default().embed(embed)).await?;
            } else {
                let embed = serenity::CreateEmbed::default()
                    .title("錯誤")
                    .description(format!(
                        "找不到名為 '{}' 的API設定。請使用 `/chat list` 查看可用設定。",
                        target_name
                    ))
                    .colour(serenity::Colour::RED);
                ctx.send(CreateReply::default().embed(embed)).await?;
            }
        }
        ApiAction::ListModels => {
            let current_config = active_config(&ctx, guild_id).await;
            let effective_api_key = current_config
                .api_key
                .clone()
                .or_else(|| get_api_key_from_env(&current_config.provider));

            if effective_api_key.is_none() {
                let embed = serenity::CreateEmbed::default()
                    .title("模型列表")
                    .description("此伺服器尚未設定 API 金鑰，無法獲取模型列表。")
                    .colour(serenity::Colour::RED);
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            let api_key = effective_api_key.as_ref().unwrap();

            match providers::list_models(
                &current_config.provider,
                &current_config.api_url,
                Some(api_key),
            )
            .await
            {
                Ok(models_list) => {
                    if !models_list.is_empty() {
                        let models_to_show = if models_list.len() > 50 {
                            format!("顯示前 50 個模型（共 {} 個）：\n", models_list.len())
                        } else {
                            String::new()
                        };

                        let models_str = models_list
                            .iter()
                            .take(50)
                            .map(|model| format!("- {}", model))
                            .collect::<Vec<_>>()
                            .join("\n");

                        let full_description = format!("{}{}", models_to_show, models_str);

                        let embed = serenity::CreateEmbed::default()
                            .title("可用模型列表")
                            .description(full_description)
                            .colour(serenity::Colour::BLURPLE);
                        ctx.send(CreateReply::default().embed(embed)).await?;
                    } else {
                        let embed = serenity::CreateEmbed::default()
                            .title("模型列表")
                            .description("API 回應中沒有模型數據。")
                            .colour(serenity::Colour::ORANGE);
                        ctx.send(CreateReply::default().embed(embed)).await?;
                    }
                }
                Err(_) => {
                    let embed = serenity::CreateEmbed::default()
                        .title("可用模型")
                        .description(format!(
                            "無法從 API 獲取模型列表。\n當前設定的模型: {}",
                            current_config.model
                        ))
                        .colour(serenity::Colour::ORANGE);
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
            }
        }
        ApiAction::List => {
            let all_configs = guild_configs(&ctx, guild_id).await;

            let data = ctx.data();
            let config_guard = data.config.lock().await;
            let guilds_read = config_guard.guilds.read().await;
            let active_api = if let Some(guild_config) = guilds_read.get(&guild_id) {
                guild_config.active_api.clone().unwrap_or_default()
            } else {
                String::new()
            };
            drop(guilds_read);
            drop(config_guard);

            if all_configs.is_empty() {
                let embed = serenity::CreateEmbed::default()
                    .title("API設定列表")
                    .description("此伺服器尚未設定任何API。")
                    .colour(serenity::Colour::ORANGE);
                ctx.send(CreateReply::default().embed(embed)).await?;
            } else {
                let mut description = String::new();
                for (name, config) in &all_configs {
                    let status = if config.enabled { "✅" } else { "❌" };
                    let active_marker = if name == &active_api { " 🌟" } else { "" };
                    let provider_debug = format!("{:?}", config.provider);
                    description.push_str(&format!(
                        "{} **{}**{} - {} ({})\n",
                        status, name, active_marker, config.model, provider_debug
                    ));
                }

                let embed = serenity::CreateEmbed::default()
                    .title("API設定列表")
                    .description(description)
                    .colour(serenity::Colour::BLURPLE);
                ctx.send(CreateReply::default().embed(embed)).await?;
            }
        }
        ApiAction::Switch => {
            let all_configs = guild_configs(&ctx, guild_id).await;

            if all_configs.is_empty() {
                let embed = serenity::CreateEmbed::default()
                    .title("錯誤")
                    .description("此伺服器沒有設定任何API配置")
                    .colour(serenity::Colour::RED);
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            }

            if let Some(ref target_name) = name {
                if all_configs.contains_key(target_name) {
                    let success = ctx
                        .data()
                        .config
                        .lock()
                        .await
                        .set_active_api(guild_id, target_name)
                        .await
                        .unwrap_or(false);

                    if success {
                        let embed = serenity::CreateEmbed::default()
                            .title("API 切換成功")
                            .description(format!("已切換到 '{}' API 設定", target_name))
                            .colour(serenity::Colour::DARK_GREEN);
                        ctx.send(CreateReply::default().embed(embed)).await?;
                    } else {
                        let embed = serenity::CreateEmbed::default()
                            .title("API 切換失敗")
                            .description(format!("無法切換到 '{}' API 設定", target_name))
                            .colour(serenity::Colour::RED);
                        ctx.send(CreateReply::default().embed(embed)).await?;
                    }
                } else {
                    let embed = serenity::CreateEmbed::default()
                        .title("錯誤")
                        .description(format!(
                            "找不到名為 '{}' 的API設定。請使用 `/chat list` 查看可用設定。",
                            target_name
                        ))
                        .colour(serenity::Colour::RED);
                    ctx.send(CreateReply::default().embed(embed)).await?;
                }
            } else {
                let mut description = String::new();
                for (name, config) in &all_configs {
                    let status = if config.enabled { "✅" } else { "❌" };
                    let provider_debug = format!("{:?}", config.provider);
                    description.push_str(&format!(
                        "{} **{}** - {} ({})\n",
                        status, name, config.model, provider_debug
                    ));
                }

                let embed = serenity::CreateEmbed::default()
                    .title("可用的API設定")
                    .description(
                        "請使用指令指定要切換到的API設定名稱。\n範例: /chat switch name:設定名稱",
                    )
                    .field("設定列表", description, false)
                    .colour(serenity::Colour::BLURPLE);
                ctx.send(CreateReply::default().embed(embed)).await?;
            }
        }
    }

    Ok(())
}

async fn save_api_key_to_env(provider: &ApiProvider, key: &str) {
    let env_path = Path::new(".env");

    let env_content = if env_path.exists() {
        std::fs::read_to_string(env_path).unwrap_or_default()
    } else {
        String::new()
    };

    let var = env_var_name(provider);

    let mut lines: Vec<String> = env_content.lines().map(|s| s.to_string()).collect();
    let mut found = false;

    for line in &mut lines {
        if line.starts_with(&format!("{}=", var)) {
            *line = format!("{}={}", var, key);
            found = true;
            break;
        }
    }

    if !found {
        lines.push(format!("{}={}", var, key));
    }

    if let Ok(mut file) = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(env_path)
    {
        let new_content = lines.join("\n");
        let _ = file.write_all(new_content.as_bytes());
    }
}

async fn guild_configs(
    ctx: &Context<'_>,
    guild_id: u64,
) -> std::collections::HashMap<String, ApiConfig> {
    ctx.data()
        .config
        .lock()
        .await
        .get_guild_api_configs(guild_id)
        .await
}

async fn active_config(ctx: &Context<'_>, guild_id: u64) -> ApiConfig {
    ctx.data()
        .config
        .lock()
        .await
        .get_guild_api_config(guild_id)
        .await
}
