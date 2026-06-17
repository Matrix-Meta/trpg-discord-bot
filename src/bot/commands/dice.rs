use crate::bot::{Context, Error};
use crate::models::types::RollResult;
use crate::utils::coc::{determine_success_level, format_success_level, roll_coc_multi};
use crate::utils::dice::roll_multiple_dice;
use poise::{ChoiceParameter, CreateReply, serenity_prelude as serenity};
use serenity::model::prelude::Mentionable;

#[derive(ChoiceParameter, Clone, Copy, Debug)]
pub enum DiceMode {
    #[name = "dnd"]
    Dnd,
    #[name = "coc"]
    Coc,
}

/// 擲骰子指令 - 支援 D&D 和 CoC 7e
#[poise::command(slash_command)]
pub async fn dice(
    ctx: Context<'_>,
    #[description = "骰子模式 (dnd 或 coc)"] mode: DiceMode,
    #[description = "D&D: 骰子表達式 (例如: 2d20+5) / CoC: 技能值 (1-100)"] value: String,
    #[description = "CoC 模式: 擲骰次數 (1-100)"]
    #[min = 1]
    #[max = 100]
    times: Option<u8>,
    #[description = "附註/描述 (選填)"] description: Option<String>,
) -> Result<(), Error> {
    match mode {
        DiceMode::Dnd => roll_dnd(ctx, value, description).await,
        DiceMode::Coc => {
            let skill = value
                .parse::<u8>()
                .map_err(|_| anyhow::anyhow!("CoC 模式需要輸入 1-100 的技能值"))?;
            if !(1..=100).contains(&skill) {
                return Err(anyhow::anyhow!("技能值必須在 1-100 之間"));
            }
            roll_coc_impl(ctx, skill, times, description).await
        }
    }
}

async fn roll_dnd(
    ctx: Context<'_>,
    expression: String,
    description: Option<String>,
) -> Result<(), Error> {
    log::info!(
        "執行 D&D 擲骰: {} for guild {:?}",
        expression,
        ctx.guild_id()
    );

    let rules = {
        let data = ctx.data();
        let config_handle = data.config.lock().await;
        let guild_id = ctx.guild_id().map(|id| id.get());
        let guild_config = if let Some(id) = guild_id {
            config_handle.get_guild_config(id).await
        } else {
            Default::default()
        };
        guild_config.dnd_rules
    };

    match roll_multiple_dice(&expression, rules.max_dice_count, &rules) {
        Ok(results) => {
            let guild_id = ctx.guild_id();
            let author = ctx.author().clone();
            let crit_events = if guild_id.is_some() {
                collect_dnd_critical_events(&results, &expression, &author, ctx.channel_id())
            } else {
                Vec::new()
            };

            if results.len() == 1 {
                let content =
                    with_user_note(format_roll_result(&results[0]), description.as_deref());
                send_embed(&ctx, "D&D 擲骰結果", content).await?;
            } else {
                let content = with_user_note(
                    format_multiple_roll_results(&results),
                    description.as_deref(),
                );
                send_embed(&ctx, "D&D 連續擲骰結果", content).await?;
            }

            if let Some(guild_id) = guild_id {
                log_critical_events(&ctx, guild_id, crit_events).await?;
            }
        }
        Err(e) => {
            log::error!("D&D 擲骰錯誤: {} - 表達式: {}", e, expression);
            send_embed(&ctx, "D&D 擲骰錯誤", format!("錯誤: {}", e)).await?;
        }
    }

    Ok(())
}

