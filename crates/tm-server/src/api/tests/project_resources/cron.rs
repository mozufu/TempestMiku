use super::*;

#[tokio::test]
async fn cron_resource_gateway_reads_jobs_and_run_history() {
    let (app, store) = test_app(ModesConfig::default(), AuthConfig::NoAuth);
    let session = create(&app).await;
    let now = Utc::now();
    store
        .upsert_cron_job(NewCronJobRecord {
            id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
            name: "Weekly ship ledger".to_string(),
            schedule: WEEKLY_SHIP_LEDGER_SCHEDULE.to_string(),
            enabled: true,
            cron_mode: "deny".to_string(),
            max_turns: 8,
            script_timeout_seconds: 120,
            next_run_at: Some(now),
        })
        .await
        .unwrap();
    let run = store
        .record_cron_run(NewCronRunRecord {
            job_id: WEEKLY_SHIP_LEDGER_JOB_ID.to_string(),
            scheduled_for: now,
            status: "completed".to_string(),
            session_id: Some(session.id),
            result_json: json!({"sessionId": session.id}),
        })
        .await
        .unwrap();

    for (uri, expected) in [
        ("cron://".to_string(), "weekly-ship-ledger".to_string()),
        (
            format!("cron://{WEEKLY_SHIP_LEDGER_JOB_ID}"),
            "cronMode".to_string(),
        ),
        (
            format!("cron://{WEEKLY_SHIP_LEDGER_JOB_ID}/runs"),
            run.id.to_string(),
        ),
        (
            format!("cron://{WEEKLY_SHIP_LEDGER_JOB_ID}/runs/{}", run.id),
            "completed".to_string(),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sessions/{}/resources/resolve?uri={}",
                        session.id, uri
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let json = response_json(response).await;
        assert!(
            json["content"].as_str().unwrap().contains(&expected),
            "content for {uri}: {json}"
        );
    }

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/sessions/{}/resources/list?uri=cron://{}",
                    session.id, WEEKLY_SHIP_LEDGER_JOB_ID
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert!(
        response_json(listed)
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["uri"]
                == json!(format!(
                    "cron://{WEEKLY_SHIP_LEDGER_JOB_ID}/runs/{}",
                    run.id
                )))
    );
}
