use async_trait::async_trait;
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler, Result as HostResult};
use uuid::Uuid;

use crate::Store;

use super::super::MemoryContext;
use super::super::util::short_id;
use super::MemoryResourceHandler;
use super::access::{
    ensure_authorized_record, ensure_authorized_scope, ensure_authorized_subject,
    map_memory_store_error, unauthorized_memory_resource,
};
use super::render::{
    render_dream, render_dream_queue, render_memory_root, render_profile_fact, render_recall_chunk,
    render_skill_proposal, render_summary, render_user_model,
};
use super::uri::{
    MemoryListUri, MemoryUri, dream_uri, evolution_episode_uri, evolution_proposal_uri,
    memory_record_uri, parse_memory_list_uri, parse_memory_uri, profile_fact_uri, recall_chunk_uri,
    review_proposal_uri, skill_proposal_uri, summary_uri,
};

#[async_trait]
impl<S> ResourceHandler for MemoryResourceHandler<S>
where
    S: Store,
{
    fn scheme(&self) -> &str {
        "memory"
    }

    fn capability(&self) -> &str {
        "resources.read:memory"
    }

    async fn read(
        &self,
        uri: &str,
        selector: Option<&str>,
        ctx: &InvocationCtx,
    ) -> HostResult<ResourceContent> {
        let authority = self.authority(ctx, uri)?;
        self.store
            .ensure_memory_scope_active(&authority.subject, &authority.scope)
            .await
            .map_err(map_memory_store_error)?;
        match parse_memory_uri(uri)? {
            MemoryUri::Root => self.root_resource(selector).await,
            MemoryUri::UserModel => self.user_model_resource(selector).await,
            MemoryUri::Dreams => self.dream_queue_resource(selector).await,
            MemoryUri::EvolutionAudits => {
                let session_id = Uuid::parse_str(&ctx.session_id)
                    .map_err(|_| unauthorized_memory_resource(uri))?;
                let records = self
                    .store
                    .evolution_audits(session_id)
                    .await
                    .map_err(map_memory_store_error)?;
                let content = serde_json::to_string_pretty(&records)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                self.text_resource(
                    "memory://evolution-audits",
                    "memory_evolution_audits",
                    Some("Self-evolution audit history".to_string()),
                    content,
                    selector,
                )
            }
            MemoryUri::EvolutionEpisode { id } => {
                let episode = self
                    .store
                    .evolution_episode(id)
                    .await
                    .map_err(map_memory_store_error)?;
                ensure_authorized_record(
                    authority,
                    &episode.owner_subject,
                    &episode.memory_scope,
                    uri,
                )?;
                let traces = self
                    .store
                    .experience_traces(episode.id)
                    .await
                    .map_err(map_memory_store_error)?;
                self.json_resource(
                    &evolution_episode_uri(&episode),
                    "evolution_episode",
                    Some(format!("evolution episode {}", short_id(episode.id))),
                    serde_json::json!({"episode": episode, "traces": traces}),
                    selector,
                )
            }
            MemoryUri::Dream { id } => {
                let dream = self.store.dream(id).await.map_err(map_memory_store_error)?;
                ensure_authorized_record(authority, &dream.subject, &dream.scope, uri)?;
                self.text_resource(
                    &dream_uri(&dream),
                    "memory_dream",
                    Some(format!("dream {}", short_id(dream.id))),
                    render_dream(&dream),
                    selector,
                )
            }
            MemoryUri::ProfileFact { subject, id } => {
                ensure_authorized_subject(authority, &subject, uri)?;
                let fact = self
                    .store
                    .profile_fact(&subject, id)
                    .await
                    .map_err(map_memory_store_error)?;
                self.text_resource(
                    &profile_fact_uri(&fact),
                    "memory_profile_fact",
                    Some(format!("profile fact {}", short_id(fact.id))),
                    render_profile_fact(&fact),
                    selector,
                )
            }
            MemoryUri::RecallChunk { scope, id } => {
                ensure_authorized_scope(authority, &scope, uri)?;
                let chunk = self
                    .store
                    .recall_chunk(&scope, id)
                    .await
                    .map_err(map_memory_store_error)?;
                self.text_resource(
                    &recall_chunk_uri(&chunk),
                    "memory_recall_chunk",
                    Some(format!("recall chunk {}", short_id(chunk.id))),
                    render_recall_chunk(&chunk),
                    selector,
                )
            }
            MemoryUri::Record { kind, id } => {
                let record = self
                    .store
                    .memory_record(&authority.subject, &authority.scope, kind, id)
                    .await
                    .map_err(map_memory_store_error)?;
                self.json_resource(
                    &memory_record_uri(&record),
                    "memory_record",
                    Some(format!("{} memory record {}", kind.as_str(), short_id(id))),
                    serde_json::to_value(record)
                        .map_err(|error| HostError::HostCall(error.to_string()))?,
                    selector,
                )
            }
            MemoryUri::RecallTrace { turn_id } => {
                let session_id = Uuid::parse_str(&ctx.session_id)
                    .map_err(|_| unauthorized_memory_resource(uri))?;
                let event = self
                    .store
                    .event_for_turn(session_id, turn_id, "memory_recall")
                    .await
                    .map_err(map_memory_store_error)?
                    .ok_or_else(|| unauthorized_memory_resource(uri))?;
                let event_subject = event
                    .payload_json
                    .pointer("/context/subject")
                    .and_then(serde_json::Value::as_str);
                let event_scope = event
                    .payload_json
                    .pointer("/context/scope")
                    .and_then(serde_json::Value::as_str);
                if event_subject != Some(authority.subject.as_str())
                    || event_scope != Some(authority.scope.as_str())
                {
                    return Err(unauthorized_memory_resource(uri));
                }
                self.json_resource(
                    uri,
                    "memory_recall_trace",
                    Some(format!("memory recall for turn {}", short_id(turn_id))),
                    event.payload_json,
                    selector,
                )
            }
            MemoryUri::Summary { id } => {
                let summary = self
                    .store
                    .memory_summary(id)
                    .await
                    .map_err(map_memory_store_error)?;
                ensure_authorized_record(authority, &summary.subject, &summary.scope, uri)?;
                self.text_resource(
                    &summary_uri(&summary),
                    "memory_summary",
                    Some(summary.title.clone()),
                    render_summary(&summary),
                    selector,
                )
            }
            MemoryUri::SkillProposal { id } => {
                let proposal = self
                    .store
                    .skill_proposal(id)
                    .await
                    .map_err(map_memory_store_error)?;
                let source_dream = self
                    .store
                    .dream(proposal.source_dream_id)
                    .await
                    .map_err(map_memory_store_error)?;
                ensure_authorized_record(
                    authority,
                    &source_dream.subject,
                    &source_dream.scope,
                    uri,
                )?;
                self.text_resource(
                    &skill_proposal_uri(&proposal),
                    "memory_skill_proposal",
                    Some(proposal.name.clone()),
                    render_skill_proposal(&proposal),
                    selector,
                )
            }
            MemoryUri::EvolutionProposal { id } => {
                let proposal = self
                    .store
                    .evolution_memory_proposal(id)
                    .await
                    .map_err(map_memory_store_error)?;
                ensure_authorized_record(authority, &proposal.subject, &proposal.scope, uri)?;
                let content = serde_json::to_string_pretty(&proposal)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                self.text_resource(
                    &evolution_proposal_uri(id),
                    "memory_evolution_proposal",
                    Some(format!("evolution proposal {}", short_id(id))),
                    content,
                    selector,
                )
            }
            MemoryUri::ReviewProposal { id } => {
                let session_id = Uuid::parse_str(&ctx.session_id)
                    .map_err(|_| unauthorized_memory_resource(uri))?;
                let proposal = self
                    .store
                    .evolution_review_proposal(id)
                    .await
                    .map_err(map_memory_store_error)?;
                if proposal.session_id != session_id {
                    return Err(unauthorized_memory_resource(uri));
                }
                let content = serde_json::to_string_pretty(&proposal)
                    .map_err(|error| HostError::HostCall(error.to_string()))?;
                self.text_resource(
                    &review_proposal_uri(id),
                    "memory_evolution_review_proposal",
                    Some(format!(
                        "{} addendum proposal {}",
                        proposal.target.kind(),
                        short_id(id)
                    )),
                    content,
                    selector,
                )
            }
        }
    }

    async fn preview(&self, uri: &str, ctx: &InvocationCtx) -> HostResult<ResourceContent> {
        let mut content = self.read(uri, None, ctx).await?;
        content.preview = preview(&content.content, self.preview_bytes);
        content.has_more = content.has_more || content.content.len() > self.preview_bytes;
        content.content.clear();
        Ok(content)
    }

    async fn list(&self, uri: Option<&str>, ctx: &InvocationCtx) -> HostResult<Vec<ResourceEntry>> {
        let uri = uri.unwrap_or("memory://root");
        let authority = self.authority(ctx, uri)?;
        self.store
            .ensure_memory_scope_active(&authority.subject, &authority.scope)
            .await
            .map_err(map_memory_store_error)?;
        match parse_memory_list_uri(uri)? {
            MemoryListUri::Root => self.root_entries().await,
            MemoryListUri::UserModel => self.profile_fact_entries().await,
            MemoryListUri::ScopeChunks { scope } => {
                ensure_authorized_scope(authority, &scope, uri)?;
                self.recall_chunk_entries(&scope).await
            }
            MemoryListUri::Records => self.memory_record_entries(authority).await,
            MemoryListUri::Recalls => {
                let session_id = Uuid::parse_str(&ctx.session_id)
                    .map_err(|_| unauthorized_memory_resource(uri))?;
                self.recall_trace_entries(session_id, authority).await
            }
            MemoryListUri::Summaries => self.summary_entries(&self.scope).await,
            MemoryListUri::EvolutionEpisodes => self.evolution_episode_entries(authority).await,
            MemoryListUri::Dreams => self.dream_entries(&self.scope).await,
            MemoryListUri::SkillProposals => {
                let session_id = Uuid::parse_str(&ctx.session_id)
                    .map_err(|_| unauthorized_memory_resource(uri))?;
                self.skill_proposal_entries(session_id).await
            }
            MemoryListUri::ReviewProposals => {
                let session_id = Uuid::parse_str(&ctx.session_id)
                    .map_err(|_| unauthorized_memory_resource(uri))?;
                self.review_proposal_entries(session_id).await
            }
        }
    }
}

