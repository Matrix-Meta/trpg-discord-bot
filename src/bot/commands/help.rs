use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::bot::{Context, Error};
use poise::{
    CreateReply,
    serenity_prelude::{self as serenity, CreateActionRow, CreateButton},
};

/// 顯示指令說明
#[poise::command(slash_command)]
pub async fn help(ctx: Context<'_>) -> Result<(), Error> {
    log::info!("執行 help 指令 for guild {:?}", ctx.guild_id());

    let embed = serenity::CreateEmbed::default()
        .title("TRPG Discord Bot 指令說明")
        .description(
            "請點擊下方按鈕查看各指令的詳細說明。\n支援 `/roll`、`/coc`、`/skill add`、`/skill show`、`/log-stream`、`/log-stream-mode`、`/crit`、`/admin`。",
        )
        .colour(serenity::Colour::FOOYOO);

    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new("help_roll")
            .label("D&D 擲骰")
            .style(serenity::ButtonStyle::Primary),
        CreateButton::new("help_coc")
            .label("CoC 擲骰")
            .style(serenity::ButtonStyle::Primary),
        CreateButton::new("help_skill")
            .label("技能指令")
            .style(serenity::ButtonStyle::Primary),
        CreateButton::new("help_logs")
            .label("日誌指令")
            .style(serenity::ButtonStyle::Secondary),
        CreateButton::new("help_admin")
            .label("管理指令")
            .style(serenity::ButtonStyle::Secondary),
    ])];

    let reply = CreateReply::default().embed(embed).components(components);

    let sent = ctx.send(reply).await?;
    let message = Arc::new(sent.into_message().await?);
    let author_id = ctx.author().id;

    let mut details = HashMap::new();
    details.insert(
        "help_roll",
        "**/roll <骰子表達式>**\n支援 `2d6`、`d20+5`、`1d10>=15`、`+3 d6` 等格式，解析骰數、面數、修正值與比較條件。預設最多 50 次擲骰。",
    );
    details.insert(
        "help_coc",
        "**/coc <技能值> [次數]**\n技能值 1-100，可設定 1-10 次連續擲骰。自動判斷普通/困難/極限成功、大成功（1）與大失敗（技能<50 時 96-100，否則 100）。",
    );
    details.insert(
        "help_logs",
        "**日誌相關指令**\n`/log-stream on <頻道>`：啟用串流並綁定頻道。\n`/log-stream off`：關閉串流。\n`/log-stream-mode <live|batch>`：切換即時或批次。\n`/crit <success|fail> [頻道]`：設定大成功/大失敗紀錄頻道，留空則清除設定。",
    );
    details.insert(
        "help_admin",
        "**管理指令（需開發者）**\n`/admin restart`：確認後重新啟動機器人。\n`/admin shutdown`：確認後關閉機器人。\n`/admin dev-add <用戶>` / `/admin dev-remove <用戶>`：維護開發者名單。\n`/admin dev-list`：列出所有已註冊開發者。",
    );
    details.insert(
        "help_skill",
        "**技能指令**\n`/skill add <名稱> <類型> <等級> <效果>`：新增或更新技能紀錄。\n`/skill show <名稱>`：支援模糊搜尋技能名稱，查詢技能。\n`/skill delete <名稱>`：刪除此伺服器中的技能。",
    );

    let details = Arc::new(details);
    let ctx_clone = ctx.serenity_context().clone();
    let message_handle = Arc::clone(&message);
    let details_handle = Arc::clone(&details);

    tokio::spawn(async move {
        loop {
            let interaction = message_handle
                .await_component_interaction(&ctx_clone)
                .timeout(Duration::from_secs(120))
                .author_id(author_id)
                .await;

            let Some(interaction) = interaction else {
                break;
            };

            if let Some(detail) = details_handle.get(interaction.data.custom_id.as_str()) {
                let detail_embed = serenity::CreateEmbed::default()
                    .title("指令說明")
                    .description(detail.to_string())
                    .colour(serenity::Colour::FOOYOO);
                let message = serenity::CreateInteractionResponseMessage::default()
                    .ephemeral(true)
                    .add_embed(detail_embed);
                let response = serenity::CreateInteractionResponse::Message(message);
                if let Err(e) = interaction.create_response(&ctx_clone, response).await {
                    log::error!("回應 help 按鈕互動時發生錯誤: {:?}", e);
                }
            }
        }
    });

    Ok(())
}
