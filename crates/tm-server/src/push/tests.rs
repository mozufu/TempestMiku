use std::sync::Arc;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::Result;

use super::*;
use axum::{
    Router,
    body::Bytes,
    http::{HeaderMap, StatusCode},
    routing::post,
};

#[derive(Debug)]
struct ScriptedPushStore {
    lease: Mutex<Option<PushDeliveryLease>>,
    completed: Mutex<usize>,
    retried: Mutex<usize>,
    failed: Mutex<usize>,
    disabled: Mutex<usize>,
}

impl ScriptedPushStore {
    fn new(lease: PushDeliveryLease) -> Self {
        Self {
            lease: Mutex::new(Some(lease)),
            completed: Mutex::new(0),
            retried: Mutex::new(0),
            failed: Mutex::new(0),
            disabled: Mutex::new(0),
        }
    }
}

#[async_trait]
impl PushStore for ScriptedPushStore {
    async fn upsert_registration(
        &self,
        _device_id: Uuid,
        _provider: &str,
        _encrypted: EncryptedSecret,
        _now: DateTime<Utc>,
    ) -> Result<PushRegistrationMetadata> {
        panic!("unused")
    }

    async fn disable_registration(
        &self,
        _device_id: Uuid,
        _error: Option<&str>,
        _now: DateTime<Utc>,
    ) -> Result<Option<PushRegistrationMetadata>> {
        panic!("unused")
    }

    async fn materialize_deliveries(&self, _now: DateTime<Utc>, _limit: i64) -> Result<usize> {
        Ok(0)
    }

    async fn claim_next_delivery(
        &self,
        _owner_id: Uuid,
        _now: DateTime<Utc>,
        _lease_timeout: chrono::Duration,
    ) -> Result<Option<PushDeliveryLease>> {
        Ok(self.lease.lock().take())
    }

    async fn complete_delivery(
        &self,
        _lease: &PushDeliveryLease,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        *self.completed.lock() += 1;
        Ok(())
    }

    async fn retry_delivery(
        &self,
        _lease: &PushDeliveryLease,
        _error: &str,
        _available_at: DateTime<Utc>,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        *self.retried.lock() += 1;
        Ok(())
    }

    async fn fail_delivery(
        &self,
        _lease: &PushDeliveryLease,
        _error: &str,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        *self.failed.lock() += 1;
        Ok(())
    }

    async fn fail_delivery_and_disable_registration(
        &self,
        _lease: &PushDeliveryLease,
        _error: &str,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        *self.disabled.lock() += 1;
        Ok(())
    }

    async fn runtime_metrics(&self, _now: DateTime<Utc>) -> Result<PushRuntimeMetrics> {
        Ok(PushRuntimeMetrics::default())
    }
}

fn scripted_lease(cipher: &PushCipher, attempts: i32) -> PushDeliveryLease {
    let device_id = Uuid::new_v4();
    PushDeliveryLease {
        message: PushMessage {
            version: 1,
            delivery_id: Uuid::new_v4(),
            kind: PushMessageKind::ApprovalRequested,
            session_id: Uuid::new_v4(),
            approval_id: Some(Uuid::new_v4()),
            event_seq: None,
            expires_at: Utc::now() + chrono::Duration::minutes(5),
        },
        device_id,
        provider: "fake".to_string(),
        encrypted_secret: cipher
            .encrypt(device_id, "fake", "opaque-registration")
            .unwrap(),
        attempts,
        lease_owner: Uuid::new_v4(),
        lease_epoch: 1,
    }
}

