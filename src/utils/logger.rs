use log::{Level, LevelFilter, Log, Metadata, Record};
use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

const MAX_LOG_SIZE: u64 = 1024 * 1024; // 1 MiB per log file
const MAX_LOG_BACKUPS: usize = 5;

#[derive(Debug, Error)]
pub enum LoggerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to set logger: {0}")]
    SetLogger(#[from] log::SetLoggerError),
}

#[derive(Debug)]
struct LoggerState {
    file: Option<File>,
    path: Option<PathBuf>,
    last_entry: Option<String>,
    repeat_count: u32,
}

pub struct DiscordLogger {
    state: Mutex<LoggerState>,
}

impl DiscordLogger {
    pub fn new(log_file: Option<&str>) -> Result<DiscordLogger, std::io::Error> {
        let path = log_file.map(PathBuf::from);
        let file = if let Some(p) = path.as_ref() {
            Some(OpenOptions::new().create(true).append(true).open(p)?)
        } else {
            None
        };

        Ok(DiscordLogger {
            state: Mutex::new(LoggerState {
                file,
                path,
                last_entry: None,
                repeat_count: 0,
            }),
        })
    }

    pub fn init(log_file: Option<&str>) -> Result<(), LoggerError> {
        let logger = DiscordLogger::new(log_file)?;
        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Info);
        Ok(())
    }

    fn write_message(state: &mut LoggerState, message: &str) {
        println!("{}", message);
        Self::ensure_capacity(state, message.len() + 1);

        if let Some(file) = state.file.as_mut() {
            if let Err(e) = writeln!(file, "{}", message) {
                eprintln!("Failed to write log entry: {}", e);
            }
        }
    }

    fn ensure_capacity(state: &mut LoggerState, incoming_len: usize) {
        let path = match state.path.clone() {
            Some(p) => p,
            None => return,
        };

        let mut file = match state.file.take() {
            Some(f) => f,
            None => match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to open log file: {}", e);
                    return;
                }
            },
        };

        let needs_rotate = match file.metadata() {
            Ok(metadata) => metadata.len().saturating_add(incoming_len as u64) > MAX_LOG_SIZE,
            Err(e) => {
                eprintln!("Failed to inspect log file: {}", e);
                false
            }
        };

        if needs_rotate {
            let _ = file.flush();
            drop(file);

            if let Err(e) = Self::rotate_logs(&path) {
                eprintln!("Failed to rotate log file: {}", e);
            }

            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => state.file = Some(f),
                Err(e) => {
                    eprintln!("Failed to reopen log file: {}", e);
                    state.file = None;
                }
            }
        } else {
            state.file = Some(file);
        }
    }

    fn rotate_logs(path: &Path) -> std::io::Result<()> {
        if MAX_LOG_BACKUPS == 0 {
            let _ = std::fs::remove_file(path);
            return Ok(());
        }

        let oldest = Self::backup_path(path, MAX_LOG_BACKUPS);
        if oldest.exists() {
            let _ = std::fs::remove_file(&oldest);
        }

        for index in (1..MAX_LOG_BACKUPS).rev() {
            let from = Self::backup_path(path, index);
            if from.exists() {
                let to = Self::backup_path(path, index + 1);
                let _ = std::fs::rename(&from, &to);
            }
        }

        if path.exists() {
            std::fs::rename(path, Self::backup_path(path, 1))?;
        }

        Ok(())
    }

    fn backup_path(path: &Path, index: usize) -> PathBuf {
        PathBuf::from(format!("{}.{}", path.display(), index))
    }

    fn emit_repeat_summary(state: &mut LoggerState) {
        if state.repeat_count > 0 {
            let summary = format!("(previous message repeated {} times)", state.repeat_count);
            Self::write_message(state, &summary);
            state.repeat_count = 0;
        }
    }
}

impl Log for DiscordLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        const SUPPRESS_THRESHOLD: u32 = 10;
        static NOISY_PATTERNS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
            HashSet::from([
                "do_heartbeat",
                "recv_event",
                "recv;",
                "request; self=Http",
                "req=Request",
                "post_hook; self=Ratelimit",
                "handle_event",
                "handle_gateway_dispatch",
                "build; self=Request",
                "InteractionCreate",
                "heartbeat;",
                "send_heartbeat;",
                "update_manager;",
                "latency;",
                "start_connection;",
                "set_shards;",
                "shutdown_all;",
                "Telling shard queuer",
                "checked_start;",
                "check_last_start;",
                "[ShardRunner",
                "action;",
                "send_identify;",
            ])
        });

        let message = record.args().to_string();

        if NOISY_PATTERNS
            .iter()
            .any(|pattern| message.contains(pattern))
        {
            return;
        }

        let entry = format!("{}: {}", record.level(), message);
        let mut state = self.state.lock().expect("logger mutex poisoned");

        if let Some(last) = &state.last_entry {
            if last == &entry {
                state.repeat_count = state.repeat_count.saturating_add(1);

                if state.repeat_count >= SUPPRESS_THRESHOLD {
                    let summary =
                        format!("(previous message repeated {} times)", state.repeat_count);
                    Self::write_message(&mut state, &summary);
                    state.repeat_count = 0;
                }
                return;
            }
        }

        Self::emit_repeat_summary(&mut state);
        Self::write_message(&mut state, &entry);
        state.last_entry = Some(entry);
    }

    fn flush(&self) {
        if let Ok(mut state) = self.state.lock() {
            Self::emit_repeat_summary(&mut state);
            if let Some(file) = state.file.as_mut() {
                let _ = file.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logger_creation_without_file() {
        let logger = DiscordLogger::new(None);
        assert!(logger.is_ok());
    }

    #[test]
    fn test_logger_creation_with_file() {
        let logger = DiscordLogger::new(Some("test.log"));
        assert!(logger.is_ok());
        let _ = std::fs::remove_file("test.log");
        for i in 1..=MAX_LOG_BACKUPS {
            let _ = std::fs::remove_file(format!("test.log.{}", i));
        }
    }

    #[test]
    fn test_logger_suppresses_duplicates() {
        let logger = DiscordLogger::new(None).unwrap();
        let record = Record::builder()
            .level(Level::Info)
            .args(format_args!("duplicate message"))
            .build();

        logger.log(&record);
        for _ in 0..5 {
            logger.log(&record);
        }

        let state = logger.state.lock().unwrap();
        assert_eq!(state.last_entry.as_deref(), Some("INFO: duplicate message"));
        assert_eq!(state.repeat_count, 5);
    }
}
