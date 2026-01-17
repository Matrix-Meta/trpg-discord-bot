use crate::models::types::{GlobalConfig, GuildConfig};
use notify::{EventKind, RecursiveMode, Watcher, recommended_watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::watch;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Watcher error: {0}")]
    Watcher(#[from] notify::Error),
}

#[derive(Debug)]
pub struct ConfigManager {
    pub global: Arc<tokio::sync::RwLock<GlobalConfig>>,
    pub guilds: Arc<tokio::sync::RwLock<HashMap<u64, GuildConfig>>>,
    config_path: String,
    _watcher: Arc<std::sync::Mutex<Option<notify::RecommendedWatcher>>>,
    reload_task: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    reload_tx: watch::Sender<()>,
}

impl ConfigManager {
    pub async fn new(config_path: &str) -> Result<Self, ConfigError> {
        let mut manager = Self {
            global: Arc::new(RwLock::new(GlobalConfig::default())),
            guilds: Arc::new(RwLock::new(HashMap::new())),
            config_path: config_path.to_string(),
            _watcher: Arc::new(std::sync::Mutex::new(None)),
            reload_task: Arc::new(std::sync::Mutex::new(None)),
            reload_tx: watch::channel(()).0,
        };

        manager.load_config().await?;
        manager.start_watching()?;
        Ok(manager)
    }

    fn start_watching(&mut self) -> Result<(), ConfigError> {
        let config_path = self.config_path.clone();
        let global = Arc::clone(&self.global);
        let guilds = Arc::clone(&self.guilds);
        let reload_tx = self.reload_tx.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = recommended_watcher(tx)?;
        if Path::new(&config_path).exists() {
            watcher.watch(Path::new(&config_path), RecursiveMode::NonRecursive)?;
        } else if let Some(parent) = Path::new(&config_path).parent() {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            for res in rx {
                if let Ok(event) = res {
                    let _ = event_tx.send(event);
                }
            }
        });

        let task_config_path = config_path.clone();
        let reload_task = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let path_matches = event
                    .paths
                    .iter()
                    .any(|path| path == Path::new(&task_config_path));
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) && path_matches
                {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    match tokio::fs::read_to_string(&task_config_path).await {
                        Ok(content) => match serde_json::from_str::<ConfigData>(&content) {
                            Ok(config_data) => {
                                *global.write().await = config_data.global.unwrap_or_default();
                                *guilds.write().await = config_data.guilds.unwrap_or_default();
                                let _ = reload_tx.send(());
                                log::info!("配置文件已重新加載: {}", task_config_path);
                            }
                            Err(e) => {
                                log::error!("配置文件解析失敗: {:?}", e);
                            }
                        },
                        Err(e) => {
                            log::error!("讀取配置文件失敗: {:?}", e);
                        }
                    }
                }
            }
        });

        let mut watcher_guard = self._watcher.lock().unwrap();
        *watcher_guard = Some(watcher);
        let mut task_guard = self.reload_task.lock().unwrap();
        *task_guard = Some(reload_task);

        Ok(())
    }

    pub async fn load_config(&mut self) -> Result<(), ConfigError> {
        if Path::new(&self.config_path).exists() {
            let content = fs::read_to_string(&self.config_path)?;
            let mut config_data: ConfigData = serde_json::from_str(&content)?;

            // 檢查並轉換舊格式的API配置為新格式
            if let Some(ref mut guilds) = config_data.guilds {
                for (_, guild_config) in guilds.iter_mut() {
                    // 如果存在舊格式的api_config，則轉換為新格式
                    if guild_config.api_config.is_some() {
                        let old_api_config = guild_config.api_config.take().unwrap();
                        // 為舊配置設定一個預設名稱
                        let name = if old_api_config.api_url.is_empty() {
                            "default".to_string()
                        } else {
                            old_api_config.api_url.clone()
                        };

                        // 設定名稱
                        let mut new_api_config = old_api_config;
                        new_api_config.name = name.clone();

                        // 初始化api_configs映射並添加配置
                        guild_config
                            .api_configs
                            .insert(name.clone(), new_api_config);
                        // 將此配置設為活動配置
                        guild_config.active_api = Some(name);
                    }
                }
            }

            *self.global.write().await = config_data.global.unwrap_or_default();
            *self.guilds.write().await = config_data.guilds.unwrap_or_default();
        } else {
            self.save_config().await?;
        }

        Ok(())
    }

    pub async fn save_config(&self) -> Result<(), ConfigError> {
        let global_read = self.global.read().await;
        let guilds_read = self.guilds.read().await;

        let config_data = ConfigData {
            global: Some(global_read.clone()),
            guilds: Some(guilds_read.clone()),
        };

        let content = serde_json::to_string_pretty(&config_data)?;
        fs::write(&self.config_path, content)?;
        Ok(())
    }

    pub async fn get_guild_config(&self, guild_id: u64) -> GuildConfig {
        let guilds_read = self.guilds.read().await;
        guilds_read.get(&guild_id).cloned().unwrap_or_default()
    }

    pub async fn set_guild_config(
        &self,
        guild_id: u64,
        config: GuildConfig,
    ) -> Result<(), ConfigError> {
        {
            let mut guilds_write = self.guilds.write().await;
            guilds_write.insert(guild_id, config);
        } // 釋放寫鎖
        self.save_config().await
    }

    pub async fn get_guild_api_config(&self, guild_id: u64) -> crate::ai::providers::ApiConfig {
        let guilds_read = self.guilds.read().await;
        if let Some(guild_config) = guilds_read.get(&guild_id) {
            if let Some(ref active_api_name) = guild_config.active_api {
                if let Some(api_config) = guild_config.api_configs.get(active_api_name) {
                    api_config.clone()
                } else {
                    crate::ai::providers::ApiConfig::default()
                }
            } else {
                crate::ai::providers::ApiConfig::default()
            }
        } else {
            crate::ai::providers::ApiConfig::default()
        }
    }

    pub async fn add_guild_api_config(
        &self,
        guild_id: u64,
        api_config: crate::ai::providers::ApiConfig,
    ) -> Result<(), ConfigError> {
        let mut guilds_write = self.guilds.write().await;
        let guild_config = guilds_write
            .entry(guild_id)
            .or_insert_with(GuildConfig::default);
        let config_name = api_config.name.clone();
        guild_config
            .api_configs
            .insert(config_name.clone(), api_config);
        // 如果這是第一個配置，設為活動配置
        if guild_config.active_api.is_none() {
            guild_config.active_api = Some(config_name);
        }
        drop(guilds_write);
        self.save_config().await
    }

    pub async fn get_guild_api_configs(
        &self,
        guild_id: u64,
    ) -> std::collections::HashMap<String, crate::ai::providers::ApiConfig> {
        let guilds_read = self.guilds.read().await;
        if let Some(guild_config) = guilds_read.get(&guild_id) {
            guild_config.api_configs.clone()
        } else {
            std::collections::HashMap::new()
        }
    }

    pub async fn remove_guild_api_config(
        &self,
        guild_id: u64,
        name: &str,
    ) -> Result<bool, ConfigError> {
        let mut guilds_write = self.guilds.write().await;
        let mut removed = false;
        if let Some(guild_config) = guilds_write.get_mut(&guild_id) {
            if guild_config.api_configs.remove(name).is_some() {
                removed = true;
                // 如果刪除的是活動API配置，則將活動API設為空或選擇其他配置
                if let Some(ref active_name) = guild_config.active_api {
                    if active_name == name {
                        if guild_config.api_configs.is_empty() {
                            guild_config.active_api = None;
                        } else {
                            // 選擇第一個可用的API配置作為活動配置
                            if let Some(first_key) = guild_config.api_configs.keys().next() {
                                guild_config.active_api = Some(first_key.clone());
                            }
                        }
                    }
                }
            }
        }
        drop(guilds_write);
        self.save_config().await?;
        Ok(removed)
    }

    pub async fn set_active_api(&self, guild_id: u64, name: &str) -> Result<bool, ConfigError> {
        let mut guilds_write = self.guilds.write().await;
        let mut success = false;
        if let Some(guild_config) = guilds_write.get_mut(&guild_id) {
            // 檢查是否有名為name的配置
            if guild_config.api_configs.contains_key(name) {
                guild_config.active_api = Some(name.to_string());
                success = true;
            }
        }
        drop(guilds_write);
        if success {
            self.save_config().await?;
        }
        Ok(success)
    }

    pub async fn is_developer(&self, user_id: u64) -> bool {
        let global_read = self.global.read().await;
        global_read.developers.contains(&user_id)
    }

    pub async fn add_developer(&self, user_id: u64) -> Result<bool, ConfigError> {
        let mut global_write = self.global.write().await;
        if global_write.developers.contains(&user_id) {
            return Ok(false);
        }

        global_write.developers.push(user_id);
        self.save_config().await?;
        Ok(true)
    }

    pub async fn remove_developer(&self, user_id: u64) -> Result<bool, ConfigError> {
        let mut global_write = self.global.write().await;
        let original_len = global_write.developers.len();
        global_write.developers.retain(|&id| id != user_id);

        if global_write.developers.len() == original_len {
            return Ok(false);
        }

        self.save_config().await?;
        Ok(true)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ConfigData {
    global: Option<GlobalConfig>,
    guilds: Option<HashMap<u64, GuildConfig>>,
}

// 測試用異步訪問輔助函數
impl ConfigManager {
    pub async fn get_global_config(&self) -> GlobalConfig {
        let global_read = self.global.read().await;
        global_read.clone()
    }
}

#[cfg(test)]
// 測試模組
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config_manager_creation() {
        let path = "test_config.json";
        let config = ConfigManager::new(path)
            .await
            .expect("Failed to create ConfigManager in test");
        let global = config.get_global_config().await;
        assert!(!global.restart_mode.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
