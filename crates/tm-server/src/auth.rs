use axum::http::{HeaderMap, header};

use crate::{Result, ServerError};

#[derive(Debug, Clone, Default)]
pub enum AuthConfig {
    #[default]
    NoAuth,
    BearerToken(String),
    Forwarded(ForwardedAuthConfig),
}

#[derive(Debug, Clone)]
pub struct ForwardedAuthConfig {
    pub user_header: String,
    pub expected_user: Option<String>,
}

impl AuthConfig {
    pub fn authorize(&self, headers: &HeaderMap) -> Result<()> {
        match self {
            AuthConfig::NoAuth => Ok(()),
            AuthConfig::BearerToken(token) => {
                let Some(value) = headers.get(header::AUTHORIZATION) else {
                    return Err(ServerError::Unauthorized);
                };
                let value = value.to_str().map_err(|_| ServerError::Unauthorized)?;
                if value == format!("Bearer {token}") {
                    Ok(())
                } else {
                    Err(ServerError::Forbidden)
                }
            }
            AuthConfig::Forwarded(cfg) => {
                let Some(value) = headers.get(&cfg.user_header) else {
                    return Err(ServerError::Unauthorized);
                };
                let value = value.to_str().map_err(|_| ServerError::Unauthorized)?;
                match &cfg.expected_user {
                    Some(expected) if value == expected => Ok(()),
                    Some(_) => Err(ServerError::Forbidden),
                    None => Ok(()),
                }
            }
        }
    }
}
