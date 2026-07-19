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
        let expected_version = std::env::var("TM_OMP_ACP_EXPECTED_VERSION").map_err(|_| {
            ServerError::Backend(
                "TM_OMP_ACP_EXPECTED_VERSION is required when the OMP ACP backend is enabled"
                    .to_string(),
            )
        })?;
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
        let config = Self {
            command,
            expected_version,
            cwd,
            approval_mode,
            profile,
            artifact_root,
            approval_timeout,
        };
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.expected_version.trim().is_empty() {
            return Err(ServerError::Backend(
                "OMP ACP expected version cannot be empty".to_string(),
            ));
        }
        if !matches!(self.approval_mode.as_str(), "always-ask" | "write" | "yolo") {
            return Err(ServerError::Backend(format!(
                "unsupported OMP ACP approval mode {}",
                self.approval_mode
            )));
        }
        if let Some(profile) = self.profile.as_deref()
            && !valid_profile(profile)
        {
            return Err(ServerError::Backend(format!(
                "invalid OMP ACP profile {profile:?}"
            )));
        }
        if !self.cwd.is_dir() {
            return Err(ServerError::Backend(format!(
                "OMP ACP cwd {} is not a directory",
                self.cwd.display()
            )));
        }
        if self.approval_timeout.is_zero() {
            return Err(ServerError::Backend(
                "OMP ACP approval timeout must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

fn valid_profile(profile: &str) -> bool {
    if profile == "default" {
        return true;
    }
    let bytes = profile.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 64
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
        || profile.ends_with('.')
        || !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return false;
    }
    let device_name = profile.split('.').next().unwrap_or(profile);
    !matches!(device_name, "con" | "prn" | "aux" | "nul")
        && !device_name
            .strip_prefix("com")
            .is_some_and(|suffix| suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit())
        && !device_name
            .strip_prefix("lpt")
            .is_some_and(|suffix| suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::valid_profile;

    #[test]
    fn omp_profiles_match_the_upstream_path_contract() {
        for profile in ["default", "work", "work-2", "team.alpha", "a_b"] {
            assert!(valid_profile(profile), "{profile}");
        }
        for profile in [
            "",
            ".",
            "..",
            "Work",
            "bad/profile",
            "trailing.",
            "con",
            "com1",
            "lpt9.log",
        ] {
            assert!(!valid_profile(profile), "{profile}");
        }
        assert!(!valid_profile(&"a".repeat(65)));
    }
}