impl<S> MemoryResourceHandler<S>
where
    S: Store,
{
    fn authority<'a>(
        &self,
        ctx: &'a InvocationCtx,
        uri: &str,
    ) -> HostResult<&'a tm_host::MemoryAuthority> {
        let authority = ctx
            .memory_authority
            .as_ref()
            .ok_or_else(|| unauthorized_memory_resource(uri))?;
        if authority.subject != self.subject || authority.scope != self.scope {
            return Err(unauthorized_memory_resource(uri));
        }
        Ok(authority)
    }

    async fn root_resource(&self, selector: Option<&str>) -> HostResult<ResourceContent> {
        let facts = self
            .store
            .profile_facts(&self.subject)
            .await
            .map_err(map_memory_store_error)?;
        let chunks = self
            .store
            .recall_chunks(&self.scope, "", self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        let summaries = self
            .store
            .memory_summaries(&self.scope, self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        let context = MemoryContext::from_records_with_summaries(
            &self.subject,
            &self.scope,
            facts.clone(),
            summaries.clone(),
            chunks.clone(),
            self.prompt_budget_tokens,
        );
        let content = render_memory_root(
            &self.subject,
            &self.scope,
            &context,
            &facts,
            &summaries,
            &chunks,
        );
        self.text_resource(
            "memory://root",
            "memory_root",
            Some("Memory root".to_string()),
            content,
            selector,
        )
    }

    async fn user_model_resource(&self, selector: Option<&str>) -> HostResult<ResourceContent> {
        let facts = self
            .store
            .profile_facts(&self.subject)
            .await
            .map_err(map_memory_store_error)?;
        self.text_resource(
            "memory://user-model",
            "memory_user_model",
            Some(format!("{} user model", self.subject)),
            render_user_model(&self.subject, &facts),
            selector,
        )
    }

    async fn dream_queue_resource(&self, selector: Option<&str>) -> HostResult<ResourceContent> {
        let dreams = self
            .store
            .dream_queue(&self.scope, self.recall_limit)
            .await
            .map_err(map_memory_store_error)?;
        self.text_resource(
            "memory://dreams",
            "memory_dream_queue",
            Some(format!("{} dream queue", self.scope)),
            render_dream_queue(&self.scope, &dreams),
            selector,
        )
    }
}
