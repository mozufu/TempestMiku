use super::*;

#[tokio::test]
async fn gated_postgres_runs_versioned_auth_migrations_and_redeems_once() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    for table in [
        "schema_migrations",
        "auth_devices",
        "pairing_codes",
        "evolution_audits",
        "evolution_review_proposals",
        "push_deliveries",
    ] {
        let name = format!("public.{table}");
        let exists: bool = store
            .client()
            .query_one("select to_regclass($1) is not null", &[&name])
            .await
            .unwrap()
            .get(0);
        assert!(exists, "{table} should exist");
    }
    let migrations = store
        .client()
        .query(
            "select version, name, checksum from schema_migrations order by version",
            &[],
        )
        .await
        .unwrap();
    assert!(migrations.len() >= 2);
    assert_eq!(migrations[0].get::<_, i64>("version"), 1);
    assert_eq!(migrations[1].get::<_, i64>("version"), 2);
    assert_eq!(migrations[0].get::<_, String>("checksum").len(), 64);
    assert_eq!(migrations[1].get::<_, String>("checksum").len(), 64);

    let owner_subject: String = store
        .client()
        .query_one(
            "select owner_subject from server_authority where singleton = true",
            &[],
        )
        .await
        .unwrap()
        .get("owner_subject");

    let now = Utc::now();
    let raw_code = format!("postgres-pair-{}", Uuid::new_v4());
    store
        .create_pairing_code(crate::NewPairingCode {
            id: Uuid::new_v4(),
            code_hash: crate::auth::hash_secret(&raw_code),
            created_at: now,
            expires_at: now + Duration::minutes(5),
            created_by_device_id: None,
        })
        .await
        .unwrap();
    let raw_token = format!("postgres-device-{}", Uuid::new_v4());
    let device = store
        .consume_pairing_code(
            &crate::auth::hash_secret(&raw_code),
            crate::NewAuthDevice {
                id: Uuid::new_v4(),
                owner_subject: owner_subject.clone(),
                name: "Postgres auth test".to_string(),
                platform: "test".to_string(),
                token_hash: crate::auth::hash_secret(&raw_token),
                created_at: now,
            },
            now,
        )
        .await
        .unwrap();
    assert!(
        store
            .authenticate_device(&crate::auth::hash_secret(&raw_token), Utc::now())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .consume_pairing_code(
                &crate::auth::hash_secret(&raw_code),
                crate::NewAuthDevice {
                    id: Uuid::new_v4(),
                    owner_subject: owner_subject.clone(),
                    name: "Replay".to_string(),
                    platform: "test".to_string(),
                    token_hash: crate::auth::hash_secret("replay-token"),
                    created_at: now,
                },
                now,
            )
            .await
            .is_err()
    );
    store
        .revoke_auth_device(device.id, Utc::now())
        .await
        .unwrap();
    assert!(
        store
            .authenticate_device(&crate::auth::hash_secret(&raw_token), Utc::now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn gated_postgres_push_outbox_delivers_request_and_resolution_once() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let store = PostgresStore::connect(&dsn).await.unwrap();
    store.configure_owner_subject("brian").await.unwrap();
    let now = Utc::now();
    let raw_code = format!("push-pair-{}", Uuid::new_v4());
    store
        .create_pairing_code(crate::NewPairingCode {
            id: Uuid::new_v4(),
            code_hash: crate::auth::hash_secret(&raw_code),
            created_at: now,
            expires_at: now + Duration::minutes(5),
            created_by_device_id: None,
        })
        .await
        .unwrap();
    let device = store
        .consume_pairing_code(
            &crate::auth::hash_secret(&raw_code),
            crate::NewAuthDevice {
                id: Uuid::new_v4(),
                owner_subject: "brian".to_string(),
                name: "Push outbox test".to_string(),
                platform: "android".to_string(),
                token_hash: crate::auth::hash_secret(&format!("push-device-{}", Uuid::new_v4())),
                created_at: now,
            },
            now,
        )
        .await
        .unwrap();
    let session = store
        .create_session(NewSession {
            mode: ModeId::from("general"),
            persona_status: AssetStatus::Degraded {
                warning: "test".to_string(),
            },
        })
        .await
        .unwrap();
    let approval_id = Uuid::new_v4();
    store
        .create_approval_request(NewApprovalRequest {
            id: approval_id,
            session_id: session.id,
            turn_id: None,
            requester_id: Uuid::new_v4(),
            origin: "native-tm".to_string(),
            action: "proc.run cargo test".to_string(),
            scope_json: json!({"capability": "proc.run"}),
            options_json: json!([
                {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]),
            effect_type: "approval_continuation".to_string(),
            effect_payload_json: json!({}),
            resumable: true,
            created_at: now,
            expires_at: now + Duration::minutes(5),
        })
        .await
        .unwrap();
    let request_event = store
        .append_event(session.id, "approval", json!({"approvalId": approval_id}))
        .await
        .unwrap();
    store
        .link_approval_event(session.id, approval_id, "approval", request_event.seq)
        .await
        .unwrap();

    let provider = Arc::new(FakePushProvider::default());
    let cipher = PushCipher::generate_for_tests();
    let service = PushService::new(
        Arc::new(PostgresPushStore::connect(&dsn).await.unwrap()),
        provider.clone(),
        cipher.clone(),
    );
    let other_service = PushService::new(
        Arc::new(PostgresPushStore::connect(&dsn).await.unwrap()),
        provider.clone(),
        cipher,
    );
    service
        .register(device.id, "fake", "opaque-registration")
        .await
        .unwrap();
    let (first_worker, second_worker) = tokio::join!(
        service.tick(Uuid::new_v4()),
        other_service.tick(Uuid::new_v4())
    );
    assert_eq!(first_worker.unwrap() + second_worker.unwrap(), 1);
    let deliveries = provider.deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].1.kind, PushMessageKind::ApprovalRequested);
    assert_eq!(deliveries[0].1.session_id, session.id);
    assert_eq!(deliveries[0].1.approval_id, Some(approval_id));

    store
        .resolve_approval_request_with_event(
            session.id,
            approval_id,
            NewApprovalResolution {
                status: "denied".to_string(),
                selected_option_id: Some("reject".to_string()),
                resolution_json: json!({
                    "approvalId": approval_id,
                    "status": "denied"
                }),
                resolved_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    let (first_worker, second_worker) = tokio::join!(
        service.tick(Uuid::new_v4()),
        other_service.tick(Uuid::new_v4())
    );
    assert_eq!(first_worker.unwrap() + second_worker.unwrap(), 1);
    let deliveries = provider.deliveries();
    assert_eq!(deliveries.len(), 2);
    assert_eq!(deliveries[1].1.kind, PushMessageKind::ApprovalResolved);

    let final_event = store
        .append_event(
            session.id,
            "final",
            json!({"text": "private assistant reply"}),
        )
        .await
        .unwrap();
    let (first_worker, second_worker) = tokio::join!(
        service.tick(Uuid::new_v4()),
        other_service.tick(Uuid::new_v4())
    );
    assert_eq!(first_worker.unwrap() + second_worker.unwrap(), 1);
    let deliveries = provider.deliveries();
    assert_eq!(deliveries.len(), 3);
    assert_eq!(deliveries[2].1.kind, PushMessageKind::SessionReady);
    assert_eq!(deliveries[2].1.session_id, session.id);
    assert_eq!(deliveries[2].1.event_seq, Some(final_event.seq));
    assert_eq!(deliveries[2].1.approval_id, None);
    assert!(
        !serde_json::to_string(&deliveries[2].1)
            .unwrap()
            .contains("private assistant reply")
    );
    assert_eq!(service.runtime_metrics().await.unwrap().queue_depth, 0);
}
#[tokio::test]
async fn gated_postgres_upgrades_legacy_base_schema_without_losing_history() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_legacy_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();

    let mut legacy_config = dsn.parse::<tokio_postgres::Config>().unwrap();
    legacy_config.options(format!("-c search_path={schema}"));
    let (legacy_client, legacy_connection) =
        legacy_config.connect(tokio_postgres::NoTls).await.unwrap();
    let legacy_connection_task = tokio::spawn(async move {
        legacy_connection.await.unwrap();
    });
    legacy_client
        .batch_execute(include_str!("../../../migrations/0001_base.sql"))
        .await
        .unwrap();
    let session_id = Uuid::new_v4();
    let created_at = Utc::now() - Duration::days(30);
    let mode = serde_json::to_value(ModeId::from("general")).unwrap();
    let persona_status = serde_json::to_value(AssetStatus::Degraded {
        warning: "legacy deployment".to_string(),
    })
    .unwrap();
    legacy_client
        .execute(
            "insert into sessions
                (id, created_at, updated_at, status, mode, mode_state_json, persona_status)
             values ($1, $2, $2, 'open', $3, null, $4)",
            &[&session_id, &created_at, &mode, &persona_status],
        )
        .await
        .unwrap();
    legacy_client
        .execute(
            "insert into messages(session_id, seq, role, content, created_at)
             values ($1, 1, 'user', 'preserve this legacy message', $2)",
            &[&session_id, &created_at],
        )
        .await
        .unwrap();
    drop(legacy_client);
    legacy_connection_task.await.unwrap();

    let upgraded = PostgresStore::connect_in_schema(&dsn, &schema)
        .await
        .unwrap();
    let session = upgraded.get_session(session_id).await.unwrap();
    assert_eq!(session.status, "open");
    assert_eq!(session.owner_subject, "owner");
    assert_eq!(session.memory_scope, "global");
    let messages = upgraded.session_messages(session_id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "preserve this legacy message");
    let migrations = upgraded
        .client()
        .query(
            "select version, checksum from schema_migrations order by version",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(migrations.len(), 21);
    assert!(migrations.iter().enumerate().all(|(index, row)| {
        row.get::<_, i64>("version") == index as i64 + 1
            && row.get::<_, String>("checksum").len() == 64
    }));

    drop(upgraded);
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
