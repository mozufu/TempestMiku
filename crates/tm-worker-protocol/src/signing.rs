use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum SignatureError {
    #[error("signing key file could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("signing key must be exactly 64 lowercase hexadecimal characters")]
    InvalidKey,
    #[error("invalid request signature")]
    InvalidSignature,
    #[error("request timestamp is outside the allowed clock skew")]
    ClockSkew,
}

#[derive(Clone)]
pub struct SigningKey([u8; 32]);

impl SigningKey {
    pub fn from_hex(value: &str) -> Result<Self, SignatureError> {
        let value = value.trim();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SignatureError::InvalidKey);
        }
        let bytes = hex::decode(value).map_err(|_| SignatureError::InvalidKey)?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| SignatureError::InvalidKey)?;
        Ok(Self(key))
    }

    pub fn from_hex_file(path: impl AsRef<Path>) -> Result<Self, SignatureError> {
        Self::from_hex(&fs::read_to_string(path)?)
    }

    pub fn sign(&self, canonical: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts 32-byte keys");
        mac.update(canonical);
        hex::encode(mac.finalize().into_bytes())
    }

    pub fn verify(&self, canonical: &[u8], signature: &str) -> Result<(), SignatureError> {
        let signature = hex::decode(signature).map_err(|_| SignatureError::InvalidSignature)?;
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts 32-byte keys");
        mac.update(canonical);
        mac.verify_slice(&signature)
            .map_err(|_| SignatureError::InvalidSignature)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAuth {
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}

impl RequestAuth {
    pub fn new(
        key: &SigningKey,
        method: &str,
        path: &str,
        body: &[u8],
        timestamp: i64,
        nonce: impl Into<String>,
    ) -> Self {
        let nonce = nonce.into();
        let canonical = canonical_request(method, path, timestamp, &nonce, body);
        Self {
            timestamp,
            nonce,
            signature: key.sign(&canonical),
        }
    }

    pub fn verify(
        &self,
        key: &SigningKey,
        method: &str,
        path: &str,
        body: &[u8],
        now: i64,
        max_skew_seconds: i64,
    ) -> Result<(), SignatureError> {
        if now.abs_diff(self.timestamp) > max_skew_seconds as u64 {
            return Err(SignatureError::ClockSkew);
        }
        key.verify(
            &canonical_request(method, path, self.timestamp, &self.nonce, body),
            &self.signature,
        )
    }
}

pub fn canonical_request(
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> Vec<u8> {
    let body_sha256 = hex::encode(Sha256::digest(body));
    format!(
        "{}\n{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        timestamp,
        nonce,
        body_sha256
    )
    .into_bytes()
}

pub fn current_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_hex(&"11".repeat(32)).unwrap()
    }

    #[test]
    fn signature_binds_method_path_timestamp_nonce_and_body() {
        let auth = RequestAuth::new(&key(), "POST", "/v1/jobs", b"{}", 100, "nonce-1");
        auth.verify(&key(), "POST", "/v1/jobs", b"{}", 100, 30)
            .unwrap();
        assert!(
            auth.verify(&key(), "POST", "/v1/jobs", b"[]", 100, 30)
                .is_err()
        );
        assert!(
            auth.verify(&key(), "GET", "/v1/jobs", b"{}", 100, 30)
                .is_err()
        );
    }

    #[test]
    fn rejects_clock_skew_and_noncanonical_keys() {
        let auth = RequestAuth::new(&key(), "GET", "/v1/health", b"", 100, "nonce-2");
        assert!(matches!(
            auth.verify(&key(), "GET", "/v1/health", b"", 131, 30),
            Err(SignatureError::ClockSkew)
        ));
        assert!(SigningKey::from_hex(&"AA".repeat(32)).is_err());
    }
}
