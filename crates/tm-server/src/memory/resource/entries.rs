use tm_artifacts::preview;
use tm_host::{ResourceEntry, Result as HostResult};
use uuid::Uuid;

use crate::Store;

use super::super::util::short_id;
use super::MemoryResourceHandler;
use super::access::map_memory_store_error;
use super::uri::{
    dream_uri, evolution_episode_uri, memory_record_title, memory_record_uri, profile_fact_uri,
    recall_chunk_uri, review_proposal_uri, skill_proposal_uri, summary_uri,
};

impl<S> MemoryResourceHandler<S>
where
    S: Store,
{
    pub(super) async fn root_entries(&self) -> HostResult<Vec<ResourceEntry>> {
        let mut entries = vec![
            ResourceEntry {
                uri: "memory://root".to_string(),
                name: "root".to_string(),
                kind: "memory_root".to_string(),
                title: Some("Memory root".to_string()),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://user-model".to_string(),
                name: "user-model".to_string(),
                kind: "memory_user_model".to_string(),
                title: Some(format!("{} user model", self.subject)),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://records".to_string(),
                name: "records".to_string(),
                kind: "memory_record_collection".to_string(),
                title: Some(format!("{} typed memory records", self.scope)),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://recalls".to_string(),
                name: "recalls".to_string(),
                kind: "memory_recall_trace_collection".to_string(),
                title: Some("Turn recall traces".to_string()),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://dreams".to_string(),
                name: "dreams".to_string(),
                kind: "memory_dream_queue".to_string(),
                title: Some(format!("{} dream queue", self.scope)),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://evolution-audits".to_string(),
                name: "evolution-audits".to_string(),
                kind: "memory_evolution_audits".to_string(),
                title: Some("Self-evolution audit history".to_string()),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://evolution/episodes".to_string(),
                name: "evolution-episodes".to_string(),
                kind: "evolution_episode_collection".to_string(),
                title: Some(format!("{} evolution episodes", self.scope)),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://skill-proposals".to_string(),
                name: "skill-proposals".to_string(),
                kind: "memory_skill_proposal_collection".to_string(),
                title: Some("Skill proposals".to_string()),
                size_bytes: None,
                modified_at: None,
            },
            ResourceEntry {
                uri: "memory://review-proposals".to_string(),
                name: "review-proposals".to_string(),
                kind: "memory_evolution_review_proposal_collection".to_string(),
                title: Some("Persona and mode review proposals".to_string()),
                size_bytes: None,
                modified_at: None,
            },
        ];
        entries.extend(self.dream_entries(&self.scope).await?);
        entries.extend(self.profile_fact_entries().await?);
        entries.extend(self.recall_chunk_entries(&self.scope).await?);
        entries.extend(self.summary_entries(&self.scope).await?);
        Ok(entries)
    }

    pub(super) async fn profile_fact_entries(&self) -> HostResult<Vec<ResourceEntry>> {
        let facts = self
            .store
            .profile_facts(&self.subject)
            .await
            .map_err(map_memory_store_error)?;
        Ok(facts
            .into_iter()
            .map(|fact| ResourceEntry {
                uri: profile_fact_uri(&fact),
                name: short_id(fact.id),
                kind: "memory_profile_fact".to_string(),
                title: Some(format!(
                    "{} {} {}",
                    fact.subject, fact.predicate, fact.object
                )),
                size_bytes: None,
                modified_at: Some(fact.valid_from.to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn dream_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
        let dreams = self
            .store
            .dream_queue(scope, self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        Ok(dreams
            .into_iter()
            .map(|dream| ResourceEntry {
                uri: dream_uri(&dream),
                name: short_id(dream.id),
                kind: "memory_dream".to_string(),
                title: Some(format!("{} {}", dream.reason, dream.status)),
                size_bytes: None,
                modified_at: Some(dream.enqueued_at.to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn recall_chunk_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
        let chunks = self
            .store
            .recall_chunks(scope, "", self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        Ok(chunks
            .into_iter()
            .map(|chunk| ResourceEntry {
                uri: recall_chunk_uri(&chunk),
                name: short_id(chunk.id),
                kind: "memory_recall_chunk".to_string(),
                title: Some(preview(&chunk.text, 120)),
                size_bytes: Some(chunk.text.len()),
                modified_at: Some(chunk.created_at.to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn memory_record_entries(
        &self,
        authority: &tm_host::MemoryAuthority,
    ) -> HostResult<Vec<ResourceEntry>> {
        let records = self
            .store
            .active_memory_records(&authority.subject, &authority.scope, self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        Ok(records
            .into_iter()
            .map(|record| ResourceEntry {
                uri: memory_record_uri(&record),
                name: short_id(record.id()),
                kind: format!("memory_{}_record", record.kind().as_str()),
                title: Some(memory_record_title(&record)),
                size_bytes: serde_json::to_vec(&record).ok().map(|value| value.len()),
                modified_at: Some(record.resource.observed_at().to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn recall_trace_entries(
        &self,
        session_id: Uuid,
        authority: &tm_host::MemoryAuthority,
    ) -> HostResult<Vec<ResourceEntry>> {
        let events = self
            .store
            .memory_recall_events(
                session_id,
                &authority.subject,
                &authority.scope,
                self.recall_limit,
            )
            .await
            .map_err(map_memory_store_error)?;
        Ok(events
            .into_iter()
            .filter_map(|event| {
                let turn_id = event.turn_id?;
                Some(ResourceEntry {
                    uri: format!("memory://recalls/{turn_id}"),
                    name: short_id(turn_id),
                    kind: "memory_recall_trace".to_string(),
                    title: Some(format!("turn {} recall", short_id(turn_id))),
                    size_bytes: serde_json::to_vec(&event.payload_json)
                        .ok()
                        .map(|value| value.len()),
                    modified_at: Some(event.created_at.to_rfc3339()),
                })
            })
            .collect())
    }

    pub(super) async fn evolution_episode_entries(
        &self,
        authority: &tm_host::MemoryAuthority,
    ) -> HostResult<Vec<ResourceEntry>> {
        let episodes = self
            .store
            .evolution_episodes(&authority.subject, &authority.scope, 50)
            .await
            .map_err(map_memory_store_error)?;
        Ok(episodes
            .into_iter()
            .map(|episode| ResourceEntry {
                uri: evolution_episode_uri(&episode),
                name: short_id(episode.id),
                kind: "evolution_episode".to_string(),
                title: Some(format!(
                    "{} turn {}",
                    episode.status,
                    short_id(episode.turn_id)
                )),
                size_bytes: serde_json::to_vec(&episode).ok().map(|value| value.len()),
                modified_at: Some(episode.updated_at.to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn skill_proposal_entries(
        &self,
        session_id: Uuid,
    ) -> HostResult<Vec<ResourceEntry>> {
        let proposals = self
            .store
            .skill_proposals_for_session(session_id)
            .await
            .map_err(map_memory_store_error)?;
        Ok(proposals
            .into_iter()
            .map(|proposal| ResourceEntry {
                uri: skill_proposal_uri(&proposal),
                name: proposal.name.clone(),
                kind: "memory_skill_proposal".to_string(),
                title: Some(format!("{} ({})", proposal.name, proposal.status)),
                size_bytes: Some(proposal.body.len()),
                modified_at: Some(proposal.updated_at.to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn review_proposal_entries(
        &self,
        session_id: Uuid,
    ) -> HostResult<Vec<ResourceEntry>> {
        let proposals = self
            .store
            .evolution_review_proposals_for_session(session_id)
            .await
            .map_err(map_memory_store_error)?;
        Ok(proposals
            .into_iter()
            .map(|proposal| ResourceEntry {
                uri: review_proposal_uri(proposal.id),
                name: short_id(proposal.id),
                kind: "memory_evolution_review_proposal".to_string(),
                title: Some(format!(
                    "{} {} ({})",
                    proposal.target.kind(),
                    proposal.target.id(),
                    proposal.status
                )),
                size_bytes: serde_json::to_vec(&proposal).ok().map(|value| value.len()),
                modified_at: Some(proposal.updated_at.to_rfc3339()),
            })
            .collect())
    }

    pub(super) async fn summary_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
        let summaries = self
            .store
            .memory_summaries(scope, self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        Ok(summaries
            .into_iter()
            .map(|summary| ResourceEntry {
                uri: summary_uri(&summary),
                name: short_id(summary.id),
                kind: "memory_summary".to_string(),
                title: Some(summary.title),
                size_bytes: Some(summary.body.len()),
                modified_at: Some(summary.updated_at.to_rfc3339()),
            })
            .collect())
    }
}