async fn roll_coc_impl(
    ctx: Context<'_>,
    skill: u8,
    times: Option<u8>,
    description: Option<String>,
) -> Result<(), Error> {
    log::info!("執行 CoC 擲骰: 技能值={}, 次數={:?}", skill, times);

    let guild_id = match ctx.guild_id() {
        Some(id) => id.get(),
        None => {
            ctx.say("此指令只能在伺服器中使用").await?;
            return Ok(());
        }
    };

    let rules = {
        let data = ctx.data();
        let config_handle = data.config.lock().await;
        config_handle.get_guild_config(guild_id).await.coc_rules
    };

    let times = times.unwrap_or(1);
    let results = roll_coc_multi(skill, times, &rules);
    let guild_id = ctx.guild_id();
    let author = ctx.author().clone();
    let crit_events = if guild_id.is_some() {
        collect_coc_critical_events(&results, skill, &rules, &author, ctx.channel_id())
    } else {
        Vec::new()
    };

    if results.len() == 1 {
        let result = &results[0];
        let success_level = determine_success_level(result.total as u16, skill, &rules);
        let success_text = format_success_level(success_level);
        let content = with_user_note(
            format!(
                "技能值: {}\n骰子結果: {}\n判定結果: {}{}",
                skill,
                result.rolls[0],
                success_text,
                if result.is_critical_success {
                    " ✨ 大成功!"
                } else if result.is_critical_fail {
                    " 💥 大失敗!"
                } else {
                    ""
                }
            ),
            description.as_deref(),
        );
        send_embed(&ctx, "CoC 7e 擲骰結果", content).await?;
    } else {
        // 多次擲骰：顯示統計摘要 + 完整列表
        let total_count = results.len();
        let success_count = results
            .iter()
            .filter(|r| {
                let level = determine_success_level(r.total as u16, skill, &rules);
                level <= 4 // 成功等級 1-4: 大成功、極限成功、困難成功、普通成功
            })
            .count();
        let crit_success_count = results.iter().filter(|r| r.is_critical_success).count();
        let crit_fail_count = results.iter().filter(|r| r.is_critical_fail).count();

        let mut message = format!(
            "連續擲骰次數: {}\n技能值: {}\n\n📊 統計摘要\n成功: {}/{} ({:.1}%)\n大成功: {} | 大失敗: {}\n\n",
            total_count,
            skill,
            success_count,
            total_count,
            (success_count as f64 / total_count as f64) * 100.0,
            crit_success_count,
            crit_fail_count
        );

        // 完整結果列表
        message.push_str("📋 詳細結果\n");
        for (index, result) in results.iter().enumerate() {
            let success_level = determine_success_level(result.total as u16, skill, &rules);
            let success_text = format_success_level(success_level);
            let crit = if result.is_critical_success {
                " ✨"
            } else if result.is_critical_fail {
                " 💥"
            } else {
                ""
            };
            let status = match result.comparison_result {
                Some(true) => " ✅",
                Some(false) => " ❌",
                None => "",
            };
            message.push_str(&format!(
                "{}. {} → {}{}{}\n",
                index + 1,
                result.rolls[0],
                success_text,
                crit,
                status
            ));

            // Discord embed 限制 4096 字元，如果超過則分批發送
            if message.len() > 3800 && index < results.len() - 1 {
                let content = format!("{}\n(續...)", message);
                send_embed(&ctx, "CoC 7e 連續擲骰結果 (部分)", content).await?;
                message.clear();
                message.push_str(&format!("(接續 {} - {})\n", index + 2, results.len()));
            }
        }
        let content = with_user_note(message, description.as_deref());
        send_embed(&ctx, "CoC 7e 連續擲骰結果", content).await?;
    }

    if let Some(guild_id) = guild_id {
        log_critical_events(&ctx, guild_id, crit_events).await?;
    }

    Ok(())
}

fn format_roll_result(result: &RollResult) -> String {
    let rolls_str = result
        .rolls
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<String>>()
        .join(" + ");

    let total_with_mod = if result.modifier != 0 {
        format!("({}) + {} = {}", rolls_str, result.modifier, result.total)
    } else {
        format!("{} = {}", rolls_str, result.total)
    };

    let crit_info = if result.is_critical_success {
        " ✨ 大成功!"
    } else if result.is_critical_fail {
        " 💥 大失敗!"
    } else {
        ""
    };

    let comparison_info = match result.comparison_result {
        Some(true) => "✅ 成功 ",
        Some(false) => "❌ 失敗 ",
        None => "",
    };

    format!(
        "🎲 D&D 擲骰: {} = {}{}{}",
        result.dice_expr, total_with_mod, crit_info, comparison_info
    )
}

fn format_multiple_roll_results(results: &[RollResult]) -> String {
    let mut output = String::from("🎲 連續擲骰結果:\n");

    for (i, result) in results.iter().enumerate() {
        let rolls_str = result
            .rolls
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<String>>()
            .join(" + ");

        let total_with_mod = if result.modifier != 0 {
            format!("({}) + {} = {}", rolls_str, result.modifier, result.total)
        } else {
            format!("{} = {}", rolls_str, result.total)
        };

        output.push_str(&format!(
            "{}. {} = {}\n",
            i + 1,
            result.dice_expr,
            total_with_mod
        ));
    }

    output
}

