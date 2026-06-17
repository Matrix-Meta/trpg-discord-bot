use crate::bot::{Context, Error};
use poise::{
    ChoiceParameter, CreateReply,
    serenity_prelude::{
        self as serenity, ButtonStyle, CreateActionRow, CreateButton, CreateInteractionResponse,
        CreateInteractionResponseMessage,
    },
};
use rand::random;
use std::time::Duration;
use tokio::{process::Command as TokioCommand, time::sleep};

// 定義 ProcessControl 枚舉
#[derive(Clone)]
enum ProcessControl {
    Execv,
    Service { name: String },
}

#[derive(Clone, Copy, Debug, ChoiceParameter)]
pub enum AdminAction {
    #[name = "restart"]
    Restart,
    #[name = "shutdown"]
    Shutdown,
    #[name = "dev-add"]
    DevAdd,
    #[name = "dev-remove"]
    DevRemove,
    #[name = "dev-list"]
    DevList,
}

/// 管理指令
#[poise::command(slash_command)]
pub async fn admin(
    ctx: Context<'_>,
    #[description = "管理操作"] action: AdminAction,
    #[description = "要添加或移除的開發者"] user: Option<serenity::User>,
) -> Result<(), Error> {
    log::info!(
        "執行管理指令: {:?} for user {:?}, guild {:?}",
        action,
        ctx.author().id,
        ctx.guild_id()
    );

    let caller_id = ctx.author().id.get();

    let has_permission = {
        let config_manager = ctx.data().config.lock().await;
        config_manager.is_developer(caller_id).await
    };

    if !has_permission {
        log::warn!("用戶 {:?} 嘗試執行管理指令但沒有權限", ctx.author().id);
        ctx.say("您沒有權限執行此操作！").await?;
        return Ok(());
    }

    match action {
        AdminAction::Restart => {
            if !confirm_action(&ctx, "確認執行重啟操作？").await? {
                log::info!("用戶 {:?} 取消重啟操作", ctx.author().id);
                return Ok(());
            }
            let control = match process_control_from_config(&ctx).await {
                Ok(control) => control,
                Err(e) => {
                    log::error!("配置加載錯誤: {:?}", e);
                    ctx.say("配置加載錯誤").await?;
                    return Ok(());
                }
            };
            log::info!("用戶 {:?} 確認執行重啟操作", ctx.author().id);
            ctx.say("已確認，機器人即將重新啟動……").await?;
            schedule_restart(control).await?;
        }
        AdminAction::Shutdown => {
            if !confirm_action(&ctx, "確認關閉機器人？").await? {
                log::info!("用戶 {:?} 取消關閉操作", ctx.author().id);
                return Ok(());
            }
            let control = match process_control_from_config(&ctx).await {
                Ok(control) => control,
                Err(e) => {
                    log::error!("配置加載錯誤: {:?}", e);
                    ctx.say("配置加載錯誤").await?;
                    return Ok(());
                }
            };
            log::info!("用戶 {:?} 確認執行關閉操作", ctx.author().id);
            ctx.say("已確認，機器人即將關閉……").await?;
            schedule_shutdown(control).await?;
        }
        AdminAction::DevAdd => {
            let user = match user {
                Some(u) => u,
                None => {
                    ctx.say("請指定要添加的用戶！").await?;
                    return Ok(());
                }
            };

            if !confirm_action(&ctx, format!("確認將 <@{}> 新增為開發者？", user.id)).await?
            {
                log::info!("用戶 {:?} 取消添加開發者操作", ctx.author().id);
                return Ok(());
            }

            let config_manager = ctx.data().config.lock().await;
            match config_manager.add_developer(user.id.get()).await {
                Ok(success) => {
                    if success {
                        log::info!("用戶 {:?} 已添加到開發者列表", user.id);
                        ctx.say(format!("用戶 <@{}> 已添加到開發者列表", user.id))
                            .await?;
                    } else {
                        log::info!("用戶 {:?} 已經是開發者", user.id);
                        ctx.say(format!("用戶 <@{}> 已經是開發者", user.id)).await?;
                    }
                }
                Err(e) => {
                    log::error!("添加開發者時發生錯誤: {:?}", e);
                    ctx.say("添加開發者時發生錯誤").await?;
                    return Err(e.into());
                }
            }
        }
        AdminAction::DevRemove => {
            let user = match user {
                Some(u) => u,
                None => {
                    ctx.say("請指定要移除的用戶！").await?;
                    return Ok(());
                }
            };

            if !confirm_action(&ctx, format!("確認將 <@{}> 從開發者列表移除？", user.id)).await?
            {
                log::info!("用戶 {:?} 取消移除開發者操作", ctx.author().id);
                return Ok(());
            }

            let config_manager = ctx.data().config.lock().await;
            match config_manager.remove_developer(user.id.get()).await {
                Ok(success) => {
                    if success {
                        log::info!("用戶 {:?} 已從開發者列表移除", user.id);
                        ctx.say(format!("用戶 <@{}> 已從開發者列表移除", user.id))
                            .await?;
                    } else {
                        log::info!("用戶 {:?} 不在開發者列表中", user.id);
                        ctx.say(format!("用戶 <@{}> 不在開發者列表中", user.id))
                            .await?;
                    }
                }
                Err(e) => {
                    log::error!("移除開發者時發生錯誤: {:?}", e);
                    ctx.say("移除開發者時發生錯誤").await?;
                    return Err(e.into());
                }
            }
        }
        AdminAction::DevList => {
            let config_manager = ctx.data().config.lock().await;
            let global_config = config_manager.get_global_config().await;
            let developers = &global_config.developers;
            if developers.is_empty() {
                log::info!("查詢開發者列表，結果為空");
                ctx.say("目前沒有開發者").await?;
            } else {
                log::info!("查詢開發者列表，共有 {} 位開發者", developers.len());
                let mut list = String::from("開發者列表:\n");
                for dev_id in developers {
                    list.push_str(&format!("<@{}>\n", dev_id));
                }
                ctx.say(list).await?;
            }
        }
    }

    Ok(())
}

