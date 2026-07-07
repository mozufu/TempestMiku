use std::{env, time::Duration};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct E2eConfig {
    pub base_url: String,
    pub bearer_token: Option<String>,
    pub timeout: Duration,
}

impl E2eConfig {
    pub fn from_env() -> Self {
        let timeout = env::var("TM_MIKU_E2E_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT);
        Self {
            base_url: env::var("TM_MIKU_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
            bearer_token: env::var("TM_MIKU_BEARER_TOKEN")
                .or_else(|_| env::var("TM_MIKU_TOKEN"))
                .ok()
                .filter(|token| !token.trim().is_empty()),
            timeout,
        }
    }
}
