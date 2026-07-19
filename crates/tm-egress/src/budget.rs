use std::{collections::HashMap, sync::Mutex};

use crate::{EgressBudgetRequest, EgressBudgetReservation, EgressError, EgressUsage, Result};

#[derive(Debug, Clone, Default)]
struct SessionUsage {
    total: EgressUsage,
    destinations: HashMap<String, EgressUsage>,
    reservations: HashMap<String, ReservationRecord>,
}

#[derive(Debug, Clone)]
struct ReservationRecord {
    reservation: EgressBudgetReservation,
    request_bytes: u64,
    settled: bool,
    response_bytes: u64,
    elapsed_ms: u64,
}

#[derive(Debug, Default)]
pub(crate) struct BudgetBook {
    sessions: Mutex<HashMap<String, SessionUsage>>,
}

impl BudgetBook {
    pub(crate) fn reserve(&self, request: EgressBudgetRequest) -> Result<EgressBudgetReservation> {
        let mut sessions = self.sessions.lock().expect("egress budget lock poisoned");
        let usage = sessions.entry(request.session_id.clone()).or_default();
        if let Some(existing) = usage.reservations.get(&request.reservation_id) {
            if existing.reservation.session_id == request.session_id
                && existing.reservation.destination_id == request.destination_id
                && existing.reservation.response_reserved == request.response_reserved
                && existing.reservation.time_reserved_ms == request.time_reserved_ms
                && existing.request_bytes == request.request_bytes
            {
                return Ok(existing.reservation.clone());
            }
            return Err(EgressError::Durability(
                "egress budget reservation id collision".into(),
            ));
        }
        let destination_usage = usage
            .destinations
            .get(&request.destination_id)
            .copied()
            .unwrap_or_default();
        usage.total.validate_reservation(
            request.session_limits,
            request.request_bytes,
            request.response_reserved,
            request.time_reserved_ms,
        )?;
        destination_usage.validate_reservation(
            request.destination_limits,
            request.request_bytes,
            request.response_reserved,
            request.time_reserved_ms,
        )?;
        reserve_usage(
            &mut usage.total,
            request.request_bytes,
            request.response_reserved,
            request.time_reserved_ms,
        );
        reserve_usage(
            usage
                .destinations
                .entry(request.destination_id.clone())
                .or_default(),
            request.request_bytes,
            request.response_reserved,
            request.time_reserved_ms,
        );
        let reservation = EgressBudgetReservation {
            reservation_id: request.reservation_id,
            session_id: request.session_id,
            destination_id: request.destination_id,
            response_reserved: request.response_reserved,
            time_reserved_ms: request.time_reserved_ms,
        };
        usage.reservations.insert(
            reservation.reservation_id.clone(),
            ReservationRecord {
                reservation: reservation.clone(),
                request_bytes: request.request_bytes,
                settled: false,
                response_bytes: 0,
                elapsed_ms: 0,
            },
        );
        Ok(reservation)
    }

    pub(crate) fn settle(
        &self,
        reservation: EgressBudgetReservation,
        response_bytes: u64,
        elapsed_ms: u64,
    ) -> Result<()> {
        let mut sessions = self.sessions.lock().expect("egress budget lock poisoned");
        let session = sessions.get_mut(&reservation.session_id).ok_or_else(|| {
            EgressError::Durability("egress budget session reservation is missing".into())
        })?;
        let record = session
            .reservations
            .get_mut(&reservation.reservation_id)
            .ok_or_else(|| {
                EgressError::Durability("egress budget reservation is missing".into())
            })?;
        if record.reservation != reservation {
            return Err(EgressError::Durability(
                "egress budget settlement collision".into(),
            ));
        }
        if record.settled {
            if record.response_bytes == response_bytes && record.elapsed_ms == elapsed_ms {
                return Ok(());
            }
            return Err(EgressError::Durability(
                "egress budget reservation already settled differently".into(),
            ));
        }
        settle_usage(
            &mut session.total,
            reservation.response_reserved,
            reservation.time_reserved_ms,
            response_bytes,
            elapsed_ms,
        );
        let destination = session
            .destinations
            .get_mut(&reservation.destination_id)
            .ok_or_else(|| EgressError::Durability("egress destination usage is missing".into()))?;
        settle_usage(
            destination,
            reservation.response_reserved,
            reservation.time_reserved_ms,
            response_bytes,
            elapsed_ms,
        );
        record.settled = true;
        record.response_bytes = response_bytes;
        record.elapsed_ms = elapsed_ms;
        Ok(())
    }

    pub(crate) fn clear_session(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("egress budget lock poisoned")
            .remove(session_id);
    }
}

fn reserve_usage(
    usage: &mut EgressUsage,
    request_bytes: u64,
    response_reserved: u64,
    time_reserved_ms: u64,
) {
    usage.requests = usage.requests.saturating_add(1);
    usage.request_bytes = usage.request_bytes.saturating_add(request_bytes);
    usage.response_reserved = usage.response_reserved.saturating_add(response_reserved);
    usage.time_reserved_ms = usage.time_reserved_ms.saturating_add(time_reserved_ms);
}

fn settle_usage(
    usage: &mut EgressUsage,
    response_reserved: u64,
    time_reserved_ms: u64,
    response_bytes: u64,
    elapsed_ms: u64,
) {
    usage.response_reserved = usage.response_reserved.saturating_sub(response_reserved);
    usage.time_reserved_ms = usage.time_reserved_ms.saturating_sub(time_reserved_ms);
    usage.response_bytes = usage.response_bytes.saturating_add(response_bytes);
    usage.time_ms = usage
        .time_ms
        .saturating_add(elapsed_ms.min(time_reserved_ms));
}