#[test]
fn registration_secrets_are_bound_to_device_and_provider() {
    let cipher = PushCipher::generate_for_tests();
    let device_id = Uuid::new_v4();
    let encrypted = cipher.encrypt(device_id, "fake", "opaque-token").unwrap();
    let debug = format!("{encrypted:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&hex::encode(&encrypted.ciphertext)));
    assert_eq!(
        cipher.decrypt(device_id, "fake", &encrypted).unwrap(),
        "opaque-token"
    );
    assert!(cipher.decrypt(Uuid::new_v4(), "fake", &encrypted).is_err());
    assert!(cipher.decrypt(device_id, "other", &encrypted).is_err());
}

#[test]
fn encryption_key_parser_fails_closed() {
    assert!(PushCipher::from_base64("not base64").is_err());
    assert!(PushCipher::from_base64(&STANDARD.encode([7_u8; 31])).is_err());
    assert!(PushCipher::from_base64(&STANDARD.encode([7_u8; 32])).is_ok());
}

#[test]
fn provider_payload_contains_only_routing_identifiers() {
    let message = PushMessage {
        version: 1,
        delivery_id: Uuid::new_v4(),
        kind: PushMessageKind::ApprovalRequested,
        session_id: Uuid::new_v4(),
        approval_id: Some(Uuid::new_v4()),
        event_seq: None,
        expires_at: Utc::now(),
    };
    let value = serde_json::to_value(message).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 6);
    for forbidden in ["action", "scope", "token", "transcript", "credential"] {
        assert!(!value.to_string().contains(forbidden));
    }

    let session_message = PushMessage {
        version: 1,
        delivery_id: Uuid::new_v4(),
        kind: PushMessageKind::SessionReady,
        session_id: Uuid::new_v4(),
        approval_id: None,
        event_seq: Some(42),
        expires_at: Utc::now(),
    };
    let value = serde_json::to_value(session_message).unwrap();
    assert_eq!(value["kind"], "session_ready");
    assert_eq!(value["eventSeq"], 42);
    assert!(value.get("approvalId").is_none());
}

#[test]
fn unified_push_origin_and_registration_fail_closed() {
    assert!(UnifiedPushProvider::new("http://push.example.test").is_err());
    assert!(UnifiedPushProvider::new("https://user@push.example.test").is_err());
    assert!(UnifiedPushProvider::new("https://push.example.test/path").is_err());

    let provider = UnifiedPushProvider::new("https://push.example.test").unwrap();
    let valid_keys = serde_json::json!({
        "endpoint": "https://push.example.test/up-secret",
        "p256dh": "BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8",
        "auth": "xS03Fi5ErfTNH_l9WHE9Ig"
    });
    assert!(provider.parse_registration(&valid_keys.to_string()).is_ok());

    let wrong_origin = serde_json::json!({
        "endpoint": "https://internal.example.test/up-secret",
        "p256dh": valid_keys["p256dh"],
        "auth": valid_keys["auth"]
    });
    assert!(
        provider
            .parse_registration(&wrong_origin.to_string())
            .is_err()
    );
}

