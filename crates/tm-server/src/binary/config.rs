use std::{fs, net::SocketAddr, path::PathBuf};

use tm_artifacts::default_root;
use tm_host::P0HostConfig;
use tm_mcp::{McpBounds, McpRuntimeConfig};
use tm_modes::ModesConfig;
use tm_server::{SelfHostedAsrConfig, ServerRole};

use super::BoxError;

pub(super) fn server_addr_from_env() -> Result<SocketAddr, BoxError> {
    Ok(std::env::var("TM_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
        .parse()?)
}

pub(super) fn database_dsn_from_env() -> Option<String> {
    std::env::var("TM_DATABASE_URL")
        .ok()
        .filter(|dsn| !dsn.trim().is_empty())
}

pub(super) fn self_hosted_asr_config_from_env() -> Result<Option<SelfHostedAsrConfig>, BoxError> {
    self_hosted_asr_config_from_values(
        optional_utf8_env("TM_SELF_HOSTED_ASR_ENDPOINT")?,
        optional_utf8_env("TM_SELF_HOSTED_ASR_LABEL")?,
        optional_utf8_env("TM_SELF_HOSTED_ASR_MODEL_ID")?,
    )
}

fn optional_utf8_env(name: &str) -> Result<Option<String>, BoxError> {
    std::env::var_os(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| format!("{name} must be valid UTF-8").into())
        })
        .transpose()
}

fn self_hosted_asr_config_from_values(
    endpoint: Option<String>,
    label: Option<String>,
    model_id: Option<String>,
) -> Result<Option<SelfHostedAsrConfig>, BoxError> {
    let values = [endpoint, label, model_id].map(|value| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    match values {
        [None, None, None] => Ok(None),
        [Some(endpoint), Some(label), Some(model_id)] => Ok(Some(SelfHostedAsrConfig::new(
            &endpoint, &label, &model_id,
        )?)),
        _ => Err(
            "TM_SELF_HOSTED_ASR_ENDPOINT, TM_SELF_HOSTED_ASR_LABEL, and TM_SELF_HOSTED_ASR_MODEL_ID must be set together"
                .into(),
        ),
    }
}

pub(super) fn server_role_from_env() -> Result<ServerRole, BoxError> {
    Ok(std::env::var("TM_SERVER_ROLE")
        .unwrap_or_else(|_| "api".to_string())
        .parse::<ServerRole>()?)
}

pub(super) fn owner_subject_from_env() -> Result<String, BoxError> {
    let owner_subject = std::env::var("TM_OWNER_SUBJECT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "brian".to_string());
    let owner_subject = owner_subject.trim().to_string();
    if owner_subject.len() > 128 || owner_subject.chars().any(char::is_control) {
        return Err("TM_OWNER_SUBJECT must be 1-128 non-control characters".into());
    }
    Ok(owner_subject)
}

pub(super) fn modes_config_from_env() -> ModesConfig {
    std::env::var_os("TM_MODES_PATH")
        .map(ModesConfig::from_path)
        .unwrap_or_default()
}

pub(super) fn apply_managed_persona_paths(
    mut persona: ModesConfig,
    artifact_root: &std::path::Path,
) -> ModesConfig {
    let managed_skills_path = std::env::var_os("TM_MANAGED_SKILLS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_root.join("managed-skills"));
    persona = persona.with_managed_skills_path(managed_skills_path);
    let managed_mode_addenda_path = std::env::var_os("TM_MANAGED_MODE_ADDENDA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_root.join("managed-mode-addenda"));
    persona = persona.with_managed_mode_addenda_path(managed_mode_addenda_path);
    let managed_persona_addenda_path = std::env::var_os("TM_MANAGED_PERSONA_ADDENDA_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_root.join("managed-persona-addenda"));
    persona.with_managed_persona_addenda_path(managed_persona_addenda_path)
}

pub(super) fn load_host_config() -> Result<P0HostConfig, BoxError> {
    let path = std::env::var_os("TM_HOST_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TM_CONFIG").map(PathBuf::from))
        .or_else(|| {
            let default = PathBuf::from(".tempestmiku/config.json");
            default.exists().then_some(default)
        });
    match path {
        Some(path) => Ok(P0HostConfig::from_json_file(path)?),
        None => Ok(P0HostConfig {
            linked_folders: Vec::new(),
            approvals: Default::default(),
            artifact_root: None,
            proc_run_timeout_ms: tm_host::default_proc_run_timeout_ms(),
            proc_isolation: Default::default(),
            self_evolution: Default::default(),
            egress: Default::default(),
        }),
    }
}

pub(super) fn load_mcp_config(host_config: &P0HostConfig) -> Result<McpRuntimeConfig, BoxError> {
    let path = std::env::var_os("TM_MCP_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(".tempestmiku/mcp.json");
            default.exists().then_some(default)
        });
    let config = match path {
        Some(path) => {
            let content = fs::read_to_string(&path)?;
            serde_json::from_str::<McpRuntimeConfig>(&content)
                .map_err(|error| format!("loading MCP config from {}: {error}", path.display()))?
        }
        None => McpRuntimeConfig::default(),
    };
    config.validate(&McpBounds::default())?;
    config.validate_egress(&host_config.egress)?;
    Ok(config)
}

pub(super) fn server_artifact_root(host_config: &P0HostConfig) -> PathBuf {
    std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT")
        .map(PathBuf::from)
        .or_else(|| host_config.artifact_root.clone())
        .unwrap_or_else(default_root)
}

pub(super) fn required_env(name: &str) -> Result<String, BoxError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required").into())
}

pub(super) fn optional_env_parse<T>(name: &str, default: T) -> Result<T, BoxError>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + 'static,
{
    match std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => Ok(value.parse::<T>()?),
        None => Ok(default),
    }
}

pub(super) fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_hosted_asr_is_disabled_by_default_and_rejects_partial_config() {
        assert!(
            self_hosted_asr_config_from_values(None, None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            self_hosted_asr_config_from_values(
                Some("https://asr.example.test/transcribe".to_string()),
                None,
                None,
            )
            .is_err()
        );
        assert!(
            self_hosted_asr_config_from_values(
                Some("https://asr.example.test/transcribe".to_string()),
                Some("家用 ASR".to_string()),
                Some("tea-asr-1.1-mini".to_string()),
            )
            .unwrap()
            .is_some()
        );
    }
}
