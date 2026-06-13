//! API 設定型別與供應商列舉。維持與既有 config.json 的反序列化相容（新增 Grok）。
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_name")]
    pub name: String,
    pub api_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub enabled: bool,
    pub provider: ApiProvider,
}

fn default_api_name() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApiProvider {
    OpenAI,
    OpenRouter,
    Grok,
    Anthropic,
    Google,
    Custom,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            api_url: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: None,
            model: "gpt-3.5-turbo".to_string(),
            enabled: false,
            provider: ApiProvider::OpenAI,
        }
    }
}

pub fn get_api_key_from_env(provider: &ApiProvider) -> Option<String> {
    match provider {
        ApiProvider::OpenAI => env::var("OPENAI_API_KEY").ok(),
        ApiProvider::OpenRouter => env::var("OPENROUTER_API_KEY").ok(),
        ApiProvider::Grok => env::var("XAI_API_KEY").ok(),
        ApiProvider::Anthropic => env::var("ANTHROPIC_API_KEY").ok(),
        ApiProvider::Google => env::var("GOOGLE_API_KEY").ok(),
        ApiProvider::Custom => env::var("CUSTOM_API_KEY").ok(),
    }
}

pub fn env_var_name(provider: &ApiProvider) -> &'static str {
    match provider {
        ApiProvider::OpenAI => "OPENAI_API_KEY",
        ApiProvider::OpenRouter => "OPENROUTER_API_KEY",
        ApiProvider::Grok => "XAI_API_KEY",
        ApiProvider::Anthropic => "ANTHROPIC_API_KEY",
        ApiProvider::Google => "GOOGLE_API_KEY",
        ApiProvider::Custom => "CUSTOM_API_KEY",
    }
}

pub fn get_default_model_for_provider(provider: &ApiProvider) -> String {
    match provider {
        ApiProvider::OpenRouter => "google/gemma-2-9b-it".to_string(),
        ApiProvider::OpenAI => "gpt-4o-mini".to_string(),
        ApiProvider::Grok => "grok-2-latest".to_string(),
        ApiProvider::Anthropic => "claude-3-5-haiku-latest".to_string(),
        ApiProvider::Google => "gemini-1.5-flash".to_string(),
        ApiProvider::Custom => "gpt-3.5-turbo".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_openai() {
        let c = ApiConfig::default();
        assert_eq!(c.provider, ApiProvider::OpenAI);
        assert_eq!(c.model, "gpt-3.5-turbo");
    }

    #[test]
    fn legacy_provider_strings_deserialize() {
        // 既有 config.json 內既有變體必須仍可反序列化
        for s in ["OpenAI", "OpenRouter", "Anthropic", "Google", "Custom"] {
            let json = format!("\"{}\"", s);
            let p: ApiProvider = serde_json::from_str(&json).unwrap();
            let _ = env_var_name(&p);
        }
    }

    #[test]
    fn grok_uses_xai_key_name() {
        assert_eq!(env_var_name(&ApiProvider::Grok), "XAI_API_KEY");
    }
}
