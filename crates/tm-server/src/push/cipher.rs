use std::{fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use uuid::Uuid;

use crate::{Result, ServerError};

use super::EncryptedSecret;

const SECRET_VERSION: i16 = 1;

#[derive(Clone)]
pub struct PushCipher {
    key: Arc<[u8; 32]>,
}

impl fmt::Debug for PushCipher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PushCipher").finish_non_exhaustive()
    }
}

impl PushCipher {
    pub fn from_base64(value: &str) -> Result<Self> {
        let decoded = STANDARD.decode(value.trim()).map_err(|_| {
            ServerError::InvalidRequest("TM_PUSH_ENCRYPTION_KEY must be base64-encoded".to_string())
        })?;
        let key: [u8; 32] = decoded.try_into().map_err(|_| {
            ServerError::InvalidRequest(
                "TM_PUSH_ENCRYPTION_KEY must decode to exactly 32 bytes".to_string(),
            )
        })?;
        Ok(Self { key: Arc::new(key) })
    }

    pub fn generate_for_tests() -> Self {
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).expect("test push key generation succeeds");
        Self { key: Arc::new(key) }
    }

    pub(super) fn encrypt(
        &self,
        device_id: Uuid,
        provider: &str,
        secret: &str,
    ) -> Result<EncryptedSecret> {
        let mut nonce = [0_u8; 24];
        getrandom::fill(&mut nonce).map_err(|error| {
            ServerError::Store(format!("push nonce generation failed: {error}"))
        })?;
        let cipher = XChaCha20Poly1305::new(self.key.as_ref().into());
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: push_secret_aad(device_id, provider).as_bytes(),
                },
            )
            .map_err(|_| ServerError::Store("push registration encryption failed".to_string()))?;
        Ok(EncryptedSecret {
            ciphertext,
            nonce: nonce.to_vec(),
            version: SECRET_VERSION,
        })
    }

    pub(super) fn decrypt(
        &self,
        device_id: Uuid,
        provider: &str,
        encrypted: &EncryptedSecret,
    ) -> Result<String> {
        if encrypted.version != SECRET_VERSION || encrypted.nonce.len() != 24 {
            return Err(ServerError::Store(
                "unsupported push registration secret envelope".to_string(),
            ));
        }
        let cipher = XChaCha20Poly1305::new(self.key.as_ref().into());
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(&encrypted.nonce),
                Payload {
                    msg: &encrypted.ciphertext,
                    aad: push_secret_aad(device_id, provider).as_bytes(),
                },
            )
            .map_err(|_| ServerError::Store("push registration decryption failed".to_string()))?;
        String::from_utf8(plaintext)
            .map_err(|_| ServerError::Store("push registration is not UTF-8".to_string()))
    }
}

fn push_secret_aad(device_id: Uuid, provider: &str) -> String {
    format!("tempestmiku.push.v1:{device_id}:{provider}")
}
