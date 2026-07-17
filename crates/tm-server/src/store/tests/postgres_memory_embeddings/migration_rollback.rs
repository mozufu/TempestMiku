use super::super::*;

#[tokio::test]
async fn gated_postgres_p8_durable_migration_failure_rolls_back_only_v14() {
    let Some(dsn) = postgres_test_dsn() else {
        return;
    };
    let admin = PostgresStore::connect(&dsn).await.unwrap();
    let schema = format!("tm_p8_rollback_{}", Uuid::new_v4().simple());
    admin
        .client()
        .batch_execute(&format!("create schema {schema}"))
        .await
        .unwrap();
    let mut config = dsn.parse::<tokio_postgres::Config>().unwrap();
    config.options(format!("-c search_path={schema}"));
    let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
    let connection_task = tokio::spawn(async move {
        connection.await.unwrap();
    });
    // The malformed pre-existing relation is accepted by `create table if not exists` but makes
    // the later P8.2 evidence FK invalid. The ordered migration runner must leave version 14
    // unapplied and roll back every relation it created in that migration.
    client
        .batch_execute("create table memory_records(id uuid primary key)")
        .await
        .unwrap();
    drop(client);
    connection_task.await.unwrap();

    assert!(
        PostgresStore::connect_in_schema(&dsn, &schema)
            .await
            .is_err()
    );

    let mut inspect_config = dsn.parse::<tokio_postgres::Config>().unwrap();
    inspect_config.options(format!("-c search_path={schema}"));
    let (inspect, inspect_connection) =
        inspect_config.connect(tokio_postgres::NoTls).await.unwrap();
    let inspect_connection_task = tokio::spawn(async move {
        inspect_connection.await.unwrap();
    });
    let max_version: i64 = inspect
        .query_one("select max(version)::bigint from schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(max_version, 13);
    let evidence_table_exists: bool = inspect
        .query_one(
            "select to_regclass('memory_record_evidence') is not null",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!evidence_table_exists);
    drop(inspect);
    inspect_connection_task.await.unwrap();
    admin
        .client()
        .batch_execute(&format!("drop schema {schema} cascade"))
        .await
        .unwrap();
}
