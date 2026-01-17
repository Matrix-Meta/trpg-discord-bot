use crate::bot::{Context, Error};
use poise::CreateReply;
use poise::serenity_prelude as serenity;
use tokio_rusqlite::params;

#[derive(Debug, Clone)]
struct EffectRecord {
    name: String,
    category: String,
    description: String,
    note: Option<String>,
}

/// 查詢異常狀態效果（支援模糊搜尋）
#[poise::command(slash_command)]
pub async fn effect(
    ctx: Context<'_>,
    #[description = "異常狀態名稱或關鍵字"] keyword: String,
    #[description = "最多顯示幾筆結果，預設為 5"]
    #[min = 1]
    #[max = 20]
    limit: Option<u8>,
) -> Result<(), Error> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        let embed = serenity::CreateEmbed::default()
            .title("錯誤")
            .description("請提供要搜尋的異常狀態名稱或關鍵字。")
            .color(serenity::Colour::RED);
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let limit = limit.unwrap_or(5) as usize;

    // TODO: 將查詢改為真正的 base-settings.db 連線
    let matches = fetch_effects_from_base(ctx, keyword, limit).await?;

    if matches.is_empty() {
        let embed = serenity::CreateEmbed::default()
            .title(format!("搜索結果：<{}>", keyword))
            .description("找不到符合的異常狀態效果。")
            .color(serenity::Colour::ORANGE);
        ctx.send(CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let mut embed = serenity::CreateEmbed::default()
        .title(format!("搜索結果：<{}>", keyword))
        .color(serenity::Colour::FOOYOO);

    for EffectRecord {
        name,
        category,
        description,
        note,
    } in matches
    {
        let mut value = description;
        if let Some(note) = note {
            value.push_str(&format!("\n**備註**: {}", note));
        }
        embed = embed.field(
            if category.is_empty() {
                name
            } else {
                format!("{} ({})", name, category)
            },
            value,
            false,
        );
    }

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

async fn fetch_effects_from_base(
    ctx: Context<'_>,
    keyword: &str,
    limit: usize,
) -> Result<Vec<EffectRecord>, Error> {
    let keyword_lower = keyword.to_lowercase();
    let base_settings_db = ctx.data().base_settings_db.clone();
    let keyword_pattern = format!("%{}%", keyword_lower);

    let results = base_settings_db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT category, name, description FROM 異常狀態 \
                WHERE LOWER(name) LIKE ?1 OR LOWER(category) LIKE ?1 OR LOWER(description) LIKE ?1 \
                ORDER BY CASE \
                    WHEN LOWER(name) LIKE ?1 THEN 1 \
                    WHEN LOWER(category) LIKE ?1 THEN 2 \
                    WHEN LOWER(description) LIKE ?1 THEN 3 \
                    ELSE 4 \
                END, \
                ABS(LENGTH(name) - LENGTH(?2)) ASC, name \
                LIMIT ?3",
            )?;

            let rows = stmt.query_map(params![keyword_pattern, keyword_lower, limit], |row| {
                Ok(EffectRecord {
                    category: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    note: None, // 異常狀態表沒有note欄位，所以設為None
                })
            })?;

            let mut effects = Vec::new();
            for effect in rows.flatten() {
                effects.push(effect);
            }

            Ok(effects)
        })
        .await
        .map_err(|e| Error::msg(format!("資料庫查詢錯誤: {}", e)))?;

    Ok(results)
}