#[tokio::test]
async fn unified_push_posts_encrypted_routing_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let sent_tx = Arc::new(Mutex::new(Some(sent_tx)));
    let app = Router::new().route(
        "/up-secret",
        post({
            let sent_tx = Arc::clone(&sent_tx);
            move |headers: HeaderMap, body: Bytes| async move {
                if let Some(sender) = sent_tx.lock().take() {
                    let _ = sender.send((headers, body));
                }
                StatusCode::OK
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let provider = UnifiedPushProvider::build(&origin, true).unwrap();
    let message = PushMessage {
        version: 1,
        delivery_id: Uuid::new_v4(),
        kind: PushMessageKind::ApprovalRequested,
        session_id: Uuid::new_v4(),
        approval_id: Some(Uuid::new_v4()),
        event_seq: None,
        expires_at: Utc::now() + chrono::Duration::minutes(5),
    };
    let registration = serde_json::json!({
        "endpoint": format!("{origin}/up-secret"),
        "p256dh": "BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8",
        "auth": "xS03Fi5ErfTNH_l9WHE9Ig"
    });

    let result = provider.deliver(&registration.to_string(), &message).await;
    assert_eq!(result.outcome, PushProviderOutcome::Delivered);
    let (headers, body) = sent_rx.await.unwrap();
    assert_eq!(headers[reqwest::header::CONTENT_ENCODING], "aes128gcm");
    assert_eq!(
        headers[reqwest::header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(headers["urgency"], "high");
    assert!(!body.is_empty());
    let approval_id = message.approval_id.expect("approval route id").to_string();
    assert!(
        !body
            .windows(approval_id.len())
            .any(|part| part == approval_id.as_bytes())
    );
    server.abort();
}

#[tokio::test]
async fn unified_push_transport_errors_do_not_expose_endpoint_capabilities() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let provider = UnifiedPushProvider::build(&origin, true).unwrap();
    let message = PushMessage {
        version: 1,
        delivery_id: Uuid::new_v4(),
        kind: PushMessageKind::ApprovalRequested,
        session_id: Uuid::new_v4(),
        approval_id: Some(Uuid::new_v4()),
        event_seq: None,
        expires_at: Utc::now() + chrono::Duration::minutes(5),
    };
    let registration = serde_json::json!({
        "endpoint": format!("{origin}/up-secret-capability"),
        "p256dh": "BLMbF9ffKBiWQLCKvTHb6LO8Nb6dcUh6TItC455vu2kElga6PQvUmaFyCdykxY2nOSSL3yKgfbmFLRTUaGv4yV8",
        "auth": "xS03Fi5ErfTNH_l9WHE9Ig"
    });

    let result = provider.deliver(&registration.to_string(), &message).await;
    assert_eq!(result.outcome, PushProviderOutcome::TransientFailure);
    assert_eq!(
        result.error.as_deref(),
        Some("UnifiedPush delivery transport failed")
    );
    assert!(!result.error.unwrap().contains("up-secret-capability"));
}

#[tokio::test]
async fn fake_provider_delivery_uses_decrypted_registration_and_completes() {
    let cipher = PushCipher::generate_for_tests();
    let store = Arc::new(ScriptedPushStore::new(scripted_lease(&cipher, 1)));
    let provider = Arc::new(FakePushProvider::default());
    let service = PushService::new(store.clone(), provider.clone(), cipher);

    assert_eq!(service.tick(Uuid::new_v4()).await.unwrap(), 1);
    assert_eq!(*store.completed.lock(), 1);
    assert_eq!(provider.deliveries()[0].0, "opaque-registration");
    assert!(!format!("{provider:?}").contains("opaque-registration"));
}

#[tokio::test]
async fn transient_failure_retries_but_permanent_failure_disables_registration() {
    let cipher = PushCipher::generate_for_tests();
    let transient_store = Arc::new(ScriptedPushStore::new(scripted_lease(&cipher, 1)));
    let transient_provider = Arc::new(FakePushProvider::default());
    transient_provider.queue_outcome(PushProviderResult {
        outcome: PushProviderOutcome::TransientFailure,
        error: Some("temporary outage".to_string()),
    });
    PushService::new(transient_store.clone(), transient_provider, cipher.clone())
        .tick(Uuid::new_v4())
        .await
        .unwrap();
    assert_eq!(*transient_store.retried.lock(), 1);
    assert_eq!(*transient_store.disabled.lock(), 0);

    let permanent_store = Arc::new(ScriptedPushStore::new(scripted_lease(&cipher, 1)));
    let permanent_provider = Arc::new(FakePushProvider::default());
    permanent_provider.queue_outcome(PushProviderResult {
        outcome: PushProviderOutcome::PermanentRegistrationFailure,
        error: Some("invalid registration".to_string()),
    });
    PushService::new(permanent_store.clone(), permanent_provider, cipher)
        .tick(Uuid::new_v4())
        .await
        .unwrap();
    assert_eq!(*permanent_store.disabled.lock(), 1);
    assert_eq!(*permanent_store.retried.lock(), 0);
}
