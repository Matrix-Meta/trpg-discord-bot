use crate::bot::{Context, Error};
use poise::ChoiceParameter;
use poise::{
    CreateReply,
    serenity_prelude::{self as serenity, CreateActionRow, CreateSelectMenuOption},
};

#[derive(ChoiceParameter, Clone, Copy, Debug)]
pub enum OutputMode {
    #[name = "部分 (前5筆)"]
    Partial,
    #[name = "全部"]
    All,
}

/// 基礎設定資料庫搜尋指令
#[poise::command(slash_command, rename = "bs-search")]
pub async fn base_settings_search(
    ctx: Context<'_>,
    #[description = "搜尋關鍵字 (對所選資料表中的資料進行模糊搜尋)"] search_keyword: Option<String>,
    #[description = "輸出模式"] mode: Option<OutputMode>,
) -> Result<(), Error> {
    log::info!(
        "執行基礎設定資料庫搜尋指令: search_keyword: {:?}, mode: {:?}",
        search_keyword,
        mode
    );

    // 獲取資料庫連線
    let base_settings_db = ctx.data().base_settings_db.clone();

    // 獲取所有資料表名稱
    let tables: Vec<String> = base_settings_db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            let mut tables = Vec::new();
            for table_name in rows {
                tables.push(table_name?);
            }
            Ok(tables)
        })
        .await
        .map_err(|e| {
            log::error!("查詢資料表失敗: {}", e);
            Error::msg("查詢資料表失敗")
        })?;

    if tables.is_empty() {
        let embed = serenity::CreateEmbed::default()
            .title("基礎設定資料庫")
            .description("目前沒有任何資料表")
            .colour(serenity::Colour::ORANGE);
        ctx.send(CreateReply::default().embed(embed).ephemeral(true))
            .await?;
        return Ok(());
    }

    // 提供選單讓用戶選擇存在的資料表
    let mut options = Vec::new();
    for table in &tables {
        options.push(
            CreateSelectMenuOption::new(table.clone(), table.clone())
                .description(format!("資料表: {}", table)),
        );
    }

    // 限制選項數量，Discord 選單最多支援 25 個選項
    let truncated = options.len() > 25;
    if truncated {
        options.truncate(25);
    }

    let select_menu = serenity::CreateSelectMenu::new(
        "table_selection",
        serenity::CreateSelectMenuKind::String { options },
    )
    .placeholder("選擇一個資料表...");

    let components = vec![CreateActionRow::SelectMenu(select_menu)];

    let mut description = format!("可用的資料表：\n{}", tables.join(", "));
    if truncated {
        description
            .push_str("\n\n注意：資料表超過 25 個，僅顯示前 25 個。請縮小資料表數量後再試。");
    }

    let embed = serenity::CreateEmbed::default()
        .title("選擇資料表")
        .description(description)
        .colour(serenity::Colour::BLURPLE);

    let reply = CreateReply::default().embed(embed).components(components);
    let sent = ctx.send(reply).await?;
    let message = sent.into_message().await?;
    let ctx_clone = ctx.serenity_context().clone();
    let author_id = ctx.author().id;

    // 等待用戶選擇
    let selection = message
        .await_component_interaction(&ctx_clone)
        .author_id(author_id)
        .timeout(std::time::Duration::from_secs(60))
        .await;

    let Some(interaction) = selection else {
        let timeout_reply = CreateReply::default()
            .content("操作逾時，請重新執行指令。")
            .ephemeral(true);
        ctx.send(timeout_reply).await?;
        return Ok(());
    };

    if interaction.data.custom_id == "table_selection" {
        if let serenity::ComponentInteractionDataKind::StringSelect { values } =
            &interaction.data.kind
        {
            if let Some(selected_value) = values.first() {
                interaction
                    .create_response(
                        &ctx_clone,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::default()
                                .content(format!(
                                    "已選擇資料表: **{}**，正在載入...",
                                    selected_value
                                ))
                                .components(vec![]),
                        ),
                    )
                    .await?;

                let (count, column_names, all_data) =
                    get_table_info_full(&ctx, selected_value).await?;

                let filtered_data = if let Some(keyword) = &search_keyword {
                    let keyword_lower = keyword.to_lowercase();
                    all_data
                        .into_iter()
                        .filter(|row| {
                            row.iter()
                                .any(|value| value.to_lowercase().contains(&keyword_lower))
                        })
                        .collect()
                } else {
                    all_data
                };

                if search_keyword.is_some() && filtered_data.len() == 1 {
                    let row = &filtered_data[0];
                    let mut row_content = String::new();
                    for value in row {
                        row_content.push_str(&format!("`{}` ", value));
                    }

                    let detail_embed = serenity::CreateEmbed::default()
                        .title(format!("🔍 搜尋結果: {}", selected_value))
                        .description(row_content.trim())
                        .colour(serenity::Colour::GOLD);
                    ctx.send(CreateReply::default().embed(detail_embed).ephemeral(true))
                        .await?;
                } else if !filtered_data.is_empty() {
                    if filtered_data.len() == 1 && search_keyword.is_none() {
                        let row = &filtered_data[0];
                        let mut row_content = String::new();
                        for value in row {
                            row_content.push_str(&format!("`{}` ", value));
                        }

                        let detail_embed = serenity::CreateEmbed::default()
                            .title(format!("資料表內容: {}", selected_value))
                            .description(row_content.trim())
                            .colour(serenity::Colour::BLURPLE);
                        ctx.send(CreateReply::default().embed(detail_embed).ephemeral(true))
                            .await?;
                    } else {
                        const ROWS_PER_PAGE: usize = 5;
                        let total_pages = filtered_data.len().div_ceil(ROWS_PER_PAGE);
                        let mut current_page = 0;

                        let create_page =
                            |page_index: usize| -> (serenity::CreateEmbed, Vec<CreateActionRow>) {
                                let start_idx = page_index * ROWS_PER_PAGE;
                                let end_idx =
                                    std::cmp::min(start_idx + ROWS_PER_PAGE, filtered_data.len());

                                let mut description = String::new();
                                let mut components = Vec::new();

                                for (i, row) in filtered_data[start_idx..end_idx].iter().enumerate()
                                {
                                    let row_idx = start_idx + i;
                                    let mut row_str = format!("**{}**. ", row_idx + 1);
                                    for value in row {
                                        row_str.push_str(&format!("`{}` ", value));
                                    }
                                    row_str.push('\n');
                                    description.push_str(&row_str);
                                }

                                let rows_in_page = end_idx - start_idx;
                                if rows_in_page > 0 {
                                    let mut row_row = CreateActionRow::Buttons(vec![]);
                                    for i in 0..rows_in_page {
                                        let row_idx = start_idx + i;
                                        let button_id = format!("row_detail_{}", row_idx);
                                        let button = serenity::CreateButton::new(button_id)
                                            .label(format!("{}", row_idx + 1))
                                            .style(serenity::ButtonStyle::Primary);

                                        if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                            row_row
                                        {
                                            buttons.push(button);
                                        }
                                    }
                                    components.push(row_row);
                                }

                                if total_pages > 1 {
                                    let mut pagination_row = CreateActionRow::Buttons(vec![]);

                                    if page_index > 0 {
                                        let prev_button = serenity::CreateButton::new(format!(
                                            "row_prev_{}",
                                            page_index
                                        ))
                                        .label("上一頁")
                                        .style(serenity::ButtonStyle::Secondary);
                                        if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                            pagination_row
                                        {
                                            buttons.push(prev_button);
                                        }
                                    }

                                    let page_info_button = serenity::CreateButton::new(format!(
                                        "row_info_{}",
                                        page_index
                                    ))
                                    .label(format!("{}/{}", page_index + 1, total_pages))
                                    .style(serenity::ButtonStyle::Secondary)
                                    .disabled(true);
                                    if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                        pagination_row
                                    {
                                        buttons.push(page_info_button);
                                    }

                                    if page_index < total_pages - 1 {
                                        let next_button = serenity::CreateButton::new(format!(
                                            "row_next_{}",
                                            page_index
                                        ))
                                        .label("下一頁")
                                        .style(serenity::ButtonStyle::Secondary);
                                        if let serenity::CreateActionRow::Buttons(ref mut buttons) =
                                            pagination_row
                                        {
                                            buttons.push(next_button);
                                        }
                                    }

                                    components.push(pagination_row);
                                }

                                let title = if let Some(ref keyword) = search_keyword {
                                    format!(
                                        "搜尋「{}」的結果 (第 {}/{} 頁)",
                                        keyword,
                                        page_index + 1,
                                        total_pages
                                    )
                                } else {
                                    format!(
                                        "資料表內容: {} (第 {}/{} 頁)",
                                        selected_value,
                                        page_index + 1,
                                        total_pages
                                    )
                                };

                                let embed = serenity::CreateEmbed::default()
                                    .title(title)
                                    .description(description)
                                    .colour(serenity::Colour::BLURPLE);

                                (embed, components)
                            };

                        let (embed, components) = create_page(current_page);
                        let reply = CreateReply::default().embed(embed).components(components);
                        let sent = ctx.send(reply).await?;

                        let mut message = sent.into_message().await?;

                        let ctx_clone = ctx.serenity_context().clone();
                        let author_id = ctx.author().id;

                        let mut last_interaction =
                            next_page_interaction(&mut message, &ctx_clone, author_id).await;

                        while let Some(interaction) = last_interaction {
                            if let Some(row_index_str) =
                                interaction.data.custom_id.strip_prefix("row_detail_")
                            {
                                if let Ok(row_index) = row_index_str.parse::<usize>() {
                                    if row_index < filtered_data.len() {
                                        let selected_row = &filtered_data[row_index];

                                        let mut detail_description = String::new();
                                        for (i, value) in selected_row.iter().enumerate() {
                                            if i < column_names.len() {
                                                detail_description.push_str(&format!(
                                                    "**{}**: `{}`\n",
                                                    column_names[i], value
                                                ));
                                            } else {
                                                detail_description.push_str(&format!(
                                                    "**未知欄位**: `{}`\n",
                                                    value
                                                ));
                                            }
                                        }

                                        let detail_embed = serenity::CreateEmbed::default()
                                            .title(format!("詳細資料 - 資料列 {}", row_index + 1))
                                            .description(detail_description)
                                            .colour(serenity::Colour::GOLD);

                                        let response =
                                            serenity::CreateInteractionResponseMessage::default()
                                                .embed(detail_embed)
                                                .ephemeral(true);
                                        interaction
                                            .create_response(
                                                &ctx_clone,
                                                serenity::CreateInteractionResponse::Message(
                                                    response,
                                                ),
                                            )
                                            .await?;

                                        last_interaction = next_page_interaction(
                                            &mut message,
                                            &ctx_clone,
                                            author_id,
                                        )
                                        .await;
                                        continue;
                                    }
                                }
                            }

                            if interaction.data.custom_id.starts_with("row_next_") {
                                if current_page < total_pages - 1 {
                                    current_page += 1;
                                }

                                let (new_embed, new_components) = create_page(current_page);
                                let update_msg =
                                    serenity::CreateInteractionResponseMessage::default()
                                        .embed(new_embed)
                                        .components(new_components);
                                interaction
                                    .create_response(
                                        &ctx_clone,
                                        serenity::CreateInteractionResponse::UpdateMessage(
                                            update_msg,
                                        ),
                                    )
                                    .await?;

                                message = *interaction.message.clone();
                                last_interaction =
                                    next_page_interaction(&mut message, &ctx_clone, author_id)
                                        .await;
                                continue;
                            }

                            if interaction.data.custom_id.starts_with("row_prev_") {
                                current_page = current_page.saturating_sub(1);

                                let (new_embed, new_components) = create_page(current_page);
                                let update_msg =
                                    serenity::CreateInteractionResponseMessage::default()
                                        .embed(new_embed)
                                        .components(new_components);
                                interaction
                                    .create_response(
                                        &ctx_clone,
                                        serenity::CreateInteractionResponse::UpdateMessage(
                                            update_msg,
                                        ),
                                    )
                                    .await?;

                                message = *interaction.message.clone();
                                last_interaction =
                                    next_page_interaction(&mut message, &ctx_clone, author_id)
                                        .await;
                                continue;
                            }

                            last_interaction =
                                next_page_interaction(&mut message, &ctx_clone, author_id).await;
                        }

                        if last_interaction.is_none() {
                            let _ = message
                                .channel_id
                                .send_message(
                                    &ctx_clone.http,
                                    serenity::CreateMessage::default()
                                        .content("操作逾時，請重新執行指令。"),
                                )
                                .await;
                            let _ = message
                                .edit(
                                    &ctx_clone.http,
                                    serenity::EditMessage::new().components(vec![]),
                                )
                                .await;
                        }
                    }
                } else {
                    let embed = serenity::CreateEmbed::default()
                        .title("無搜尋結果")
                        .description(if let Some(ref keyword) = search_keyword {
                            format!(
                                "在資料表 `{}` 中找不到包含 '{}' 的資料",
                                selected_value, keyword
                            )
                        } else {
                            format!(
                                "資料表 `{}` 中沒有任何資料 (總計 {} 筆)",
                                selected_value, count
                            )
                        })
                        .colour(serenity::Colour::ORANGE);
                    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
                        .await?;
                }
            }
        }
    }

    Ok(())
}