async fn confirm_action(ctx: &Context<'_>, prompt: impl Into<String>) -> Result<bool, Error> {
    let prompt = prompt.into();
    let nonce: u64 = random();
    let confirm_id = format!("admin_confirm:{}:{}", ctx.author().id, nonce);
    let cancel_id = format!("admin_cancel:{}:{}", ctx.author().id, nonce);
    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(confirm_id.clone())
            .label("確認")
            .style(ButtonStyle::Primary),
        CreateButton::new(cancel_id.clone())
            .label("取消")
            .style(ButtonStyle::Secondary),
    ])];

    let reply = CreateReply::default()
        .content(prompt)
        .components(components)
        .ephemeral(true);
    let sent = ctx.send(reply).await?;
    let mut message = sent.into_message().await?;
    let ctx_clone = ctx.serenity_context().clone();
    let author_id = ctx.author().id;

    let interaction = message
        .await_component_interaction(&ctx_clone)
        .author_id(author_id)
        .timeout(Duration::from_secs(30))
        .await;

    match interaction {
        Some(interaction) if interaction.data.custom_id == confirm_id => {
            let mut response = CreateInteractionResponseMessage::default();
            response = response.content("已確認").components(Vec::new());
            interaction
                .create_response(
                    &ctx_clone,
                    CreateInteractionResponse::UpdateMessage(response),
                )
                .await?;
            Ok(true)
        }
        Some(interaction) => {
            let mut response = CreateInteractionResponseMessage::default();
            response = response.content("操作已取消").components(Vec::new());
            interaction
                .create_response(
                    &ctx_clone,
                    CreateInteractionResponse::UpdateMessage(response),
                )
                .await?;
            Ok(false)
        }
        None => {
            let edit = serenity::builder::EditMessage::new()
                .content("操作逾時，未執行任何變更")
                .components(Vec::new());
            let _ = message.edit(&ctx_clone.http, edit).await;
            Ok(false)
        }
    }
}

