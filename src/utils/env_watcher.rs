use notify::{EventKind, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Arc;

pub struct EnvWatcher {
    _watcher: Arc<std::sync::Mutex<Option<notify::RecommendedWatcher>>>,
}

impl EnvWatcher {
    pub fn new(env_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let env_path = env_path.to_string();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx)?;
        watcher.watch(Path::new(&env_path), RecursiveMode::NonRecursive)?;

        std::thread::spawn(move || {
            for res in rx {
                match res {
                    Ok(event) => {
                        if matches!(event.kind, EventKind::Modify(_)) {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            log::info!("檢測到 .env 檔案變更，重新載入環境變數");

                            if let Err(e) = dotenvy::from_path(&env_path) {
                                log::error!("重新載入 .env 檔案失敗: {}", e);
                            } else {
                                log::info!(".env 檔案已重新載入");
                            }
                        }
                    }
                    Err(e) => log::error!(".env 監視器錯誤: {:?}", e),
                }
            }
        });

        Ok(Self {
            _watcher: Arc::new(std::sync::Mutex::new(Some(watcher))),
        })
    }
}
