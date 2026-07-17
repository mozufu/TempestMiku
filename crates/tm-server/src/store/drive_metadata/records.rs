use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tm_drive::DriveError;
use tokio_postgres::{GenericClient, error::SqlState};

pub(super) async fn query_records<C, T>(client: &C, query: &str) -> tm_drive::Result<Vec<T>>
where
    C: GenericClient + Sync,
    T: DeserializeOwned,
{
    client
        .query(query, &[])
        .await
        .map_err(store_error)?
        .into_iter()
        .map(|row| from_json(row.get("record_json")))
        .collect()
}

pub(super) async fn query_record_opt<C, T, I>(
    client: &C,
    query: &str,
    id: &I,
) -> tm_drive::Result<Option<T>>
where
    C: GenericClient + Sync,
    T: DeserializeOwned,
    I: tokio_postgres::types::ToSql + Sync,
{
    client
        .query_opt(query, &[id])
        .await
        .map_err(store_error)?
        .map(|row| from_json(row.get("record_json")))
        .transpose()
}

pub(super) async fn version_error<C, I>(
    client: &C,
    entity: &'static str,
    table: &str,
    key: &str,
    id: &I,
    expected: u64,
) -> tm_drive::Result<DriveError>
where
    C: GenericClient + Sync,
    I: tokio_postgres::types::ToSql + Sync + ToString,
{
    let query = format!("select version from {table} where {key}=$1");
    let row = client.query_opt(&query, &[id]).await.map_err(store_error)?;
    Ok(match row {
        Some(row) => DriveError::Conflict {
            entity,
            id: id.to_string(),
            expected,
            actual: u64::try_from(row.get::<_, i64>("version"))
                .map_err(|_| DriveError::Store("negative drive record version".to_string()))?,
        },
        None => DriveError::NotFound(format!("{entity} {}", id.to_string())),
    })
}

pub(super) fn validate_replacement(
    entity: &'static str,
    expected_id: impl ToString,
    actual_id: impl ToString,
) -> tm_drive::Result<()> {
    if expected_id.to_string() == actual_id.to_string() {
        Ok(())
    } else {
        Err(DriveError::InvalidArgs(format!(
            "replacement {entity} id {} does not match {}",
            actual_id.to_string(),
            expected_id.to_string()
        )))
    }
}

pub(super) fn require_version(
    entity: &'static str,
    id: impl ToString,
    expected: u64,
    actual: u64,
) -> tm_drive::Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(DriveError::Conflict {
            entity,
            id: id.to_string(),
            expected,
            actual,
        })
    }
}

pub(super) fn next_version(
    entity: &'static str,
    id: impl ToString,
    version: u64,
) -> tm_drive::Result<u64> {
    version.checked_add(1).ok_or_else(|| {
        DriveError::Store(format!(
            "drive {entity} {} version overflow",
            id.to_string()
        ))
    })
}

pub(super) fn to_i64(label: &str, value: u64) -> tm_drive::Result<i64> {
    i64::try_from(value).map_err(|_| DriveError::Store(format!("{label} exceeds postgres bigint")))
}

pub(super) fn json_value<T: Serialize>(value: &T) -> tm_drive::Result<Value> {
    serde_json::to_value(value).map_err(|err| DriveError::Store(err.to_string()))
}

fn from_json<T: DeserializeOwned>(value: Value) -> tm_drive::Result<T> {
    serde_json::from_value(value).map_err(|err| DriveError::Store(err.to_string()))
}

pub(super) fn enum_label<T: Serialize>(value: &T) -> tm_drive::Result<String> {
    match json_value(value)? {
        Value::String(value) => Ok(value),
        _ => Err(DriveError::Store(
            "drive enum did not serialize as a string".to_string(),
        )),
    }
}

pub(super) fn map_entry_write_error(error: tokio_postgres::Error, path: &str) -> DriveError {
    if error.code() == Some(&SqlState::UNIQUE_VIOLATION) {
        DriveError::Collision(path.to_string())
    } else {
        store_error(error)
    }
}

pub(super) fn store_error(error: tokio_postgres::Error) -> DriveError {
    DriveError::Store(error.to_string())
}
