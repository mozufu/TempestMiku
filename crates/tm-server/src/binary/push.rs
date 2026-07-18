use std::sync::Arc;

use tm_server::{PushCipher, PushProvider, UnifiedPushProvider};

use super::{BoxError, config::required_env};

pub(super) type ConfiguredPush = (Arc<dyn PushProvider>, PushCipher);

pub(super) fn push_config_from_env() -> Result<Option<ConfiguredPush>, BoxError> {
    let provider = std::env::var("TM_PUSH_PROVIDER").unwrap_or_else(|_| "disabled".to_string());
    match provider.trim().to_ascii_lowercase().as_str() {
        "" | "disabled" | "none" => Ok(None),
        "fake" if cfg!(debug_assertions) => {
            let key = required_env("TM_PUSH_ENCRYPTION_KEY")?;
            Ok(Some((
                Arc::new(tm_server::FakePushProvider::default()),
                PushCipher::from_base64(&key)?,
            )))
        }
        "fake" => Err("TM_PUSH_PROVIDER=fake is unavailable in release builds".into()),
        "unifiedpush" => {
            let key = required_env("TM_PUSH_ENCRYPTION_KEY")?;
            let endpoint_origin = required_env("TM_UNIFIED_PUSH_ENDPOINT_ORIGIN")?;
            Ok(Some((
                Arc::new(UnifiedPushProvider::new(&endpoint_origin)?),
                PushCipher::from_base64(&key)?,
            )))
        }
        other => Err(format!(
            "unsupported TM_PUSH_PROVIDER={other}; expected disabled or unifiedpush"
        )
        .into()),
    }
}
