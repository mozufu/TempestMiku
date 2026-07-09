use std::{path::PathBuf, time::Duration};

use crate::{Result, ServerError};

#[derive(Debug, Clone)]
pub struct OmpAcpConfig {
    pub command: PathBuf,
    pub expected_version: String,
    pub cwd: PathBuf,
    pub approval_mode: String,
    pub profile: Option<String>,
    pub artifact_root: PathBuf,
    pub approval_timeout: Duration,
}

impl OmpAcpConfig {
    pub fn from_env() -> Result<Self> {
        let command = std::env::var_os("TM_OMP_ACP_COMMAND")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("omp"));
        let expected_version = std::env::var("TM_OMP_ACP_EXPECTED_VERSION")
            .unwrap_or_else(|_| "omp/16.2.2".to_string());
        let cwd = std::env::var_os("TM_OMP_ACP_CWD")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|err| ServerError::Backend(format!("cannot determine omp cwd: {err}")))?;
        let approval_mode =
            std::env::var("TM_OMP_ACP_APPROVAL_MODE").unwrap_or_else(|_| "always-ask".to_string());
        let profile = std::env::var("TM_OMP_ACP_PROFILE").ok();
        let artifact_root = std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(tm_artifacts::default_root);
        let approval_timeout = std::env::var("TM_OMP_ACP_APPROVAL_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(60));
        Ok(Self {
            command,
            expected_version,
            cwd,
            approval_mode,
            profile,
            artifact_root,
            approval_timeout,
        })
    }
}
