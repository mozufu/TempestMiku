use std::{net::SocketAddr, path::PathBuf};

use tm_artifacts::default_root;
use tm_host::P0HostConfig;
use tm_modes::ModesConfig;
use tm_server::ServerRole;

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
    persona.with_managed_mode_addenda_path(managed_mode_addenda_path)
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
            self_evolution: Default::default(),
        }),
    }
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
