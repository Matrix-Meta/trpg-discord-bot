use calamine::{Reader as CalamineReader, open_workbook_auto};
use csv;
use regex::Regex;
use std::io::Cursor;
use tokio_rusqlite::params;
use uuid;

#[derive(Debug, Clone)]
pub enum FileType {
    Csv,
    Xlsx,
    Xls,
    Ods,
    Json,
    Tsv,
    Unknown,
}

impl FileType {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "csv" => FileType::Csv,
            "xlsx" => FileType::Xlsx,
            "xls" => FileType::Xls,
            "ods" => FileType::Ods,
            "json" => FileType::Json,
            "tsv" => FileType::Tsv,
            _ => FileType::Unknown,
        }
    }
}

impl FileType {
    pub fn from_content_type(content_type: &str) -> Self {
        match content_type.to_lowercase().as_str() {
            t if t.contains("csv") => FileType::Csv,
            t if t.contains("excel") || t.contains("spreadsheetml") => FileType::Xlsx,
            t if t.contains("xls") => FileType::Xls,
            t if t.contains("ods") => FileType::Ods,
            t if t.contains("json") => FileType::Json,
            t if t.contains("tsv") => FileType::Tsv,
            _ => FileType::Unknown,
        }
    }
}

pub struct ImportService;

impl ImportService {
    ///從雲端獲取文件內容
    /// 從 URL 獲取文件內容
    pub async fn fetch_file_content(
        identifier: &str,
        expected_file_type: Option<&str>,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>> {
        // 檢查是否是 Google Sheets URL，如果是則嘗試轉換為導出 URL
        let actual_url = if identifier.contains("docs.google.com/spreadsheets") {
            // 嘗試解析不同類型的 Google Sheets URLs
            if identifier.contains("/export?") {
                // 如果 URL 已是導出格式，直接使用
                log::info!("使用已提供的 Google Sheets 導出 URL: {}", identifier);
                identifier.to_string()
            } else if identifier.contains("/d/") {
                // 這是一個 Google Sheets 文件 URL，嘗試轉換為指定格式的導出
                // 基本格式: https://docs.google.com/spreadsheets/d/SPREADSHEET_ID/edit...
                let parts: Vec<&str> = identifier.split("/d/").collect();
                if parts.len() > 1 {
                    let id_part = parts[1].split("/edit").next().unwrap_or(parts[1]);
                    // 根據使用者指定的檔案類型選擇導出格式，如果未指定則預設為 csv
                    let format = match expected_file_type {
                        Some(ft) => match ft.to_lowercase().as_str() {
                            "xlsx" => "xlsx",
                            "xls" => "xls",
                            "ods" => "ods",
                            "csv" => "csv",
                            "tsv" => "tsv",
                            _ => {
                                log::warn!("未知的檔案類型 '{}', 使用預設格式: csv", ft);
                                "csv"
                            }
                        },
                        None => "csv", // 預設使用 csv 格式
                    };
                    let export_url = format!(
                        "https://docs.google.com/spreadsheets/d/{}/export?format={}",
                        id_part, format
                    );
                    log::info!(
                        "轉換 Google Sheets URL: {} -> {} (根據使用者指定格式: {})",
                        identifier,
                        export_url,
                        format
                    );
                    export_url
                } else {
                    log::warn!("無法解析 Google Sheets URL: {}，使用原始 URL", identifier);
                    // 如果解析失敗，使用原始 URL
                    identifier.to_string()
                }
            } else {
                log::info!(
                    "未識別到標準 Google Sheets 格式，使用原始 URL: {}",
                    identifier
                );
                // 不是標準的 Google Sheets URL 格式，使用原始 URL
                identifier.to_string()
            }
        } else {
            identifier.to_string()
        };

        log::info!("嘗試獲取文件內容: {}", actual_url);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let response = client
            .get(&actual_url)
            .send()
            .await
            .map_err(|e| {
                let error_msg = format!("無法連接到 URL: {} - 請檢查:\n  1. 網路連線是否正常\n  2. URL 是否正確且可公開存取\n  3. 若是 Google Sheets，請確認已發布為公開存取\n  4. URL 格式是否正確\n詳細錯誤: {}", actual_url, e);
                log::error!("{}", error_msg);
                error_msg
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_msg = format!(
                "HTTP 請求失敗: {} (狀態碼: {})\n請檢查:\n  1. URL 是否正確且可公開存取\n  2. 若是 Google Sheets，請確認已發布為公開存取\n  3. 網站是否正常運作",
                status, actual_url
            );
            log::error!("{}", error_msg);
            return Err(error_msg.into());
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        if let Some(length) = response.content_length() {
            if length as usize > max_bytes {
                return Err(
                    format!("檔案過大，大小 {} bytes，限制 {} bytes", length, max_bytes).into(),
                );
            }
        }

        log::info!("獲取成功，內容類型: {}", content_type);

        let bytes = response.bytes().await?;
        if bytes.len() > max_bytes {
            return Err(format!(
                "檔案過大，大小 {} bytes，限制 {} bytes",
                bytes.len(),
                max_bytes
            )
            .into());
        }
        log::info!("獲取文件大小: {} 字節", bytes.len());

        Ok((bytes.to_vec(), content_type))
    }

    ///解析文件內容並注入資料庫
    pub async fn process_and_inject(
        db: &tokio_rusqlite::Connection,
        table_name: &str,
        file_bytes: Vec<u8>,
        file_type: FileType,
        sheet_name: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match file_type {
            FileType::Csv => {
                let content = String::from_utf8_lossy(&file_bytes);
                Self::process_csv(db, table_name, &content).await
                    .map_err(|e| {
                        format!("CSV 處理失敗: {}\n診斷資訊:\n  1. 請確認檔案為有效的 CSV 格式\n  2. 檢查檔案編碼是否為 UTF-8\n  3. 確認檔案結構包含表頭和數據行\n  4. 檢查是否有特殊字符導致解析錯誤\n詳細錯誤: {}", e, e)
                    })?;
            }
            FileType::Tsv => {
                let content = String::from_utf8_lossy(&file_bytes);
                Self::process_tsv(db, table_name, &content).await
                    .map_err(|e| {
                        format!("TSV 處理失敗: {}\n診斷資訊:\n  1. 請確認檔案為有效的 TSV 格式\n  2. 檢查檔案編碼是否為 UTF-8\n  3. 確認檔案結構包含表頭和數據行\n  4. 檢查是否有特殊字符導致解析錯誤\n詳細錯誤: {}", e, e)
                    })?;
            }
            FileType::Json => {
                let content = String::from_utf8_lossy(&file_bytes);
                Self::process_json(db, table_name, &content).await
                    .map_err(|e| {
                        format!("JSON 處理失敗: {}\n診斷資訊:\n  1. 請確認檔案為有效的 JSON 格式\n  2. 檢查檔案結構是否為對象或對象數組\n  3. 確認 JSON 語法正確（括號、引號、逗號等）\n  4. 檢查是否有特殊字符或不可見字符\n詳細錯誤: {}", e, e)
                    })?;
            }
            FileType::Xlsx | FileType::Xls | FileType::Ods => {
                Self::process_spreadsheet(db, table_name, file_bytes, file_type, sheet_name).await
                    .map_err(|e| {
                        format!("試算表處理失敗: {}\n診斷資訊:\n  1. 請確認檔案為有效的 Excel/ODS 格式\n  2. 檢查檔案是否損壞或加密\n  3. 確認工作表名稱是否存在且正確\n  4. 檢查檔案大小是否過大\n詳細錯誤: {}", e, e)
                    })?;
            }
            FileType::Unknown => {
                return Err("無法識別的檔案類型\n診斷資訊:\n  1. 請確認您提供的是支援的檔案格式 (CSV, XLSX, XLS, ODS, JSON, TSV)\n  2. 檢查 URL 或檔案類型參數是否正確\n  3. 若自動檢測失敗，請手動指定檔案類型".into());
            }
        }

        Ok(())
    }

    async fn process_csv(
        db: &tokio_rusqlite::Connection,
        table_name: &str,
        csv_data: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db_clone = db.clone();

        // 解析 CSV 獲取表頭
        let mut reader = csv::Reader::from_reader(Cursor::new(csv_data));
        let headers = reader.headers()
            .map_err(|e| {
                format!("解析 CSV 標題行失敗: {}\n診斷資訊:\n  1. 請確認 CSV 檔案包含有效的標題行\n  2. 檢查檔案編碼是否為 UTF-8\n  3. 確認檔案內容不為空\n  4. 檢查是否有特殊字符導致解析錯誤\n詳細錯誤: {}", e, e)
            })?.clone();

        if headers.is_empty() {
            return Err("CSV 檔案沒有標題行\n診斷資訊:\n  1. 請確認 CSV 檔案包含標題行\n  2. 檢查檔案是否為空\n  3. 確認檔案結構正確".into());
        }

        log::info!(
            "CSV 檔案包含 {} 個欄位: {:?}",
            headers.len(),
            headers.iter().collect::<Vec<_>>()
        );

        // 確保列名唯一性
        let mut used_names = std::collections::HashMap::new();
        let columns_def: Vec<String> = headers
            .iter()
            .map(|header| {
                let sanitized_header = Self::sanitize_column_name(header);
                let unique_header = {
                    let count = used_names.entry(sanitized_header.clone()).or_insert(0);
                    *count += 1;
                    if *count == 1 {
                        sanitized_header
                    } else {
                        format!("{}_{}", sanitized_header, *count - 1)
                    }
                };
                format!("\"{}\" TEXT", unique_header)
            })
            .collect();

        let columns_str = columns_def.join(", ");
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            table_name, columns_str
        );

        db_clone
            .call(move |conn| {
                conn.execute(&create_sql, params![])
                    .map_err(|e| {
                        log::error!("創建資料表失敗: {}\n診斷資訊:\n  1. 請檢查表名是否有效\n  2. 確認欄位名稱是否符合 SQL 規範\n  3. 檢查資料庫是否可寫入\n  4. 檢查欄位數量是否過多\n詳細錯誤: {}", e, e);
                        e // 返回原始錯誤類型
                    })?;
                Ok(())
            })
            .await
            .map_err(|e| {
                format!("創建資料表失敗: {}\n診斷資訊:\n  1. 請檢查表名是否有效\n  2. 確認欄位名稱是否符合 SQL 規範\n  3. 檢查資料庫是否可寫入\n  4. 檢查欄位數量是否過多\n詳細錯誤: {}", e, e)
            })?;

        // 準備插入語句
        let insert_sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
            table_name,
            headers
                .iter()
                .map(|h| format!("\"{}\"", Self::sanitize_column_name(h)))
                .collect::<Vec<_>>()
                .join(", "),
            (0..headers.len())
                .map(|_| "?".to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // 插入數據
        let mut row_count = 0;
        for result in reader.records() {
            let record = result
                .map_err(|e| {
                    format!("解析 CSV 記錄失敗: {}\n診斷資訊:\n  1. 檢查檔案格式是否正確\n  2. 確認是否有特殊字符或未閉合的引號\n  3. 檢查記錄是否包含不可見字符\n  4. 驗證 CSV 格式是否符合 RFC 4180 標準\n詳細錯誤: {}", e, e)
                })?;

            let values: Vec<String> = record.iter().map(|s| s.trim().to_string()).collect();
            let insert_sql_clone = insert_sql.clone();

            db_clone
                .call(move |conn| {
                    let mut stmt = conn.prepare(&insert_sql_clone)
                        .map_err(|e| {
                            log::error!("準備 SQL 語句失敗: {}\n診斷資訊:\n  1. 檢查參數數量是否超過限制\n  2. 確認 SQL 語法是否正確\n  3. 驗證欄位數量與值數量是否匹配\n詳細錯誤: {}", e, e);
                            e // 返回原始錯誤類型
                        })?;

                    match values.len() {
                        1 => stmt.execute(params![&values[0]])?,
                        2 => stmt.execute(params![&values[0], &values[1]])?,
                        3 => stmt.execute(params![&values[0], &values[1], &values[2]])?,
                        4 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3]])?,
                        5 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4]])?,
                        6 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5]])?,
                        7 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6]])?,
                        8 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7]])?,
                        n if n <= 16 => {
                            // For longer parameter lists up to 16, use a generic approach
                            match n {
                                9 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8]])?,
                                10 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9]])?,
                                11 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10]])?,
                                12 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11]])?,
                                13 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12]])?,
                                14 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12], &values[13]])?,
                                15 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12], &values[13], &values[14]])?,
                                16 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12], &values[13], &values[14], &values[15]])?,
                                _ => stmt.execute(params![&values[0]])?, // fallback
                            }
                        },
                        _ => {
                            // For more than 16 parameters, just use the first 16
                            match values.len() {
                                0 => stmt.execute(params![])?,
                                1 => stmt.execute(params![&values[0]])?,
                                2 => stmt.execute(params![&values[0], &values[1]])?,
                                3 => stmt.execute(params![&values[0], &values[1], &values[2]])?,
                                4 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3]])?,
                                5 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4]])?,
                                6 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5]])?,
                                7 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6]])?,
                                8 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7]])?,
                                _ => stmt.execute(params![&values[0]])?, // fallback
                            }
                        }
                    };
                    Ok(())
                })
                .await
                .map_err(|e| {
                    format!("插入第 {} 行數據失敗: {}\n診斷資訊:\n  1. 檢查該行數據格式是否正確\n  2. 確認欄位數量與標題行是否匹配\n  3. 驗證數據類型是否符合預期\n  4. 檢查是否有過長的字符串\n詳細錯誤: {}", row_count + 1, e, e)
                })?;

            row_count += 1;
        }

        log::info!("成功處理 {} 行 CSV 數據", row_count);
        Ok(())
    }

    async fn process_tsv(
        db: &tokio_rusqlite::Connection,
        table_name: &str,
        tsv_data: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db_clone = db.clone();

        let mut lines = tsv_data.lines();
        let header_line = lines.next()
            .ok_or("TSV 檔案無標題行\n診斷資訊:\n  1. 請確認 TSV 檔案包含標題行\n  2. 檢查檔案是否為空\n  3. 確認檔案結構正確")?;
        let headers: Vec<&str> = header_line.split('\t').collect();

        if headers.is_empty() {
            return Err("TSV 檔案標題行為空\n診斷資訊:\n  1. 請確認 TSV 檔案標題行包含有效欄位\n  2. 檢查標題行中是否有正確的製表符分隔\n  3. 確認檔案編碼是否正確".into());
        }

        log::info!("TSV 檔案包含 {} 個欄位: {:?}", headers.len(), headers);

        // 確保列名唯一性
        let mut used_names = std::collections::HashMap::new();
        let columns_def: Vec<String> = headers
            .iter()
            .map(|&header| {
                let sanitized_header = Self::sanitize_column_name(header);
                let unique_header = {
                    let count = used_names.entry(sanitized_header.clone()).or_insert(0);
                    *count += 1;
                    if *count == 1 {
                        sanitized_header
                    } else {
                        format!("{}_{}", sanitized_header, *count - 1)
                    }
                };
                format!("\"{}\" TEXT", unique_header)
            })
            .collect();

        let columns_str = columns_def.join(", ");
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            table_name, columns_str
        );

        db_clone
            .call(move |conn| {
                conn.execute(&create_sql, params![])
                    .map_err(|e| {
                        log::error!("創建資料表失敗: {}\n診斷資訊:\n  1. 請檢查表名是否有效\n  2. 確認欄位名稱是否符合 SQL 規範\n  3. 檢查資料庫是否可寫入\n  4. 檢查欄位數量是否過多\n詳細錯誤: {}", e, e);
                        e // 返回原始錯誤類型
                    })?;
                Ok(())
            })
            .await
            .map_err(|e| {
                format!("創建資料表失敗: {}\n診斷資訊:\n  1. 請檢查表名是否有效\n  2. 確認欄位名稱是否符合 SQL 規範\n  3. 檢查資料庫是否可寫入\n  4. 檢查欄位數量是否過多\n詳細錯誤: {}", e, e)
            })?;

        // 準備插入語句
        let insert_sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
            table_name,
            {
                // 確保列名唯一性
                let mut used_names = std::collections::HashMap::new();
                headers
                    .iter()
                    .map(|&h| {
                        let sanitized_header = Self::sanitize_column_name(h);
                        let unique_header = {
                            let count = used_names.entry(sanitized_header.clone()).or_insert(0);
                            *count += 1;
                            if *count == 1 {
                                sanitized_header
                            } else {
                                format!("{}_{}", sanitized_header, *count - 1)
                            }
                        };
                        format!("\"{}\"", unique_header)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            (0..headers.len())
                .map(|_| "?".to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // 插入數據
        let mut row_index = 1; // 從 1 開始計算，包括標題行
        for line in lines {
            row_index += 1;
            let values: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();

            // 驗證列數是否與標題匹配
            if values.len() != headers.len() {
                log::warn!(
                    "第 {} 行列數不匹配: 預期 {} 個，實際 {} 個",
                    row_index,
                    headers.len(),
                    values.len()
                );
            }

            let insert_sql_clone = insert_sql.clone();

            db_clone
                .call(move |conn| {
                    let mut stmt = conn.prepare(&insert_sql_clone)
                        .map_err(|e| {
                            log::error!("準備 SQL 語句失敗: {}\n診斷資訊:\n  1. 檢查參數數量是否超過限制\n  2. 確認 SQL 語法是否正確\n  3. 驗證欄位數量與值數量是否匹配\n詳細錯誤: {}", e, e);
                            e // 返回原始錯誤類型
                        })?;

                    match values.len() {
                        1 => stmt.execute(params![&values[0]])?,
                        2 => stmt.execute(params![&values[0], &values[1]])?,
                        3 => stmt.execute(params![&values[0], &values[1], &values[2]])?,
                        4 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3]])?,
                        5 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4]])?,
                        6 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5]])?,
                        7 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6]])?,
                        8 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7]])?,
                        n if n <= 16 => {
                            // For longer parameter lists up to 16, use a generic approach
                            match n {
                                9 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8]])?,
                                10 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9]])?,
                                11 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10]])?,
                                12 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11]])?,
                                13 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12]])?,
                                14 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12], &values[13]])?,
                                15 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12], &values[13], &values[14]])?,
                                16 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7], &values[8], &values[9], &values[10], &values[11], &values[12], &values[13], &values[14], &values[15]])?,
                                _ => stmt.execute(params![&values[0]])?, // fallback
                            }
                        },
                        _ => {
                            // For more than 16 parameters, just use the first 16
                            match values.len() {
                                0 => stmt.execute(params![])?,
                                1 => stmt.execute(params![&values[0]])?,
                                2 => stmt.execute(params![&values[0], &values[1]])?,
                                3 => stmt.execute(params![&values[0], &values[1], &values[2]])?,
                                4 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3]])?,
                                5 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4]])?,
                                6 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5]])?,
                                7 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6]])?,
                                8 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7]])?,
                                _ => stmt.execute(params![&values[0]])?, // fallback
                            }
                        }
                    };
                    Ok(())
                })
                .await
                .map_err(|e| {
                    format!("插入第 {} 行數據失敗: {}\n診斷資訊:\n  1. 檢查該行數據格式是否正確\n  2. 確認欄位數量與標題行是否匹配\n  3. 驗證數據類型是否符合預期\n  4. 檢查是否有過長的字符串\n詳細錯誤: {}", row_index, e, e)
                })?;
        }

        log::info!("成功處理 {} 行 TSV 數據", row_index - 1); // 減去標題行
        Ok(())
    }

    async fn process_json(
        db: &tokio_rusqlite::Connection,
        table_name: &str,
        json_data: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db_clone = db.clone();

        // 解析 JSON
        let json_value: serde_json::Value = serde_json::from_str(json_data)
            .map_err(|e| {
                format!("JSON 解析失敗: {}\n診斷資訊:\n  1. 請確認 JSON 語法正確（括號、引號、逗號等）\n  2. 檢查是否有特殊字符或不可見字符\n  3. 確認檔案編碼是否為 UTF-8\n  4. 驗證檔案是否完整沒有截斷\n詳細錯誤: {}", e, e)
            })?;

        // 檢查是否為數組
        let items = match json_value {
            serde_json::Value::Array(arr) => {
                log::info!("JSON 檔案包含 {} 個項目", arr.len());
                arr
            },
            serde_json::Value::Object(_) => {
                log::info!("JSON 檔案包含單一物件，將轉換為陣列處理");
                vec![json_value]
            },
            _ => return Err("JSON 格式不正確，應為對象或對象數組\n診斷資訊:\n  1. 請確認 JSON 根層是物件或物件陣列\n  2. 檢查是否為有效的 JSON 格式\n  3. 確認檔案內容符合預期格式".into()),
        };

        if items.is_empty() {
            return Err("JSON 檔案為空\n診斷資訊:\n  1. 請確認 JSON 檔案包含有效數據\n  2. 檢查檔案是否被正確解析\n  3. 確認檔案內容沒有只是空白或註釋".into());
        }

        // 從第一個項目提取鍵作為表頭
        let mut all_keys = std::collections::HashSet::new();

        if let serde_json::Value::Object(obj) = &items[0] {
            for key in obj.keys() {
                all_keys.insert(key.clone());
            }
        }

        // 檢查所有項目是否有一致的鍵
        for (index, item) in items.iter().enumerate() {
            if let serde_json::Value::Object(obj) = item {
                for key in obj.keys() {
                    all_keys.insert(key.clone());
                }
            } else {
                return Err(format!("JSON 數據中第 {} 個項目不是物件\n診斷資訊:\n  1. 請確認所有 JSON 項目都是物件格式\n  2. 檢查數據結構是否一致\n  3. 確認檔案格式符合預期", index + 1).into());
            }
        }

        let headers: Vec<String> = all_keys.into_iter().collect();
        log::info!(
            "從 JSON 數據中檢測到 {} 個標題: {:?}",
            headers.len(),
            headers
        );

        if headers.is_empty() {
            return Err("JSON 檔案中沒有檢測到任何欄位\n診斷資訊:\n  1. 請確認 JSON 項目包含有效鍵值對\n  2. 檢查物件是否為空\n  3. 確認數據結構符合預期".into());
        }

        // 確保列名唯一性
        let mut used_names = std::collections::HashMap::new();
        let columns_def: Vec<String> = headers
            .iter()
            .map(|header| {
                let sanitized_header = Self::sanitize_column_name(header);
                let unique_header = {
                    let count = used_names.entry(sanitized_header.clone()).or_insert(0);
                    *count += 1;
                    if *count == 1 {
                        sanitized_header
                    } else {
                        format!("{}_{}", sanitized_header, *count - 1)
                    }
                };
                format!("\"{}\" TEXT", unique_header)
            })
            .collect();

        let columns_str = columns_def.join(", ");
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            table_name, columns_str
        );

        db_clone
            .call(move |conn| {
                conn.execute(&create_sql, params![])
                    .map_err(|e| {
                        log::error!("創建資料表失敗: {}\n診斷資訊:\n  1. 請檢查表名是否有效\n  2. 確認欄位名稱是否符合 SQL 規範\n  3. 檢查資料庫是否可寫入\n  4. 檢查欄位數量是否過多\n詳細錯誤: {}", e, e);
                        e // 返回原始錯誤類型
                    })?;
                Ok(())
            })
            .await
            .map_err(|e| {
                format!("創建資料表失敗: {}\n診斷資訊:\n  1. 請檢查表名是否有效\n  2. 確認欄位名稱是否符合 SQL 規範\n  3. 檢查資料庫是否可寫入\n  4. 檢查欄位數量是否過多\n詳細錯誤: {}", e, e)
            })?;

        // 準備插入語句
        let insert_sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
            table_name,
            {
                // 確保列名唯一性
                let mut used_names_insert = std::collections::HashMap::new();
                headers
                    .iter()
                    .map(|h| {
                        let sanitized_header = Self::sanitize_column_name(h);
                        let unique_header = {
                            let count = used_names_insert
                                .entry(sanitized_header.clone())
                                .or_insert(0);
                            *count += 1;
                            if *count == 1 {
                                sanitized_header
                            } else {
                                format!("{}_{}", sanitized_header, *count - 1)
                            }
                        };
                        format!("\"{}\"", unique_header)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            (0..headers.len())
                .map(|_| "?".to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // 插入數據
        for (index, item) in items.iter().enumerate() {
            if let serde_json::Value::Object(obj) = item {
                let mut values = Vec::new();

                for header in &headers {
                    let value = obj
                        .get(header)
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Null => "".to_string(),
                            _ => v.to_string(), // For arrays/objects, convert to string representation
                        })
                        .unwrap_or_else(|| "".to_string());
                    values.push(value);
                }

                let insert_sql_clone = insert_sql.clone();
                db_clone
                    .call(move |conn| {
                    let mut stmt = conn.prepare(&insert_sql_clone)
                        .map_err(|e| {
                            log::error!("準備 SQL 語句失敗: {}\n診斷資訊:\n  1. 檢查參數數量是否超過限制\n  2. 確認 SQL 語法是否正確\n  3. 驗證欄位數量與值數量是否匹配\n詳細錯誤: {}", e, e);
                            e // 返回原始錯誤類型
                        })?;

                    match values.len() {
                        1 => stmt.execute(params![&values[0]])?,
                        2 => stmt.execute(params![&values[0], &values[1]])?,
                        3 => stmt.execute(params![&values[0], &values[1], &values[2]])?,
                        4 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3]])?,
                        5 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4]])?,
                        6 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5]])?,
                        7 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6]])?,
                        8 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7]])?,
                        n if n <= 16 => {
                            // For longer parameter lists up to 16, use a generic approach
                            let values_owned: Vec<&str> = values.iter().map(|s| s.as_str()).collect();
                            // Execute with a fixed maximum number of params (pad with empty strings if needed)
                            match n {
                                9 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8]])?,
                                10 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9]])?,
                                11 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9], &values_owned[10]])?,
                                12 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9], &values_owned[10], &values_owned[11]])?,
                                13 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9], &values_owned[10], &values_owned[11], &values_owned[12]])?,
                                14 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9], &values_owned[10], &values_owned[11], &values_owned[12], &values_owned[13]])?,
                                15 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9], &values_owned[10], &values_owned[11], &values_owned[12], &values_owned[13], &values_owned[14]])?,
                                16 => stmt.execute(params![&values_owned[0], &values_owned[1], &values_owned[2], &values_owned[3], &values_owned[4], &values_owned[5], &values_owned[6], &values_owned[7], &values_owned[8], &values_owned[9], &values_owned[10], &values_owned[11], &values_owned[12], &values_owned[13], &values_owned[14], &values_owned[15]])?,
                                _ => stmt.execute(params![&values_owned[0]])?, // fallback
                            }
                        },
                        _ => {
                            // For more than 16 parameters, just use the first 16
                            match values.len() {
                                0 => stmt.execute(params![])?,
                                1 => stmt.execute(params![&values[0]])?,
                                2 => stmt.execute(params![&values[0], &values[1]])?,
                                3 => stmt.execute(params![&values[0], &values[1], &values[2]])?,
                                4 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3]])?,
                                5 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4]])?,
                                6 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5]])?,
                                7 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6]])?,
                                8 => stmt.execute(params![&values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6], &values[7]])?,
                                _ => stmt.execute(params![&values[0]])?, // fallback
                            }
                        }
                    };
                        Ok(())
                    })
                    .await
                    .map_err(|e| {
                        format!("插入第 {} 個 JSON 項目失敗: {}\n診斷資訊:\n  1. 檢查該項目數據格式是否正確\n  2. 確認欄位數量與預期是否匹配\n  3. 驗證數據類型是否符合預期\n  4. 檢查是否有過長的字符串\n詳細錯誤: {}", index + 1, e, e)
                    })?;
            } else {
                return Err(format!("JSON 數據中第 {} 個項目不是物件，無法處理\n診斷資訊:\n  1. 請確認所有 JSON 項目都是物件格式\n  2. 檢查數據結構是否一致\n  3. 確認檔案格式符合預期", index + 1).into());
            }
        }

        log::info!("成功處理 {} 個 JSON 項目", items.len());
        Ok(())
    }

    async fn process_spreadsheet(
        db: &tokio_rusqlite::Connection,
        table_name_prefix: &str,
        file_data: Vec<u8>,
        file_type: FileType,
        sheet_filter: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db_clone = db.clone();

        // 使用 tempfile 創建臨時文件來處理 Excel
        let temp_dir = std::env::temp_dir();
        let extension = match file_type {
            FileType::Xlsx => "xlsx",
            FileType::Xls => "xls",
            FileType::Ods => "ods",
            _ => "tmp",
        };
        let temp_filename = format!("temp_import_{}.{}", uuid::Uuid::new_v4(), extension);
        let temp_path = temp_dir.join(temp_filename);

        std::fs::write(&temp_path, file_data)
            .map_err(|e| {
                format!("創建臨時文件失敗: {}\n診斷資訊:\n  1. 檢查磁碟空間是否充足\n  2. 確認臨時目錄可寫入\n  3. 驗證檔案數據是否完整\n詳細錯誤: {}", e, e)
            })?;

        // 使用 calamine 自動檢測格式
        let mut workbook = open_workbook_auto(&temp_path)
            .map_err(|e| {
                // 清理臨時文件
                let _ = std::fs::remove_file(&temp_path);
                format!("無法打開試算表文件: {:?}\n診斷資訊:\n  1. 請確認檔案為有效的 Excel/ODS 格式\n  2. 檢查檔案是否損壞或加密\n  3. 確認檔案大小是否過大\n  4. 驗證檔案格式與副檔名匹配\n詳細錯誤: {:?}", e, e)
            })?;

        // 獲取所有工作表名稱
        let sheet_names = workbook.sheet_names();
        log::info!("發現 {} 個工作表: {:?}", sheet_names.len(), sheet_names);

        if sheet_names.is_empty() {
            // 清理臨時文件
            let _ = std::fs::remove_file(&temp_path);
            return Err("試算表文件中沒有找到任何工作表\n診斷資訊:\n  1. 請確認試算表文件包含至少一個工作表\n  2. 檢查檔案是否損壞\n  3. 確認檔案格式正確".into());
        }

        // 處理指定的工作表或所有工作表
        let sheets_to_process = match sheet_filter {
            Some(ref filter_name) => {
                if sheet_names.contains(filter_name) {
                    log::info!("指定處理工作表: '{}'", filter_name);
                    vec![filter_name.clone()]
                } else {
                    // 清理臨時文件
                    let _ = std::fs::remove_file(&temp_path);
                    return Err(format!("找不到名為 '{}' 的工作表\n診斷資訊:\n  1. 請確認工作表名稱拼寫正確\n  2. 檢查工作表名稱是否存在於文件中\n  3. 確認工作表名稱是否包含特殊字符\n  4. 驗證工作表名稱大小寫是否匹配", filter_name).into());
                }
            }
            None => {
                log::info!("處理所有工作表: {:?}", sheet_names);
                sheet_names.clone() // 處理所有工作表
            }
        };

        log::info!("準備處理 {} 個工作表", sheets_to_process.len());

        // 為每個工作表創建一個表
        for (sheet_index, sheet_name) in sheets_to_process.iter().enumerate() {
            let actual_sheet_name = sheet_name.clone();
            let table_name = if sheets_to_process.len() > 1 {
                // 如果有多個工作表，使用前綴+工作表名作為表名
                format!(
                    "{}_{}",
                    table_name_prefix,
                    Self::sanitize_table_name(sheet_name)
                )
            } else if sheet_filter.is_none() && sheets_to_process.len() == 1 {
                // 如果只有一個工作表且未指定過濾器，直接使用原始表名
                table_name_prefix.to_string()
            } else {
                // 如果指定了特定工作表，使用原始表名
                table_name_prefix.to_string()
            };

            log::info!(
                "處理第 {} 個工作表: '{}'，目標表名: '{}'",
                sheet_index + 1,
                actual_sheet_name,
                table_name
            );

            // 獲取工作表數據
            let range = workbook.worksheet_range(&actual_sheet_name)
                .map_err(|e| {
                    // 清理臨時文件
                    let _ = std::fs::remove_file(&temp_path);
                    format!("讀取工作表 '{}' 失敗: {:?}\n診斷資訊:\n  1. 請確認工作表是否存在且可讀取\n  2. 檢查工作表是否損壞\n  3. 確認工作表格式是否支援\n詳細錯誤: {:?}", actual_sheet_name, e, e)
                })?;

            if range.is_empty() {
                log::warn!("工作表 '{}' 為空，跳過", actual_sheet_name);
                continue;
            }

            let rows: Vec<Vec<String>> = range
                .rows()
                .map(|row| {
                    row.iter()
                        .map(|cell| match cell {
                            calamine::Data::String(s) => s.clone(),
                            calamine::Data::Float(f) => f.to_string(),
                            calamine::Data::Int(i) => i.to_string(),
                            calamine::Data::Bool(b) => b.to_string(),
                            calamine::Data::Empty => "".to_string(),
                            calamine::Data::DateTime(_) => "".to_string(),
                            calamine::Data::Error(e) => format!("ERROR: {:?}", e),
                            calamine::Data::DateTimeIso(s) => s.clone(),
                            calamine::Data::DurationIso(s) => s.clone(),
                        })
                        .collect()
                })
                .collect();

            if rows.is_empty() {
                log::warn!("工作表 '{}' 為空，跳過", actual_sheet_name);
                continue;
            }

            log::info!("工作表 '{}' 包含 {} 行數據", actual_sheet_name, rows.len());

            // 檢查是否為矩陣型表 (第一行和第一列都是標題)
            let is_matrix = rows.len() > 1 && rows[0].len() > 1 && rows.len() == rows[0].len();

            if is_matrix {
                log::info!("檢測到矩陣型數據表");
                // 處理矩陣型表 (如元素反應表)
                Self::create_matrix_table(&db_clone, &table_name, rows).await
                    .map_err(|e| {
                        // 清理臨時文件
                        let _ = std::fs::remove_file(&temp_path);
                        format!("創建矩陣表失敗: {}\n診斷資訊:\n  1. 請檢查數據結構是否符合矩陣表格式\n  2. 確認第一行和第一列是否都包含標題\n  3. 驗證行列數量是否相等\n詳細錯誤: {}", e, e)
                    })?;
            } else {
                log::info!("處理一般數據表");
                // 處理一般表
                Self::create_general_table(&db_clone, &table_name, rows).await
                    .map_err(|e| {
                        // 清理臨時文件
                        let _ = std::fs::remove_file(&temp_path);
                        format!("創建一般表失敗: {}\n診斷資訊:\n  1. 請檢查數據結構是否為標準行列表格\n  2. 確認第一行是否包含標題\n  3. 驗證數據格式是否正確\n詳細錯誤: {}", e, e)
                    })?;
            }
        }

        // 清理臨時文件
        let _ = std::fs::remove_file(&temp_path);

        Ok(())
    }

    async fn create_matrix_table(
        db: &tokio_rusqlite::Connection,
        table_name: &str,
        rows: Vec<Vec<String>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db_clone = db.clone();

        // 創建矩陣關係表
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" (
                \"row_header\" TEXT,
                \"col_header\" TEXT, 
                \"value\" TEXT,
                PRIMARY KEY (\"row_header\", \"col_header\")
            )",
            table_name
        );

        db_clone
            .call(move |conn| {
                conn.execute(&create_sql, params![])?;
                Ok(())
            })
            .await?;

        // 插入矩陣數據 (跳過第一行和第一列)
        let insert_sql = format!(
            "INSERT OR REPLACE INTO \"{}\" (\"row_header\", \"col_header\", \"value\") VALUES (?, ?, ?)",
            table_name
        );

        let insert_sql_clone = insert_sql.clone();
        if rows.len() > 1 && rows[0].len() > 1 {
            for (i, row) in rows.iter().skip(1).enumerate() {
                let row_header = &rows[i + 1][0]; // 第一列是行標題
                for (j, cell_value) in row.iter().skip(1).enumerate() {
                    if j + 1 < rows[0].len() {
                        let col_header = &rows[0][j + 1]; // 第一行是列標題

                        let row_header_val = row_header.clone();
                        let col_header_val = col_header.clone();
                        let cell_value_val = cell_value.clone();
                        let insert_sql_double_clone = insert_sql_clone.clone();

                        db_clone
                            .call(move |conn| {
                                conn.execute(
                                    &insert_sql_double_clone,
                                    params![row_header_val, col_header_val, cell_value_val],
                                )?;
                                Ok(())
                            })
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn create_general_table(
        db: &tokio_rusqlite::Connection,
        table_name: &str,
        rows: Vec<Vec<String>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if rows.is_empty() {
            return Err("沒有數據可處理".into());
        }

        let db_clone = db.clone();
        let headers = &rows[0];

        // 確保列名唯一性
        let mut used_names = std::collections::HashMap::new();
        let columns_def: Vec<String> = headers
            .iter()
            .map(|header| {
                let sanitized_header = Self::sanitize_column_name(header);
                let unique_header = {
                    let count = used_names.entry(sanitized_header.clone()).or_insert(0);
                    *count += 1;
                    if *count == 1 {
                        sanitized_header
                    } else {
                        format!("{}_{}", sanitized_header, *count - 1)
                    }
                };
                format!("\"{}\" TEXT", unique_header)
            })
            .collect();

        let columns_str = columns_def.join(", ");
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            table_name, columns_str
        );

        db_clone
            .call(move |conn| {
                conn.execute(&create_sql, params![])?;
                Ok(())
            })
            .await?;

        // 準備插入語句
        let insert_sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
            table_name,
            {
                // 確保列名唯一性
                let mut used_names_insert = std::collections::HashMap::new();
                headers
                    .iter()
                    .map(|h| {
                        let sanitized_header = Self::sanitize_column_name(h);
                        let unique_header = {
                            let count = used_names_insert
                                .entry(sanitized_header.clone())
                                .or_insert(0);
                            *count += 1;
                            if *count == 1 {
                                sanitized_header
                            } else {
                                format!("{}_{}", sanitized_header, *count - 1)
                            }
                        };
                        format!("\"{}\"", unique_header)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            (0..headers.len())
                .map(|_| "?".to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // 插入數據 (跳過標題行)
        let insert_sql_clone = insert_sql.clone();
        for row in rows.iter().skip(1) {
            let values: Vec<String> = row.iter().map(|s| s.to_string()).collect();
            let padding_count = headers.len().saturating_sub(values.len());
            let mut values_owned = values;
            for _ in 0..padding_count {
                values_owned.push("".to_string());
            }

            let insert_sql_double_clone = insert_sql_clone.clone();
            db_clone
                .call(move |conn| {
                    // 使用 rusqlite::params! 宏處理動態參數
                    match values_owned.len() {
                        1 => conn.execute(&insert_sql_double_clone, params![&values_owned[0]])?,
                        2 => conn.execute(
                            &insert_sql_double_clone,
                            params![&values_owned[0], &values_owned[1]],
                        )?,
                        3 => conn.execute(
                            &insert_sql_double_clone,
                            params![&values_owned[0], &values_owned[1], &values_owned[2]],
                        )?,
                        4 => conn.execute(
                            &insert_sql_double_clone,
                            params![
                                &values_owned[0],
                                &values_owned[1],
                                &values_owned[2],
                                &values_owned[3]
                            ],
                        )?,
                        5 => conn.execute(
                            &insert_sql_double_clone,
                            params![
                                &values_owned[0],
                                &values_owned[1],
                                &values_owned[2],
                                &values_owned[3],
                                &values_owned[4]
                            ],
                        )?,
                        6 => conn.execute(
                            &insert_sql_double_clone,
                            params![
                                &values_owned[0],
                                &values_owned[1],
                                &values_owned[2],
                                &values_owned[3],
                                &values_owned[4],
                                &values_owned[5]
                            ],
                        )?,
                        7 => conn.execute(
                            &insert_sql_double_clone,
                            params![
                                &values_owned[0],
                                &values_owned[1],
                                &values_owned[2],
                                &values_owned[3],
                                &values_owned[4],
                                &values_owned[5],
                                &values_owned[6]
                            ],
                        )?,
                        8 => conn.execute(
                            &insert_sql_double_clone,
                            params![
                                &values_owned[0],
                                &values_owned[1],
                                &values_owned[2],
                                &values_owned[3],
                                &values_owned[4],
                                &values_owned[5],
                                &values_owned[6],
                                &values_owned[7]
                            ],
                        )?,
                        n if n <= 16 => {
                            // For longer parameter lists up to 16, use a generic approach
                            match n {
                                9 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8]
                                    ],
                                )?,
                                10 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9]
                                    ],
                                )?,
                                11 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9],
                                        &values_owned[10]
                                    ],
                                )?,
                                12 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9],
                                        &values_owned[10],
                                        &values_owned[11]
                                    ],
                                )?,
                                13 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9],
                                        &values_owned[10],
                                        &values_owned[11],
                                        &values_owned[12]
                                    ],
                                )?,
                                14 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9],
                                        &values_owned[10],
                                        &values_owned[11],
                                        &values_owned[12],
                                        &values_owned[13]
                                    ],
                                )?,
                                15 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9],
                                        &values_owned[10],
                                        &values_owned[11],
                                        &values_owned[12],
                                        &values_owned[13],
                                        &values_owned[14]
                                    ],
                                )?,
                                16 => conn.execute(
                                    &insert_sql_double_clone,
                                    params![
                                        &values_owned[0],
                                        &values_owned[1],
                                        &values_owned[2],
                                        &values_owned[3],
                                        &values_owned[4],
                                        &values_owned[5],
                                        &values_owned[6],
                                        &values_owned[7],
                                        &values_owned[8],
                                        &values_owned[9],
                                        &values_owned[10],
                                        &values_owned[11],
                                        &values_owned[12],
                                        &values_owned[13],
                                        &values_owned[14],
                                        &values_owned[15]
                                    ],
                                )?,
                                _ => conn
                                    .execute(&insert_sql_double_clone, params![&values_owned[0]])?, // fallback
                            }
                        }
                        _ => conn.execute(&insert_sql_double_clone, params![&values_owned[0]])?, // fallback for more than 16
                    };
                    Ok(())
                })
                .await?;
        }

        Ok(())
    }

    fn sanitize_column_name(name: &str) -> String {
        let re = Regex::new(r"[^a-zA-Z0-9_]").unwrap();
        let sanitized = re.replace_all(name, "_");
        if sanitized.is_empty() || sanitized.chars().next().is_none_or(|c| c.is_ascii_digit()) {
            format!("_{}", sanitized)
        } else {
            sanitized.to_string()
        }
    }

    fn sanitize_table_name(name: &str) -> String {
        // 只替換真正會造成 SQL 問題的字符，保留中文等有效字符
        let re = Regex::new(r"[^a-zA-Z0-9_\u{4e00}-\u{9fff}\u{3400}-\u{4dbf}\u{20000}-\u{2a6df}\u{2a700}-\u{2b73f}\u{2b740}-\u{2b81f}\u{2b820}-\u{2ceaf}\u{f900}-\u{faff}\u{2f800}-\u{2fa1f}]").unwrap();
        let sanitized = re.replace_all(name, "_");

        // 確保不以數字開頭
        if sanitized.chars().next().is_none_or(|c| c.is_ascii_digit()) {
            format!("_{}", sanitized)
        } else {
            sanitized.to_string()
        }
    }
}
