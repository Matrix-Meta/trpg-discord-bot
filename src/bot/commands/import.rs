use crate::bot::{Context, Error};
use crate::utils::import::{FileType, ImportService};
use std::path::Path;

// 根據檔案名稱或內容類型推斷檔案類型
fn detect_file_type(filename: &str, content_type: &str) -> FileType {
    // 檢查是否是 Google Sheets URL，如果是則根據導出格式決定檔案類型
    if filename.contains("docs.google.com/spreadsheets") && filename.contains("/export?") {
        if filename.contains("format=xlsx") {
            return FileType::Xlsx;
        } else if filename.contains("format=ods") {
            return FileType::Ods;
        } else {
            // 預設為 CSV 對於 Google Sheets 導出
            return FileType::Csv;
        }
    }

    // 優先根據副檔名檢測
    if let Some(ext) = Path::new(filename).extension() {
        if let Some(ext_str) = ext.to_str() {
            return FileType::from_extension(ext_str);
        }
    }

    // 退而求其次根據內容類型檢測
    FileType::from_content_type(content_type)
}

/// 從雲端獲取文件並導入至機器人資料庫
#[poise::command(slash_command)]
pub async fn import_data(
    ctx: Context<'_>,
    #[description = "文件的 URL 或共享連結"] url: String,
    #[description = "目標資料表名稱前綴（對於多工作表文件，將為每個工作表創建表：{前綴}_{工作表名}）"]
    table_name: String,
    #[description = "手動指定檔案類型 (csv, xlsx, xls, ods, json, tsv)，留空則自動檢測"] file_type: Option<String>,
    #[description = "對於多工作表文件，指定要導入的工作表名稱，留空則導入所有工作表"]
    sheet_name: Option<String>,
) -> Result<(), Error> {
    // 檢查執行者是否為管理員或開發者
    let has_permission = {
        let author_id = ctx.author().id.get();

        // 取得伺服器中的成員資訊和權限
        let is_admin = if let Some(guild_id) = ctx.guild_id() {
            let member = guild_id
                .member(&ctx.serenity_context().http, ctx.author().id)
                .await
                .map_err(|_| Error::msg("無法取得成員資訊"))?;
            member
                .permissions(&ctx.serenity_context().cache)
                .map(|perms| perms.administrator())
                .unwrap_or(false)
        } else {
            false // 在私人頻道中，用戶不可能是管理員
        };

        let config_manager = ctx.data().config.lock().await;
        let is_developer = config_manager.is_developer(author_id).await;
        is_admin || is_developer
    };

    if !has_permission {
        ctx.say("您沒有權限執行此指令。僅限伺服器管理員或已註冊開發者使用。")
            .await?;
        return Ok(());
    }

    log::info!(
        "開始導入數據: {} 到表 {}，工作表: {:?}，檔案類型: {:?}",
        url,
        table_name,
        sheet_name,
        file_type
    );

    ctx.say("開始導入數據...").await?;

    // 從雲端服務獲取文件內容，傳遞使用者指定的檔案類型以優化 Google Sheets URL
    const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
    let (file_bytes, content_type) =
        ImportService::fetch_file_content(&url, file_type.as_deref(), MAX_IMPORT_BYTES)
            .await
            .map_err(|e| {
                let error_msg = format!("獲取文件失敗: {}", e);
                log::error!("{}", error_msg);
                Error::msg(error_msg)
            })?;

    log::info!(
        "文件獲取成功，內容類型: {}，文件大小: {} 字節",
        content_type,
        file_bytes.len()
    );

    // 檢測檔案類型 - 對於 Google Sheets URL，優先使用自動檢測而非手動指定
    let detected_file_type = if url.contains("docs.google.com/spreadsheets") {
        // 對於 Google Sheets，始終使用自動檢測，因為手動指定的類型可能與實際取得的格式不匹配
        let detected_type = detect_file_type(&url, &content_type);
        log::info!(
            "Google Sheets URL 檢測到，自動選擇檔案類型: {:?} (從 '{}' 檢測，因為手動指定的類型可能不匹配實際內容)",
            detected_type,
            url
        );
        detected_type
    } else {
        // 對於非 Google Sheets URL，優先使用手動指定的類型
        match file_type {
            Some(ft) => {
                log::info!("手動指定檔案類型: {}", ft);
                FileType::from_extension(&ft)
            }
            None => {
                let detected_type = detect_file_type(&url, &content_type);
                log::info!("自動檢測檔案類型: {:?} (從 '{}' 檢測)", detected_type, url);
                detected_type
            }
        }
    };

    log::info!(
        "開始處理文件並注入資料庫，目標表前綴: {}，實際檔案類型: {:?}，目標工作表: {:?}",
        table_name,
        detected_file_type,
        sheet_name
    );

    // 呼叫服務層處理文件並注入資料庫
    ImportService::process_and_inject(
        &ctx.data().base_settings_db,
        &table_name,
        file_bytes.clone(),
        detected_file_type.clone(),
        sheet_name.clone()
    ).await
    .map_err(|e| {
        let error_msg = format!("處理文件失敗: {}", e);
        log::error!("{}", error_msg);

        // 提供更詳細的錯誤上下文
        let detailed_error = format!(
            "處理文件失敗: {}\n\n診斷資訊:\n- 原始 URL: {}\n- 檔案類型: {:?}\n- 目標表: {}\n- 目標工作表: {:?}\n- 內容類型: {}\n- 文件大小: {} 字節\n- 檔案大小限制: {} 字節\n\n除錯建議:\n  1. 檔案連結是否正確且可公開存取\n  2. 檔案格式與指定類型是否匹配\n  3. 檔案結構是否完整（表頭、數據格式等）\n  4. 如果是 Google Sheets，請確認已發布為公開存取\n  5. 檢查檔案大小是否過大\n  6. 確認工作表名稱是否存在",
            e, url, detected_file_type, table_name, sheet_name, content_type, file_bytes.len(), MAX_IMPORT_BYTES
        );
        log::error!("詳細錯誤診斷:\n{}", detailed_error);
        Error::msg(error_msg)
    })?;

    let response = format!("成功將 '{}' 的數據導入到資料表 '{}'", url, table_name);
    log::info!("{}", response);
    ctx.say(response).await?;

    Ok(())
}