fn with_user_note(base: String, note: Option<&str>) -> String {
    match note {
        Some(text) if !text.trim().is_empty() => format!("{}\n——\n📝 註: {}", base, text.trim()),
        _ => base,
    }
}

#[derive(Clone, Copy)]
enum CriticalKind {
    Success,
    Fail,
}

fn collect_dnd_critical_events(
    results: &[RollResult],
    expression: &str,
    author: &serenity::User,
    channel: serenity::ChannelId,
) -> Vec<(CriticalKind, String)> {
    let mention = author.mention().to_string();
    let multiple = results.len() > 1;
    let mut events = Vec::new();
    let channel_link = format!("<#{}>", channel);

    for (index, result) in results.iter().enumerate() {
        let roll_values = result
            .rolls
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let prefix = if multiple {
            format!("第 {} 次 ", index + 1)
        } else {
            String::new()
        };

        if result.is_critical_success {
            events.push((
                CriticalKind::Success,
                format!(
                    "{} 在 `/roll {}` {}擲出 [{}] = {}，觸發大成功（頻道：{}）",
                    mention, expression, prefix, roll_values, result.total, channel_link
                ),
            ));
        }
        if result.is_critical_fail {
            events.push((
                CriticalKind::Fail,
                format!(
                    "{} 在 `/roll {}` {}擲出 [{}] = {}，觸發大失敗（頻道：{}）",
                    mention, expression, prefix, roll_values, result.total, channel_link
                ),
            ));
        }
    }

    events
}

fn collect_coc_critical_events(
    results: &[RollResult],
    skill: u8,
    rules: &crate::models::types::CoCRules,
    author: &serenity::User,
    channel: serenity::ChannelId,
) -> Vec<(CriticalKind, String)> {
    let mention = author.mention().to_string();
    let multiple = results.len() > 1;
    let mut events = Vec::new();
    let channel_link = format!("<#{}>", channel);

    for (index, result) in results.iter().enumerate() {
        if !(result.is_critical_success || result.is_critical_fail) {
            continue;
        }

        let prefix = if multiple {
            format!("第 {} 次 ", index + 1)
        } else {
            String::new()
        };
        let success_level = determine_success_level(result.total as u16, skill, rules);
        let success_text = format_success_level(success_level);
        let base = format!(
            "{} 在 `/coc {}` {}擲出 {} ({})（頻道：{}）",
            mention, skill, prefix, result.rolls[0], success_text, channel_link
        );

        if result.is_critical_success {
            events.push((CriticalKind::Success, format!("{}，觸發大成功", base)));
        }
        if result.is_critical_fail {
            events.push((CriticalKind::Fail, format!("{}，觸發大失敗", base)));
        }
    }

    events
}

async fn log_critical_events(
    ctx: &Context<'_>,
    guild_id: serenity::GuildId,
    events: Vec<(CriticalKind, String)>,
) -> Result<(), Error> {
    if events.is_empty() {
        return Ok(());
    }

    let (success_channel, fail_channel) = {
        let data = ctx.data();
        let manager = data.config.lock().await;
        let cfg = manager.get_guild_config(guild_id.get()).await;
        (cfg.crit_success_channel, cfg.crit_fail_channel)
    };

    let http = &ctx.serenity_context().http;
    for (kind, content) in events {
        let (channel_id, title, colour) = match kind {
            CriticalKind::Success => (success_channel, "大成功紀錄", serenity::Colour::DARK_GREEN),
            CriticalKind::Fail => (fail_channel, "大失敗紀錄", serenity::Colour::DARK_RED),
        };

        let Some(channel_id) = channel_id else {
            continue;
        };
        let channel = serenity::ChannelId::new(channel_id);
        let embed = serenity::CreateEmbed::default()
            .title(title)
            .description(content.clone())
            .colour(colour);
        let builder = serenity::CreateMessage::new().embed(embed);

        if let Err(err) = channel.send_message(http, builder).await {
            log::warn!("發送關鍵紀錄失敗: {:?}", err);
        }
    }

    Ok(())
}

async fn send_embed(
    ctx: &Context<'_>,
    title: impl Into<String>,
    description: String,
) -> Result<(), Error> {
    let title = title.into();
    let embed = serenity::CreateEmbed::default()
        .title(title)
        .description(description)
        .colour(serenity::Colour::BLURPLE);
    let reply = CreateReply::default().embed(embed);
    ctx.send(reply).await?;
    Ok(())
}