async fn next_page_interaction(
    message: &mut serenity::Message,
    ctx: &serenity::Context,
    author_id: serenity::UserId,
) -> Option<serenity::ComponentInteraction> {
    message
        .await_component_interaction(ctx)
        .author_id(author_id)
        .timeout(std::time::Duration::from_secs(120))
        .await
}

async fn get_table_info_full(
    ctx: &Context<'_>,
    table_name: &str,
) -> Result<(i64, Vec<String>, Vec<Vec<String>>), Error> {
    let base_settings_db = ctx.data().base_settings_db.clone();
    let table_name = table_name.to_string();

    let result = base_settings_db
        .call(move |conn| {
            // 獲取表的行數
            let count_query = format!("SELECT COUNT(*) FROM \"{}\"", table_name);
            let count: i64 = conn.query_row(&count_query, [], |row| row.get(0))?;

            // 獲取全部數據
            let all_query = format!("SELECT * FROM \"{}\"", table_name);
            let mut all_stmt = conn.prepare(&all_query)?;
            let column_names: Vec<String> = (0..all_stmt.column_count())
                .map(|i| all_stmt.column_name(i).unwrap_or("?").to_string())
                .collect();

            let mut all_data = Vec::new();
            let mut rows = all_stmt.query([])?;
            while let Some(row) = rows.next()? {
                let mut row_values = Vec::new();
                for i in 0..column_names.len() {
                    let value: String = row.get(i).unwrap_or_default();
                    row_values.push(value);
                }
                all_data.push(row_values);
            }

            Ok((count, column_names, all_data))
        })
        .await
        .map_err(|e| {
            log::error!("獲取資料表信息失敗: {}", e);
            Error::msg("獲取資料表信息失敗")
        })?;

    Ok(result)
}