async fn schedule_restart(control: ProcessControl) -> Result<(), Error> {
    match control {
        ProcessControl::Execv => {
            tokio::spawn(async {
                sleep(Duration::from_millis(500)).await;
                #[cfg(target_family = "unix")]
                {
                    use std::env;
                    use std::os::unix::process::CommandExt;
                    let exe = match env::current_exe() {
                        Ok(path) => path,
                        Err(e) => {
                            eprintln!("Failed to get current executable path: {}", e);
                            std::process::exit(1);
                        }
                    };
                    let args: Vec<String> = env::args().collect();
                    let _ = std::process::Command::new(exe).args(&args[1..]).exec();
                }

                #[cfg(target_family = "windows")]
                {
                    // Windows doesn't have execv, so we'll spawn a new process and exit the current one
                    use std::env;
                    let exe = match env::current_exe() {
                        Ok(path) => path,
                        Err(e) => {
                            eprintln!("Failed to get current executable path: {}", e);
                            std::process::exit(1);
                        }
                    };
                    let args: Vec<String> = env::args().collect();
                    if let Err(e) = std::process::Command::new(exe).args(&args[1..]).spawn() {
                        eprintln!("Failed to spawn new process: {}", e);
                    }
                }
                std::process::exit(0);
            });
        }
        ProcessControl::Service { name } => {
            tokio::spawn(async move {
                sleep(Duration::from_millis(500)).await;
                #[cfg(target_family = "windows")]
                {
                    match TokioCommand::new("sc").args(["stop", &name]).status().await {
                        Ok(_) => {
                            match TokioCommand::new("sc")
                                .args(["start", &name])
                                .status()
                                .await
                            {
                                Ok(_) => std::process::exit(0),
                                Err(err) => {
                                    eprintln!("服務重啟失敗: {}", err);
                                    std::process::exit(1);
                                }
                            }
                        }
                        Err(err) => {
                            eprintln!("服務停止失敗: {}", err);
                            std::process::exit(1);
                        }
                    }
                }

                #[cfg(target_family = "unix")]
                {
                    match TokioCommand::new("systemctl")
                        .arg("restart")
                        .arg(&name)
                        .status()
                        .await
                    {
                        Ok(status) if status.success() => std::process::exit(0),
                        Ok(status) => {
                            eprintln!(
                                "systemctl restart {} 失敗，狀態碼 {:?}",
                                name,
                                status.code()
                            );
                            std::process::exit(status.code().unwrap_or(1));
                        }
                        Err(err) => {
                            eprintln!("systemctl restart {} 執行失敗: {}", name, err);
                            std::process::exit(1);
                        }
                    }
                }
            });
        }
    }
    Ok(())
}

async fn schedule_shutdown(control: ProcessControl) -> Result<(), Error> {
    match control {
        ProcessControl::Execv => {
            // 延遲後退出程序，讓響應能發送出去
            tokio::spawn(async {
                sleep(Duration::from_millis(500)).await;
                std::process::exit(0);
            });
            Ok(()) // 返回 Ok(())
        }
        ProcessControl::Service { name } => {
            // 在服務模式下，使用系統服務管理器關閉服務
            // 使用 tokio::spawn 在後台執行，避免阻塞當前函數
            tokio::spawn(async move {
                #[cfg(target_family = "windows")]
                {
                    match TokioCommand::new("sc").args(["stop", &name]).status().await {
                        Ok(_) => std::process::exit(0),
                        Err(err) => {
                            eprintln!("服務停止失敗: {}", err);
                            std::process::exit(1);
                        }
                    }
                }

                #[cfg(target_family = "unix")]
                {
                    match TokioCommand::new("systemctl")
                        .arg("stop")
                        .arg(&name)
                        .status()
                        .await
                    {
                        Ok(status) if status.success() => std::process::exit(0),
                        Ok(status) => {
                            eprintln!("systemctl stop {} 失敗，狀態碼 {:?}", name, status.code());
                            std::process::exit(status.code().unwrap_or(1));
                        }
                        Err(err) => {
                            eprintln!("systemctl stop {} 執行失敗: {}", name, err);
                            std::process::exit(1);
                        }
                    }
                }
            });
            Ok(()) // 返回 Ok(())
        }
    }
}

async fn process_control_from_config(ctx: &Context<'_>) -> Result<ProcessControl, Error> {
    let config_manager = ctx.data().config.lock().await;
    let global_config = config_manager.get_global_config().await;

    if global_config.restart_mode == "service" {
        if let Some(service_name) = &global_config.restart_service {
            Ok(ProcessControl::Service {
                name: service_name.clone(),
            })
        } else {
            Err(anyhow::anyhow!(
                "restart_mode 為 service 時，必須設定 restart_service"
            ))
        }
    } else {
        // 預設使用 execv 模式
        Ok(ProcessControl::Execv)
    }
}
