use crate::bot::{Context, Error};
use poise::{
    ChoiceParameter, CreateReply,
    serenity_prelude::{
        self as serenity, ButtonStyle, CreateActionRow, CreateButton, CreateInteractionResponse,
        CreateInteractionResponseMessage, Mentionable,
    },
};
use std::time::Duration;
use tokio_rusqlite::{OptionalExtension, Result as DbResult, params};

#[derive(ChoiceParameter, Clone, Copy, Debug)]
pub enum SkillAction {
    #[name = "add"]
    Add,
    #[name = "show"]
    Show,
    #[name = "delete"]
    Delete,
}

struct DbSkill {
    name: String,
    normalized_name: String,
    skill_type: String,
    level: String,
    effect: String,
    occupation: String,
    race: String,
}

/// 技能資料庫指令
#[allow(clippy::too_many_arguments)]
#[poise::command(slash_command)]
pub async fn skill(
    ctx: Context<'_>,
    #[description = "操作 add、show 或 delete"] action: SkillAction,
    #[description = "技能名稱"] name: String,
    #[description = "技能類型 (add 必填)"] skill_type: Option<String>,
    #[description = "技能等級 (add 必填)"] level: Option<String>,
    #[description = "技能效果 (add 必填)"] effect: Option<String>,
    #[description = "職業 (add 選填)"] occupation: Option<String>,
    #[description = "種族 (add 選填)"] race: Option<String>,
) -> Result<(), Error> {
    log::info!("執行技能指令: {:?} for guild {:?}", action, ctx.guild_id());
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
        SkillAction::Add => {
            let Some(skill_type) = skill_type.filter(|s| !s.trim().is_empty()) else {
                let embed = serenity::CreateEmbed::default()
                    .colour(serenity::Colour::RED)
                    .description("請提供技能類型");
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            };
            let skill_type = skill_type.trim().to_string();

            let Some(level) = level.filter(|s| !s.trim().is_empty()) else {
                let embed = serenity::CreateEmbed::default()
                    .colour(serenity::Colour::RED)
                    .description("請提供技能等級");
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            };
            let level = level.trim().to_string();
            let Some(effect) = effect.filter(|s| !s.trim().is_empty()) else {
                let embed = serenity::CreateEmbed::default()
                    .colour(serenity::Colour::RED)
                    .description("請提供技能效果");
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            };
            let effect = effect.trim().to_string();

            let occupation = occupation
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let race = race
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            add_skill(
                &ctx,
                guild_id,
                &name,
                &skill_type,
                &level,
                &effect,
                &occupation,
                &race,
            )
            .await?;

            // 獲取技能總數以顯示在嵌入消息中
            let total_skills_count = get_total_skills_count(&ctx, guild_id).await?;

            let mut fields = vec![
                ("名稱", format!("`{}`", name), false),
                ("類型", skill_type.clone(), true),
                ("等級", level.clone(), true),
                ("效果", effect.clone(), false),
            ];

            if !occupation.is_empty() {
                fields.push(("職業", occupation.clone(), true));
            }
            if !race.is_empty() {
                fields.push(("種族", race.clone(), true));
            }

            let embed = serenity::CreateEmbed::default()
                .title("技能已儲存")
                .footer(serenity::CreateEmbedFooter::new(format!(
                    "總技能數：{}",
                    total_skills_count
                )))
                .fields(fields)
                .colour(serenity::Colour::DARK_GREEN);
            ctx.send(CreateReply::default().embed(embed)).await?;
        }
        SkillAction::Show => {
            // 獲取技能總數
            let total_skills_count = get_total_skills_count(&ctx, guild_id).await?;

            // 進行多字段模糊搜索
            let search_results = search_skills(&ctx, guild_id, &name).await?;

            if search_results.is_empty() {
                let embed = serenity::CreateEmbed::default()
                    .title(format!("技能：<{}>", name))
                    .footer(serenity::CreateEmbedFooter::new(format!(
                        "總技能數：{}",
                        total_skills_count
                    )))
                    .colour(serenity::Colour::ORANGE)
                    .description(format!("找不到包含 `{}` 的技能", name));

                ctx.send(CreateReply::default().embed(embed)).await?;
            } else if search_results.len() == 1 {
                // 如果只找到一個結果，直接顯示該技能
                let db_skill = &search_results[0];
                let mut fields = vec![
                    ("類型", db_skill.skill_type.clone(), true),
                    ("等級", db_skill.level.clone(), true),
                    ("效果", db_skill.effect.clone(), false),
                ];

                if !db_skill.occupation.is_empty() {
                    fields.push(("職業", db_skill.occupation.clone(), true));
                }
                if !db_skill.race.is_empty() {
                    fields.push(("種族", db_skill.race.clone(), true));
                }

                let embed = serenity::CreateEmbed::default()
                    .title(format!("技能：<{}>", db_skill.name))
                    .footer(serenity::CreateEmbedFooter::new(format!(
                        "總技能數：{}",
                        total_skills_count
                    )))
                    .fields(fields)
                    .colour(serenity::Colour::BLURPLE);

                ctx.send(CreateReply::default().embed(embed)).await?;
            } else {
                // 如果找到多個結果，則顯示可翻頁的 embed 列表
                const SKILLS_PER_PAGE: usize = 5; // 每頁顯示5個技能
                let total_pages = search_results.len().div_ceil(SKILLS_PER_PAGE); // 計算總頁數
                let mut current_page = 0; // 當前頁面索引

                // 創建函數來生成指定頁面的embed和組件
                let create_page =
                    |page_index: usize| -> (serenity::CreateEmbed, Vec<CreateActionRow>) {
                        let start_idx = page_index * SKILLS_PER_PAGE;
                        let end_idx =
                            std::cmp::min(start_idx + SKILLS_PER_PAGE, search_results.len());

                        let mut description = String::new();
                        let mut components = Vec::new();

                        // 添加當前頁面的技能
                        for (i, skill) in search_results[start_idx..end_idx].iter().enumerate() {
                            let skill_idx = start_idx + i;
                            let mut skill_info = format!(
                                "**{}**. **名稱**: {}\n**類型**: {} | **等級**: {}",
                                skill_idx + 1, // 顯示全局編號
                                skill.name,
                                skill.skill_type,
                                skill.level
                            );

                            // 添加職業和種族信息（如果存在）
                            if !skill.occupation.is_empty() {
                                skill_info.push_str(&format!(" | **職業**: {}", skill.occupation));
                            }
                            if !skill.race.is_empty() {
                                skill_info.push_str(&format!(" | **種族**: {}", skill.race));
                            }
                            skill_info.push_str("\n\n");

                            description.push_str(&skill_info);
                        }

                        // 添加技能選擇按鈕 (每行最多4個技能按鈕，保留空間給翻頁按鈕)
                        let skills_in_page = end_idx - start_idx;
                        let mut skill_row = CreateActionRow::Buttons(vec![]);
                        for i in 0..skills_in_page {
                            let skill_idx = start_idx + i;
                            let button_id = format!("skill_detail_{}_{}", guild_id, skill_idx);
                            let button = CreateButton::new(button_id)
                                .label(format!("{}", skill_idx + 1)) // 按鈕標籤為全局編號
                                .style(ButtonStyle::Primary);

                            if let serenity::CreateActionRow::Buttons(ref mut buttons) = skill_row {
                                buttons.push(button);
                            }
                        }

                        if skills_in_page > 0 {
                            components.push(skill_row);
                        }

                        // 添加翻頁按鈕行
                        if total_pages > 1 {
                            let mut pagination_row = CreateActionRow::Buttons(vec![]);

                            // 上一頁按鈕
                            if page_index > 0 {
                                let prev_button = CreateButton::new(format!(
                                    "skill_prev_{}_{}",
                                    guild_id, page_index
                                ))
                                .label("上一頁")
                                .style(ButtonStyle::Secondary);
                                if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                    pagination_row
                                {
                                    buttons.push(prev_button);
                                }
                            }

                            // 頁數信息按鈕 (非交互)
                            let page_info_button = CreateButton::new(format!(
                                "skill_info_{}_{}",
                                guild_id, page_index
                            ))
                            .label(format!("{}/{}", page_index + 1, total_pages))
                            .style(ButtonStyle::Secondary)
                            .disabled(true); // 禁用的按鈕，僅用於顯示信息
                            if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                pagination_row
                            {
                                buttons.push(page_info_button);
                            }

                            // 下一頁按鈕
                            if page_index < total_pages - 1 {
                                let next_button = CreateButton::new(format!(
                                    "skill_next_{}_{}",
                                    guild_id, page_index
                                ))
                                .label("下一頁")
                                .style(ButtonStyle::Secondary);
                                if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                    pagination_row
                                {
                                    buttons.push(next_button);
                                }
                            }

                            components.push(pagination_row);
                        }

                        let embed = serenity::CreateEmbed::default()
                            .title(format!(
                                "包含「{}」的技能 (第 {}/{} 頁)",
                                name,
                                page_index + 1,
                                total_pages
                            ))
                            .footer(serenity::CreateEmbedFooter::new(format!(
                                "總技能數：{}",
                                total_skills_count
                            )))
                            .description(description)
                            .colour(serenity::Colour::BLURPLE);

                        (embed, components)
                    };

                // 發送當前頁面的消息
                let (embed, components) = create_page(current_page);
                let reply = CreateReply::default().embed(embed).components(components);
                let sent = ctx.send(reply).await?;

                // 處理按鈕交互
                let mut message = sent.into_message().await?;
                let ctx_clone = ctx.serenity_context().clone();
                let author_id = ctx.author().id;

                let mut next_interaction = message
                    .await_component_interaction(&ctx_clone)
                    .author_id(author_id)
                    .timeout(Duration::from_secs(120))
                    .await;

                while let Some(interaction) = next_interaction {
                    // 檢查是否為技能選擇按鈕
                    if let Some(skill_index_str) = interaction
                        .data
                        .custom_id
                        .strip_prefix(&format!("skill_detail_{}_", &guild_id))
                    {
                        if let Ok(skill_index) = skill_index_str.parse::<usize>() {
                            if skill_index < search_results.len() {
                                let selected_skill = &search_results[skill_index];

                                // 創建詳細信息的embed
                                let mut detail_embed = serenity::CreateEmbed::default()
                                    .title(format!("技能詳細：<{}>", selected_skill.name))
                                    .footer(serenity::CreateEmbedFooter::new(format!(
                                        "總技能數：{}",
                                        total_skills_count
                                    )))
                                    .fields([
                                        ("類型", selected_skill.skill_type.clone(), true),
                                        ("等級", selected_skill.level.clone(), true),
                                        ("效果", selected_skill.effect.clone(), false),
                                    ])
                                    .colour(serenity::Colour::GOLD);

                                // 添加職業和種族信息（如果存在）
                                if !selected_skill.occupation.is_empty() {
                                    detail_embed = detail_embed.field(
                                        "職業",
                                        &selected_skill.occupation,
                                        true,
                                    );
                                }
                                if !selected_skill.race.is_empty() {
                                    detail_embed =
                                        detail_embed.field("種族", &selected_skill.race, true);
                                }

                                // 首先響應詳細信息作為新消息（ephemeral）
                                let response = CreateInteractionResponseMessage::default()
                                    .embed(detail_embed)
                                    .ephemeral(true); // 設置為私密消息
                                interaction
                                    .create_response(
                                        &ctx_clone,
                                        CreateInteractionResponse::Message(response),
                                    )
                                    .await?;

                                next_interaction = message
                                    .await_component_interaction(&ctx_clone)
                                    .author_id(author_id)
                                    .timeout(Duration::from_secs(120))
                                    .await;
                                continue;
                            }
                        }
                    }

                    // 檢查是否為下一頁按鈕
                    if interaction
                        .data
                        .custom_id
                        .starts_with(&format!("skill_next_{}_", &guild_id))
                    {
                        if current_page < total_pages - 1 {
                            current_page += 1;
                        }

                        let (new_embed, new_components) = create_page(current_page);
                        let update_msg = CreateInteractionResponseMessage::default()
                            .embed(new_embed)
                            .components(new_components);
                        interaction
                            .create_response(
                                &ctx_clone,
                                CreateInteractionResponse::UpdateMessage(update_msg),
                            )
                            .await?;

                        message = *interaction.message.clone();
                        next_interaction = message
                            .await_component_interaction(&ctx_clone)
                            .author_id(author_id)
                            .timeout(Duration::from_secs(120))
                            .await;
                        continue;
                    }

                    // 檢查是否為上一頁按鈕
                    if interaction
                        .data
                        .custom_id
                        .starts_with(&format!("skill_prev_{}_", &guild_id))
                    {
                        current_page = current_page.saturating_sub(1);

                        let (new_embed, new_components) = create_page(current_page);
                        let update_msg = CreateInteractionResponseMessage::default()
                            .embed(new_embed)
                            .components(new_components);
                        interaction
                            .create_response(
                                &ctx_clone,
                                CreateInteractionResponse::UpdateMessage(update_msg),
                            )
                            .await?;

                        message = *interaction.message.clone();
                        next_interaction = message
                            .await_component_interaction(&ctx_clone)
                            .author_id(author_id)
                            .timeout(Duration::from_secs(120))
                            .await;
                        continue;
                    }

                    next_interaction = message
                        .await_component_interaction(&ctx_clone)
                        .author_id(author_id)
                        .timeout(Duration::from_secs(120))
                        .await;
                }

                if next_interaction.is_none() {
                    let _ = message
                        .edit(
                            &ctx_clone.http,
                            serenity::EditMessage::new().components(vec![]),
                        )
                        .await;
                    let _ = message
                        .channel_id
                        .send_message(
                            &ctx_clone.http,
                            serenity::CreateMessage::default()
                                .content("操作逾時，請重新執行指令。"),
                        )
                        .await;
                }
            }
        }
        SkillAction::Delete => {
            let caller = ctx.author().clone();

            let Some(db_skill) = find_skill_in_guild(&ctx, guild_id, &name).await? else {
                // 獲取技能總數以顯示在嵌入消息中
                let total_skills_count = get_total_skills_count(&ctx, guild_id).await?;

                let embed = serenity::CreateEmbed::default()
                    .footer(serenity::CreateEmbedFooter::new(format!(
                        "總技能數：{}",
                        total_skills_count
                    )))
                    .colour(serenity::Colour::ORANGE)
                    .description(format!("找不到此伺服器中的技能 `{}`，無法刪除", name));
                ctx.send(CreateReply::default().embed(embed)).await?;
                return Ok(());
            };

            // 在確認刪除前先獲取當前技能總數
            let total_skills_count_before = get_total_skills_count(&ctx, guild_id).await?;

            let confirm_id = format!(
                "skill_delete_confirm:{}:{}",
                guild_id, db_skill.normalized_name
            );
            let cancel_id = format!(
                "skill_delete_cancel:{}:{}",
                guild_id, db_skill.normalized_name
            );
            let components = vec![CreateActionRow::Buttons(vec![
                CreateButton::new(confirm_id.clone())
                    .label("確認刪除")
                    .style(ButtonStyle::Danger),
                CreateButton::new(cancel_id.clone())
                    .label("取消")
                    .style(ButtonStyle::Secondary),
            ])];

            let mut description = format!(
                "目標技能：`{}`\n類型：{}\n等級：{}\n效果：{}",
                &db_skill.name, &db_skill.skill_type, &db_skill.level, &db_skill.effect
            );

            if !db_skill.occupation.is_empty() {
                description.push_str(&format!("\n職業：{}", &db_skill.occupation));
            }
            if !db_skill.race.is_empty() {
                description.push_str(&format!("\n種族：{}", &db_skill.race));
            }

            let embed = serenity::CreateEmbed::default()
                .title("確認刪除技能")
                .footer(serenity::CreateEmbedFooter::new(format!(
                    "總技能數：{}",
                    total_skills_count_before
                )))
                .description(description)
                .colour(serenity::Colour::DARK_RED);

            let reply = CreateReply::default().embed(embed).components(components);

            let sent = ctx.send(reply).await?;
            let mut message = sent.into_message().await?;
            let ctx_clone = ctx.serenity_context().clone();
            let author_id = caller.id;

            let interaction = message
                .await_component_interaction(&ctx_clone)
                .author_id(author_id)
                .timeout(Duration::from_secs(30))
                .await;

            match interaction {
                Some(interaction) if interaction.data.custom_id == confirm_id => {
                    delete_skill(&ctx, guild_id, &db_skill.normalized_name).await?;

                    // 技能刪除後再次獲取新的技能總數
                    let total_skills_count_after = get_total_skills_count(&ctx, guild_id).await?;

                    let summary = format!(
                        "{} 刪除了技能 `{}` (剩餘技能數：{})",
                        caller.mention(),
                        db_skill.name,
                        total_skills_count_after
                    );

                    let mut response = CreateInteractionResponseMessage::default();
                    response = response.content(summary).components(Vec::new());
                    interaction
                        .create_response(
                            &ctx_clone,
                            CreateInteractionResponse::UpdateMessage(response),
                        )
                        .await?;
                }
                Some(interaction) => {
                    // 獲取技能總數以顯示在嵌入消息中
                    let total_skills_count = get_total_skills_count(&ctx, guild_id).await?;

                    let mut response = CreateInteractionResponseMessage::default();
                    response = response
                        .content(format!(
                            "{} 取消刪除操作 (總技能數：{})",
                            caller.mention(),
                            total_skills_count
                        ))
                        .components(Vec::new());
                    interaction
                        .create_response(
                            &ctx_clone,
                            CreateInteractionResponse::UpdateMessage(response),
                        )
                        .await?;
                }
                None => {
                    // 獲取技能總數以顯示在嵌入消息中
                    let total_skills_count = get_total_skills_count(&ctx, guild_id).await?;

                    let edit = serenity::builder::EditMessage::new()
                        .content(format!(
                            "操作逾時，未刪除任何技能 (總技能數：{})",
                            total_skills_count
                        ))
                        .components(Vec::new());
                    let _ = message.edit(&ctx_clone.http, edit).await;
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn add_skill(
    ctx: &Context<'_>,
    guild_id: u64,
    name: &str,
    skill_type: &str,
    level: &str,
    effect: &str,
    occupation: &str,
    race: &str,
) -> Result<(), Error> {
    let skills_db = ctx.data().skills_db.clone();
    let normalized = name.to_lowercase();
    let name = name.to_string();
    let skill_type = skill_type.to_string();
    let level = level.to_string();
    let effect = effect.to_string();
    let occupation = occupation.to_string();
    let race = race.to_string();

    skills_db.call(move |conn| {
        conn.execute(
            "INSERT INTO skills (guild_id, name, normalized_name, skill_type, level, effect, occupation, race)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(guild_id, normalized_name)
            DO UPDATE SET name=excluded.name, skill_type=excluded.skill_type, level=excluded.level, effect=excluded.effect, occupation=excluded.occupation, race=excluded.race",
            params![
                guild_id as i64,
                name,
                normalized,
                skill_type,
                level,
                effect,
                occupation,
                race
            ],
        )?;
        Ok(())
    })
    .await?;

    Ok(())
}

async fn search_skills(
    ctx: &Context<'_>,
    guild_id: u64,
    search_term: &str,
) -> Result<Vec<DbSkill>, Error> {
    let skills_db = ctx.data().skills_db.clone();
    let guild_id_i64 = guild_id as i64;
    let search_term = search_term.to_lowercase();
    let pattern = format!("%{}%", search_term);

    let result = skills_db
        .call(move |conn| -> DbResult<Vec<DbSkill>> {
            let mut stmt = conn.prepare(
                "SELECT name, normalized_name, skill_type, level, effect, occupation, race
                FROM skills
                WHERE guild_id = ?1 
                AND (normalized_name LIKE ?2 OR skill_type LIKE ?2 OR level LIKE ?2 OR occupation LIKE ?2 OR race LIKE ?2)
                ORDER BY 
                    CASE WHEN normalized_name LIKE ?2 THEN 1
                         WHEN skill_type LIKE ?2 THEN 2
                         WHEN level LIKE ?2 THEN 3
                         WHEN occupation LIKE ?2 THEN 4
                         WHEN race LIKE ?2 THEN 5
                         ELSE 6 END,
                    ABS(LENGTH(normalized_name) - LENGTH(?3)),
                    normalized_name",
            )?;

            let rows = stmt.query_map(params![guild_id_i64, pattern, search_term], |row| {
                Ok(DbSkill {
                    name: row.get(0)?,
                    normalized_name: row.get(1)?,
                    skill_type: row.get(2)?,
                    level: row.get(3)?,
                    effect: row.get(4)?,
                    occupation: row.get(5)?,
                    race: row.get(6)?,
                })
            })?;

            let mut skills = Vec::new();
            for row in rows {
                skills.push(row?);
            }

            Ok(skills)
        })
        .await?;

    Ok(result)
}

async fn find_skill_in_guild(
    ctx: &Context<'_>,
    guild_id: u64,
    name: &str,
) -> Result<Option<DbSkill>, Error> {
    let skills_db = ctx.data().skills_db.clone();
    let guild_id_i64 = guild_id as i64;
    let normalized = name.to_lowercase();
    let pattern = format!("%{}%", normalized);

    let result = skills_db
        .call(move |conn| -> DbResult<Option<DbSkill>> {
            let row = conn
                .query_row(
                    "SELECT name, normalized_name, skill_type, level, effect, occupation, race
                FROM skills
                WHERE guild_id = ?1 AND normalized_name LIKE ?2
                ORDER BY CASE WHEN normalized_name = ?3 THEN 0 ELSE 1 END,
                        ABS(LENGTH(normalized_name) - LENGTH(?3)),
                        normalized_name
                LIMIT 1",
                    params![guild_id_i64, pattern, normalized],
                    |row| {
                        Ok(DbSkill {
                            name: row.get(0)?,
                            normalized_name: row.get(1)?,
                            skill_type: row.get(2)?,
                            level: row.get(3)?,
                            effect: row.get(4)?,
                            occupation: row.get(5)?,
                            race: row.get(6)?,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
        .await?;

    Ok(result)
}

async fn get_total_skills_count(ctx: &Context<'_>, guild_id: u64) -> Result<i32, Error> {
    let skills_db = ctx.data().skills_db.clone();
    let guild_id_i64 = guild_id as i64;

    let count = skills_db
        .call(move |conn| -> DbResult<i32> {
            let mut stmt = conn.prepare("SELECT COUNT(*) FROM skills WHERE guild_id = ?1")?;
            let count: i32 = stmt.query_row([guild_id_i64], |row| row.get(0))?;
            Ok(count)
        })
        .await?;

    Ok(count)
}

async fn delete_skill(
    ctx: &Context<'_>,
    guild_id: u64,
    normalized_name: &str,
) -> Result<(), Error> {
    let skills_db = ctx.data().skills_db.clone();
    let guild_id_i64 = guild_id as i64;
    let normalized = normalized_name.to_string();

    skills_db
        .call(move |conn| -> DbResult<()> {
            conn.execute(
                "DELETE FROM skills
            WHERE guild_id = ?1 AND normalized_name = ?2",
                params![guild_id_i64, normalized],
            )?;
            Ok(())
        })
        .await?;

    Ok(())
}
