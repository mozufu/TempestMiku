use crate::actor::{ActorId, ActorRecord, ActorStatus};

use super::inbox::ActorInbox;
use super::{
    ActorKey, MAX_ACTORS_PER_SESSION, MailboxRegistry, RegistryError, root_supervisor_actor_id,
};

impl MailboxRegistry {
    /// Register an actor in the roster.
    pub async fn track_for_session(
        &self,
        session_id: &str,
        record: ActorRecord,
    ) -> Result<(), RegistryError> {
        let supervisor_id = record
            .parent
            .clone()
            .unwrap_or_else(root_supervisor_actor_id);
        self.track_with_supervisor_for_session(session_id, record, supervisor_id)
            .await
    }

    pub async fn track_with_supervisor_for_session(
        &self,
        session_id: &str,
        record: ActorRecord,
        supervisor_id: ActorId,
    ) -> Result<(), RegistryError> {
        self.track_batch_for_session(session_id, vec![(record, supervisor_id)])
            .await
    }

    /// Atomically reserve and register a wave of actors for one exact session.
    pub async fn track_batch_for_session(
        &self,
        session_id: &str,
        records: Vec<(ActorRecord, ActorId)>,
    ) -> Result<(), RegistryError> {
        for (record, _) in &records {
            if let Some(mode) = record.mode.as_deref() {
                crate::actor::validate_text_bytes("mode", mode, crate::actor::MAX_ACTOR_ROLE_BYTES)
                    .map_err(RegistryError::InvalidText)?;
            }
        }
        // Acquire every affected shard before mutating any of them. Cancellation while waiting for
        // locks leaves no half-registered wave; after the last await, commit is synchronous.
        let mut actors = self.actors.write().await;
        let mut inboxes = self.inboxes.write().await;
        let mut cancel_tokens = self.cancel_tokens.write().await;
        let mut actor_supervisors = self.actor_supervisors.write().await;
        let mut supervisors = self.supervisors.write().await;
        let live_count = actors
            .iter()
            .filter(|(candidate, record)| {
                candidate.session_id == session_id && Self::is_live_status(record.status)
            })
            .count();
        let new_live_count = records
            .iter()
            .filter(|(record, _)| Self::is_live_status(record.status))
            .filter(|(record, _)| !actors.contains_key(&ActorKey::new(session_id, &record.id)))
            .count();
        if live_count.saturating_add(new_live_count) > MAX_ACTORS_PER_SESSION {
            return Err(RegistryError::SessionActorLimit(MAX_ACTORS_PER_SESSION));
        }
        for (record, supervisor_id) in records {
            let actor_id = record.id.clone();
            let key = ActorKey::new(session_id, &actor_id);
            actors.insert(key.clone(), record);
            inboxes.entry(key.clone()).or_insert_with(ActorInbox::new);
            cancel_tokens.entry(key.clone()).or_default();
            actor_supervisors.insert(key, supervisor_id.clone());
            supervisors
                .entry(ActorKey::new(session_id, &supervisor_id))
                .or_default()
                .track(actor_id);
        }
        Ok(())
    }

    /// Look up a single actor record by id.
    pub async fn get_for_session(&self, session_id: &str, id: &ActorId) -> Option<ActorRecord> {
        self.actors
            .read()
            .await
            .get(&ActorKey::new(session_id, id))
            .cloned()
    }

    /// Update an actor's status in place.
    pub async fn update_status_for_session(
        &self,
        session_id: &str,
        id: &ActorId,
        status: ActorStatus,
    ) {
        if let Some(rec) = self
            .actors
            .write()
            .await
            .get_mut(&ActorKey::new(session_id, id))
        {
            rec.status = status;
        }
    }

    /// Snapshot of all tracked actor records.
    pub async fn list_for_session(&self, session_id: &str) -> Vec<ActorRecord> {
        self.actors
            .read()
            .await
            .iter()
            .filter(|(key, _)| key.session_id == session_id)
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub async fn live_direct_children_for_session(
        &self,
        session_id: &str,
        parent_id: &ActorId,
    ) -> Vec<ActorRecord> {
        let actors = self.actors.read().await;
        actors
            .iter()
            .filter(|(key, _)| key.session_id == session_id)
            .map(|(_, record)| record)
            .filter(|record| Self::is_live_status(record.status))
            .filter(|record| match record.parent.as_ref() {
                Some(parent) => parent == parent_id,
                None => parent_id.as_str() == "Root",
            })
            .cloned()
            .collect()
    }

    pub fn is_live_status(status: ActorStatus) -> bool {
        status != ActorStatus::Terminated
    }

    pub(super) async fn supervisor_id_for_actor(
        &self,
        session_id: &str,
        actor_id: &ActorId,
        fallback_parent: Option<ActorId>,
    ) -> ActorId {
        self.actor_supervisors
            .read()
            .await
            .get(&ActorKey::new(session_id, actor_id))
            .cloned()
            .or(fallback_parent)
            .unwrap_or_else(root_supervisor_actor_id)
    }
}
