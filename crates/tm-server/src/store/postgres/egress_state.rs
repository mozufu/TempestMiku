use chrono::Utc;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::{Result, ServerError};

use super::PostgresStore;

const EFFECT_COLUMNS: &str = "effect_id, session_id, effect_scope_id, session_digest, actor_digest, destination_id, destination_version, target_digest, request_digest, request_bytes, status, result_digest, result_bytes, error_code, error_digest";

impl PostgresStore {
    pub(super) async fn begin_egress_effect(
        &self,
        intent: tm_egress::EgressMutationIntent,
    ) -> Result<tm_egress::EgressMutationClaim> {
        let session_id = parse_uuid("egress mutation session id", &intent.session_id)?;
        let effect_scope_id = parse_uuid("egress mutation effect scope", &intent.effect_scope_id)?;
        let destination_version = to_i64("destination version", intent.destination_version)?;
        let request_bytes = usize_to_i64("request byte count", intent.request_bytes)?;
        let now = Utc::now();
        let row = self
            .client
            .query_opt(
                &format!(
                    "insert into egress_mutation_effects
                        (effect_id, session_id, effect_scope_id, session_digest, actor_digest,
                         destination_id, destination_version, target_digest, request_digest,
                         request_bytes, status, created_at, updated_at)
                     select $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'started', $11, $11
                      where exists (
                            select 1 from session_turns
                             where id = $3 and session_id = $2
                      )
                     on conflict (effect_id) do nothing
                     returning {EFFECT_COLUMNS}"
                ),
                &[
                    &intent.effect_id,
                    &session_id,
                    &effect_scope_id,
                    &intent.session_digest,
                    &intent.actor_digest,
                    &intent.destination_id,
                    &destination_version,
                    &intent.target_digest,
                    &intent.request_digest,
                    &request_bytes,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        if let Some(row) = row {
            return Ok(tm_egress::EgressMutationClaim {
                record: row_to_effect(row)?,
                created: true,
            });
        }
        let existing = self
            .client
            .query_opt(
                &format!(
                    "select {EFFECT_COLUMNS} from egress_mutation_effects where effect_id = $1"
                ),
                &[&intent.effect_id],
            )
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "durable egress mutation turn {effect_scope_id} for session {session_id}"
                ))
            })?;
        let existing = row_to_effect(existing)?;
        if !existing.intent.same_effect_identity(&intent) {
            return Err(ServerError::Conflict(
                "egress mutation effect id collides with different intent".to_string(),
            ));
        }
        Ok(tm_egress::EgressMutationClaim {
            record: existing,
            created: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finish_egress_effect(
        &self,
        effect_id: &str,
        status: tm_egress::EgressMutationStatus,
        result_digest: Option<&str>,
        result_bytes: Option<usize>,
        error_code: Option<&str>,
        error_digest: Option<&str>,
    ) -> Result<tm_egress::EgressMutationRecord> {
        if !status.is_terminal() {
            return Err(ServerError::InvalidRequest(
                "egress mutation finish requires terminal status".to_string(),
            ));
        }
        let result_bytes = result_bytes
            .map(|bytes| usize_to_i64("result byte count", bytes))
            .transpose()?;
        let status_name = status_name(status);
        let now = Utc::now();
        let row = self
            .client
            .query_opt(
                &format!(
                    "update egress_mutation_effects
                        set status = $2, result_digest = $3, result_bytes = $4,
                            error_code = $5, error_digest = $6, updated_at = $7
                      where effect_id = $1 and status = 'started'
                     returning {EFFECT_COLUMNS}"
                ),
                &[
                    &effect_id,
                    &status_name,
                    &result_digest,
                    &result_bytes,
                    &error_code,
                    &error_digest,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        if let Some(row) = row {
            return row_to_effect(row);
        }
        let existing = self
            .client
            .query_opt(
                &format!(
                    "select {EFFECT_COLUMNS} from egress_mutation_effects where effect_id = $1"
                ),
                &[&effect_id],
            )
            .await
            .map_err(store_error)?
            .ok_or_else(|| ServerError::NotFound(format!("egress mutation effect {effect_id}")))?;
        let existing = row_to_effect(existing)?;
        let requested = tm_egress::EgressMutationRecord {
            intent: existing.intent.clone(),
            status,
            result_digest: result_digest.map(str::to_string),
            result_bytes: result_bytes
                .map(|bytes| usize::try_from(bytes).expect("converted from usize")),
            error_code: error_code.map(str::to_string),
            error_digest: error_digest.map(str::to_string),
        };
        if existing == requested {
            return Ok(existing);
        }
        Err(ServerError::Conflict(
            "egress mutation effect already has a different terminal state".to_string(),
        ))
    }

    pub(super) async fn reserve_egress_usage(
        &self,
        request: tm_egress::EgressBudgetRequest,
    ) -> Result<tm_egress::EgressBudgetReservation> {
        let session_id = parse_uuid("egress budget session id", &request.session_id)?;
        let request_bytes = to_i64("request byte count", request.request_bytes)?;
        let response_reserved = to_i64("response reservation", request.response_reserved)?;
        let time_reserved_ms = to_i64("time reservation", request.time_reserved_ms)?;
        let mut client = self.egress_state_client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;

        // The session row is the cross-instance mutex for every usage/reservation transition.
        // Acquire it before looking up the idempotency key so two first-time reservations with the
        // same key cannot both observe absence and race at the unique constraint.
        lock_open_session(&transaction, session_id).await?;
        if let Some(row) = transaction
            .query_opt(
                "select session_id, destination_id, request_bytes, response_reserved,
                        time_reserved_ms, settled
                   from egress_budget_reservations where reservation_id = $1 for update",
                &[&request.reservation_id],
            )
            .await
            .map_err(store_error)?
        {
            let same = row.get::<_, Uuid>("session_id") == session_id
                && row.get::<_, String>("destination_id") == request.destination_id
                && row.get::<_, i64>("request_bytes") == request_bytes
                && row.get::<_, i64>("response_reserved") == response_reserved
                && row.get::<_, i64>("time_reserved_ms") == time_reserved_ms;
            if !same {
                return Err(ServerError::Conflict(
                    "egress budget reservation id collides with different request".to_string(),
                ));
            }
            transaction.commit().await.map_err(store_error)?;
            return Ok(reservation_from_request(request));
        }

        let now = Utc::now();
        transaction
            .execute(
                "insert into egress_session_usage(session_id, updated_at)
                 values ($1, $2) on conflict (session_id) do nothing",
                &[&session_id, &now],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "insert into egress_destination_usage(session_id, destination_id, updated_at)
                 values ($1, $2, $3) on conflict (session_id, destination_id) do nothing",
                &[&session_id, &request.destination_id, &now],
            )
            .await
            .map_err(store_error)?;
        let session_usage = transaction
            .query_one(
                "select requests, request_bytes, response_bytes, response_reserved,
                        time_ms, time_reserved_ms
                   from egress_session_usage where session_id = $1 for update",
                &[&session_id],
            )
            .await
            .map_err(store_error)
            .and_then(row_to_usage)?;
        let destination_usage = transaction
            .query_one(
                "select requests, request_bytes, response_bytes, response_reserved,
                        time_ms, time_reserved_ms
                   from egress_destination_usage
                  where session_id = $1 and destination_id = $2 for update",
                &[&session_id, &request.destination_id],
            )
            .await
            .map_err(store_error)
            .and_then(row_to_usage)?;
        session_usage
            .validate_reservation(
                request.session_limits,
                request.request_bytes,
                request.response_reserved,
                request.time_reserved_ms,
            )
            .map_err(budget_error)?;
        destination_usage
            .validate_reservation(
                request.destination_limits,
                request.request_bytes,
                request.response_reserved,
                request.time_reserved_ms,
            )
            .map_err(budget_error)?;
        transaction
            .execute(
                "update egress_session_usage
                    set requests = requests + 1, request_bytes = request_bytes + $2,
                        response_reserved = response_reserved + $3,
                        time_reserved_ms = time_reserved_ms + $4, updated_at = $5
                  where session_id = $1",
                &[
                    &session_id,
                    &request_bytes,
                    &response_reserved,
                    &time_reserved_ms,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "update egress_destination_usage
                    set requests = requests + 1, request_bytes = request_bytes + $3,
                        response_reserved = response_reserved + $4,
                        time_reserved_ms = time_reserved_ms + $5, updated_at = $6
                  where session_id = $1 and destination_id = $2",
                &[
                    &session_id,
                    &request.destination_id,
                    &request_bytes,
                    &response_reserved,
                    &time_reserved_ms,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "insert into egress_budget_reservations
                    (reservation_id, session_id, destination_id, request_bytes,
                     response_reserved, time_reserved_ms, settled, created_at)
                 values ($1, $2, $3, $4, $5, $6, false, $7)",
                &[
                    &request.reservation_id,
                    &session_id,
                    &request.destination_id,
                    &request_bytes,
                    &response_reserved,
                    &time_reserved_ms,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        Ok(reservation_from_request(request))
    }

    pub(super) async fn settle_egress_usage(
        &self,
        reservation: tm_egress::EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> Result<()> {
        if response_bytes > reservation.response_reserved {
            return Err(ServerError::Conflict(
                "egress response exceeds its durable reservation".to_string(),
            ));
        }
        let session_id = parse_uuid("egress budget session id", &reservation.session_id)?;
        let response_bytes = to_i64("response byte count", response_bytes)?;
        let elapsed_ms = to_i64("elapsed time", elapsed_ms.min(reservation.time_reserved_ms))?;
        let response_reserved = to_i64("response reservation", reservation.response_reserved)?;
        let time_reserved_ms = to_i64("time reservation", reservation.time_reserved_ms)?;
        let mut client = self.egress_state_client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        lock_session(&transaction, session_id).await?;
        let row = transaction
            .query_opt(
                "select session_id, destination_id, response_reserved, time_reserved_ms,
                        settled, response_bytes, elapsed_ms
                   from egress_budget_reservations where reservation_id = $1 for update",
                &[&reservation.reservation_id],
            )
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                ServerError::NotFound(format!(
                    "egress budget reservation {}",
                    reservation.reservation_id
                ))
            })?;
        if row.get::<_, Uuid>("session_id") != session_id
            || row.get::<_, String>("destination_id") != reservation.destination_id
            || row.get::<_, i64>("response_reserved") != response_reserved
            || row.get::<_, i64>("time_reserved_ms") != time_reserved_ms
        {
            return Err(ServerError::Conflict(
                "egress budget settlement collides with its reservation".to_string(),
            ));
        }
        if row.get::<_, bool>("settled") {
            if row.get::<_, Option<i64>>("response_bytes") == Some(response_bytes)
                && row.get::<_, Option<i64>>("elapsed_ms") == Some(elapsed_ms)
            {
                transaction.commit().await.map_err(store_error)?;
                return Ok(());
            }
            return Err(ServerError::Conflict(
                "egress budget reservation was settled differently".to_string(),
            ));
        }
        let now = Utc::now();
        transaction
            .execute(
                "update egress_session_usage
                    set response_reserved = response_reserved - $2,
                        time_reserved_ms = time_reserved_ms - $3,
                        response_bytes = response_bytes + $4,
                        time_ms = time_ms + $5, updated_at = $6
                  where session_id = $1",
                &[
                    &session_id,
                    &response_reserved,
                    &time_reserved_ms,
                    &response_bytes,
                    &elapsed_ms,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "update egress_destination_usage
                    set response_reserved = response_reserved - $3,
                        time_reserved_ms = time_reserved_ms - $4,
                        response_bytes = response_bytes + $5,
                        time_ms = time_ms + $6, updated_at = $7
                  where session_id = $1 and destination_id = $2",
                &[
                    &session_id,
                    &reservation.destination_id,
                    &response_reserved,
                    &time_reserved_ms,
                    &response_bytes,
                    &elapsed_ms,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "update egress_budget_reservations
                    set settled = true, response_bytes = $2, elapsed_ms = $3, settled_at = $4
                  where reservation_id = $1",
                &[
                    &reservation.reservation_id,
                    &response_bytes,
                    &elapsed_ms,
                    &now,
                ],
            )
            .await
            .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        Ok(())
    }

    pub(super) async fn clear_egress_usage(&self, session_id: &str) -> Result<()> {
        let session_id = parse_uuid("egress session id", session_id)?;
        let mut client = self.egress_state_client.lock().await;
        let transaction = client.transaction().await.map_err(store_error)?;
        let status = lock_session(&transaction, session_id).await?;
        if status != "ended" {
            return Err(ServerError::Conflict(format!(
                "active session {session_id} egress state cannot be cleared"
            )));
        }
        transaction
            .execute(
                "delete from egress_budget_reservations where session_id = $1",
                &[&session_id],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "delete from egress_destination_usage where session_id = $1",
                &[&session_id],
            )
            .await
            .map_err(store_error)?;
        transaction
            .execute(
                "delete from egress_session_usage where session_id = $1",
                &[&session_id],
            )
            .await
            .map_err(store_error)?;
        transaction.commit().await.map_err(store_error)?;
        Ok(())
    }
}

async fn lock_session(transaction: &Transaction<'_>, session_id: Uuid) -> Result<String> {
    transaction
        .query_opt(
            "select status from sessions where id = $1 for update",
            &[&session_id],
        )
        .await
        .map_err(store_error)?
        .map(|row| row.get("status"))
        .ok_or_else(|| ServerError::NotFound(format!("session {session_id}")))
}

async fn lock_open_session(transaction: &Transaction<'_>, session_id: Uuid) -> Result<()> {
    let status = lock_session(transaction, session_id).await?;
    if status != "open" {
        return Err(ServerError::Conflict(format!(
            "session {session_id} is not open"
        )));
    }
    Ok(())
}

fn reservation_from_request(
    request: tm_egress::EgressBudgetRequest,
) -> tm_egress::EgressBudgetReservation {
    tm_egress::EgressBudgetReservation {
        reservation_id: request.reservation_id,
        session_id: request.session_id,
        destination_id: request.destination_id,
        response_reserved: request.response_reserved,
        time_reserved_ms: request.time_reserved_ms,
    }
}

fn row_to_usage(row: Row) -> Result<tm_egress::EgressUsage> {
    Ok(tm_egress::EgressUsage {
        requests: from_i64("usage request count", row.get("requests"))?,
        request_bytes: from_i64("usage request bytes", row.get("request_bytes"))?,
        response_bytes: from_i64("usage response bytes", row.get("response_bytes"))?,
        response_reserved: from_i64("usage response reservation", row.get("response_reserved"))?,
        time_ms: from_i64("usage time", row.get("time_ms"))?,
        time_reserved_ms: from_i64("usage time reservation", row.get("time_reserved_ms"))?,
    })
}

fn row_to_effect(row: Row) -> Result<tm_egress::EgressMutationRecord> {
    let status: String = row.get("status");
    let status = match status.as_str() {
        "started" => tm_egress::EgressMutationStatus::Started,
        "succeeded" => tm_egress::EgressMutationStatus::Succeeded,
        "failed" => tm_egress::EgressMutationStatus::Failed,
        "uncertain" => tm_egress::EgressMutationStatus::Uncertain,
        other => {
            return Err(ServerError::Store(format!(
                "invalid persisted egress mutation status {other}"
            )));
        }
    };
    let result_bytes = row
        .get::<_, Option<i64>>("result_bytes")
        .map(|bytes| i64_to_usize("result byte count", bytes))
        .transpose()?;
    Ok(tm_egress::EgressMutationRecord {
        intent: tm_egress::EgressMutationIntent {
            effect_id: row.get("effect_id"),
            session_id: row.get::<_, Uuid>("session_id").to_string(),
            effect_scope_id: row.get::<_, Uuid>("effect_scope_id").to_string(),
            session_digest: row.get("session_digest"),
            actor_digest: row.get("actor_digest"),
            destination_id: row.get("destination_id"),
            destination_version: from_i64("destination version", row.get("destination_version"))?,
            target_digest: row.get("target_digest"),
            request_digest: row.get("request_digest"),
            request_bytes: i64_to_usize("request byte count", row.get("request_bytes"))?,
        },
        status,
        result_digest: row.get("result_digest"),
        result_bytes,
        error_code: row.get("error_code"),
        error_digest: row.get("error_digest"),
    })
}

fn status_name(status: tm_egress::EgressMutationStatus) -> &'static str {
    match status {
        tm_egress::EgressMutationStatus::Started => "started",
        tm_egress::EgressMutationStatus::Succeeded => "succeeded",
        tm_egress::EgressMutationStatus::Failed => "failed",
        tm_egress::EgressMutationStatus::Uncertain => "uncertain",
    }
}

fn parse_uuid(kind: &str, value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|_| ServerError::InvalidRequest(format!("{kind} is not a UUID")))
}

fn usize_to_i64(kind: &str, value: usize) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| ServerError::InvalidRequest(format!("egress {kind} exceeds bounds")))
}

fn to_i64(kind: &str, value: u64) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| ServerError::InvalidRequest(format!("egress {kind} exceeds bounds")))
}

fn from_i64(kind: &str, value: i64) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| ServerError::Store(format!("negative persisted egress {kind}")))
}

fn i64_to_usize(kind: &str, value: i64) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| ServerError::Store(format!("invalid persisted egress {kind}")))
}

fn budget_error(error: tm_egress::EgressError) -> ServerError {
    match error {
        tm_egress::EgressError::Budget(_) => {
            ServerError::Policy("egress budget exceeded".to_string())
        }
        other => ServerError::Store(format!("egress budget failed ({})", other.code())),
    }
}

fn store_error(error: tokio_postgres::Error) -> ServerError {
    ServerError::Store(error.to_string())
}
