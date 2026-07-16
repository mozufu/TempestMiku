use std::sync::Arc;

use async_trait::async_trait;
use tm_artifacts::{ResourceContent, preview};
use tm_host::{HostError, InvocationCtx, ResourceEntry, ResourceHandler, Result as HostResult};
use uuid::Uuid;

use tm_memory::{
    DreamQueueRecord, MemoryRecordKind, MemorySummaryRecord, SkillProposalRecord,
    StoredMemoryRecord,
};

use crate::store::{ProfileFactRecord, RecallChunkRecord};
use crate::{ServerError, Store};

use super::util::{decode_memory_segment, encode_memory_segment, short_id};
use super::{
    DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS, DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES,
    DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT, MemoryContext,
};

#[derive(Clone)]
pub struct MemoryResourceHandler<S> {
    store: Arc<S>,
    subject: String,
    scope: String,
    recall_limit: usize,
    prompt_budget_tokens: usize,
    preview_bytes: usize,
}

impl<S> MemoryResourceHandler<S> {
    pub fn new(store: Arc<S>, subject: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            store,
            subject: subject.into(),
            scope: scope.into(),
            recall_limit: DEFAULT_MEMORY_RESOURCE_RECALL_LIMIT,
            prompt_budget_tokens: DEFAULT_MEMORY_PROMPT_BUDGET_TOKENS,
            preview_bytes: DEFAULT_MEMORY_RESOURCE_PREVIEW_BYTES,
        }
    }

    pub fn with_recall_limit(mut self, recall_limit: usize) -> Self {
        self.recall_limit = recall_limit;
        self
    }

    pub fn with_prompt_budget_tokens(mut self, prompt_budget_tokens: usize) -> Self {
        self.prompt_budget_tokens = prompt_budget_tokens;
        self
    }

    pub fn with_preview_bytes(mut self, preview_bytes: usize) -> Self {
        self.preview_bytes = preview_bytes;
        self
    }
}

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

    async fn root_entries(&self) -> HostResult<Vec<ResourceEntry>> {
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

    async fn profile_fact_entries(&self) -> HostResult<Vec<ResourceEntry>> {
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

    async fn dream_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
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

    async fn recall_chunk_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
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

    async fn memory_record_entries(
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

    async fn recall_trace_entries(
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

    async fn skill_proposal_entries(&self, session_id: Uuid) -> HostResult<Vec<ResourceEntry>> {
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

    async fn review_proposal_entries(&self, session_id: Uuid) -> HostResult<Vec<ResourceEntry>> {
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

    async fn summary_entries(&self, scope: &str) -> HostResult<Vec<ResourceEntry>> {
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

    fn text_resource(
        &self,
        uri: &str,
        kind: &str,
        title: Option<String>,
        content: String,
        selector: Option<&str>,
    ) -> HostResult<ResourceContent> {
        let size_bytes = content.len();
        let (selected, has_more) = select_memory_text(&content, selector)?;
        Ok(ResourceContent {
            uri: uri.to_string(),
            kind: kind.to_string(),
            mime: "text/plain".to_string(),
            title,
            size_bytes,
            selector: selector.map(str::to_string),
            has_more,
            preview: preview(&selected, self.preview_bytes),
            content: selected,
        })
    }

    fn json_resource(
        &self,
        uri: &str,
        kind: &str,
        title: Option<String>,
        value: serde_json::Value,
        selector: Option<&str>,
    ) -> HostResult<ResourceContent> {
        let content = serde_json::to_string_pretty(&value)
            .map_err(|error| HostError::HostCall(error.to_string()))?;
        let mut resource = self.text_resource(uri, kind, title, content, selector)?;
        resource.mime = "application/json".to_string();
        Ok(resource)
    }
}

fn ensure_authorized_subject(
    authority: &tm_host::MemoryAuthority,
    subject: &str,
    uri: &str,
) -> HostResult<()> {
    if authority.subject == subject {
        Ok(())
    } else {
        Err(unauthorized_memory_resource(uri))
    }
}

fn ensure_authorized_scope(
    authority: &tm_host::MemoryAuthority,
    scope: &str,
    uri: &str,
) -> HostResult<()> {
    if authority.scope == scope {
        Ok(())
    } else {
        Err(unauthorized_memory_resource(uri))
    }
}

fn ensure_authorized_record(
    authority: &tm_host::MemoryAuthority,
    subject: &str,
    scope: &str,
    uri: &str,
) -> HostResult<()> {
    ensure_authorized_subject(authority, subject, uri)?;
    ensure_authorized_scope(authority, scope, uri)
}

fn unauthorized_memory_resource(uri: &str) -> HostError {
    HostError::NotFound(format!("memory resource {uri}"))
}

enum MemoryUri {
    Root,
    UserModel,
    Dreams,
    EvolutionAudits,
    Dream { id: Uuid },
    ProfileFact { subject: String, id: Uuid },
    RecallChunk { scope: String, id: Uuid },
    Record { kind: MemoryRecordKind, id: Uuid },
    RecallTrace { turn_id: Uuid },
    Summary { id: Uuid },
    SkillProposal { id: Uuid },
    EvolutionProposal { id: Uuid },
    ReviewProposal { id: Uuid },
}

enum MemoryListUri {
    Root,
    UserModel,
    ScopeChunks { scope: String },
    Records,
    Recalls,
    Summaries,
    Dreams,
    SkillProposals,
    ReviewProposals,
}

fn parse_memory_uri(uri: &str) -> HostResult<MemoryUri> {
    let path = uri
        .strip_prefix("memory://")
        .ok_or_else(|| unsupported_memory_uri(uri))?;
    if path.is_empty() || path == "root" {
        return Ok(MemoryUri::Root);
    }
    if path == "user-model" {
        return Ok(MemoryUri::UserModel);
    }
    if path == "dreams" {
        return Ok(MemoryUri::Dreams);
    }
    if path == "evolution-audits" {
        return Ok(MemoryUri::EvolutionAudits);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["dreams", id] => Ok(MemoryUri::Dream {
            id: parse_memory_uuid(id, uri)?,
        }),
        ["profile", subject, "facts", id] => Ok(MemoryUri::ProfileFact {
            subject: decode_memory_segment(subject)?,
            id: parse_memory_uuid(id, uri)?,
        }),
        ["scopes", scope, "chunks", id] => Ok(MemoryUri::RecallChunk {
            scope: decode_memory_segment(scope)?,
            id: parse_memory_uuid(id, uri)?,
        }),
        ["records", kind, id] => Ok(MemoryUri::Record {
            kind: parse_memory_record_kind(kind, uri)?,
            id: parse_memory_uuid(id, uri)?,
        }),
        ["recalls", turn_id] => Ok(MemoryUri::RecallTrace {
            turn_id: parse_memory_uuid(turn_id, uri)?,
        }),
        ["summaries", id] => Ok(MemoryUri::Summary {
            id: parse_memory_uuid(id, uri)?,
        }),
        ["skill-proposals", id] => Ok(MemoryUri::SkillProposal {
            id: parse_memory_uuid(id, uri)?,
        }),
        ["evolution-proposals", id] => Ok(MemoryUri::EvolutionProposal {
            id: parse_memory_uuid(id, uri)?,
        }),
        ["review-proposals", id] => Ok(MemoryUri::ReviewProposal {
            id: parse_memory_uuid(id, uri)?,
        }),
        _ => Err(unsupported_memory_uri(uri)),
    }
}

fn parse_memory_list_uri(uri: &str) -> HostResult<MemoryListUri> {
    let path = uri
        .strip_prefix("memory://")
        .ok_or_else(|| unsupported_memory_uri(uri))?;
    if path.is_empty() || path == "root" {
        return Ok(MemoryListUri::Root);
    }
    if path == "user-model" {
        return Ok(MemoryListUri::UserModel);
    }
    if path == "dreams" {
        return Ok(MemoryListUri::Dreams);
    }
    if path == "records" {
        return Ok(MemoryListUri::Records);
    }
    if path == "recalls" {
        return Ok(MemoryListUri::Recalls);
    }
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["scopes", scope, "chunks"] => Ok(MemoryListUri::ScopeChunks {
            scope: decode_memory_segment(scope)?,
        }),
        ["summaries"] => Ok(MemoryListUri::Summaries),
        ["skill-proposals"] => Ok(MemoryListUri::SkillProposals),
        ["review-proposals"] => Ok(MemoryListUri::ReviewProposals),
        _ => Err(unsupported_memory_uri(uri)),
    }
}

fn unsupported_memory_uri(uri: &str) -> HostError {
    HostError::InvalidPath(format!("unsupported memory uri {uri}"))
}

fn parse_memory_uuid(value: &str, uri: &str) -> HostResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| HostError::InvalidPath(format!("invalid memory uri {uri}")))
}

fn parse_memory_record_kind(value: &str, uri: &str) -> HostResult<MemoryRecordKind> {
    MemoryRecordKind::parse(value).ok_or_else(|| unsupported_memory_uri(uri))
}

fn profile_fact_uri(fact: &ProfileFactRecord) -> String {
    format!(
        "memory://profile/{}/facts/{}",
        encode_memory_segment(&fact.subject),
        fact.id
    )
}

fn recall_chunk_uri(chunk: &RecallChunkRecord) -> String {
    format!(
        "memory://scopes/{}/chunks/{}",
        encode_memory_segment(&chunk.scope),
        chunk.id
    )
}

fn memory_record_uri(record: &StoredMemoryRecord) -> String {
    format!(
        "memory://records/{}/{}",
        record.kind().as_str(),
        record.id()
    )
}

fn memory_record_title(record: &StoredMemoryRecord) -> String {
    match &record.resource {
        tm_memory::MemoryRecordResource::Episodic(record) => preview(&record.text, 120),
        tm_memory::MemoryRecordResource::Semantic(record) => format!(
            "{} {} {}",
            record.semantic_subject, record.predicate, record.object
        ),
    }
}

fn dream_uri(dream: &DreamQueueRecord) -> String {
    format!("memory://dreams/{}", dream.id)
}

fn summary_uri(summary: &MemorySummaryRecord) -> String {
    format!("memory://summaries/{}", summary.id)
}

fn skill_proposal_uri(proposal: &SkillProposalRecord) -> String {
    format!("memory://skill-proposals/{}", proposal.id)
}

fn evolution_proposal_uri(id: Uuid) -> String {
    format!("memory://evolution-proposals/{id}")
}

fn review_proposal_uri(id: Uuid) -> String {
    format!("memory://review-proposals/{id}")
}

fn render_dream_queue(scope: &str, dreams: &[DreamQueueRecord]) -> String {
    let mut lines = vec![
        "Dream queue".to_string(),
        format!("Scope: {scope}"),
        format!("Dreams: {}", dreams.len()),
    ];
    if dreams.is_empty() {
        lines.push("No dreams in this scope.".to_string());
    } else {
        lines.extend(dreams.iter().map(|dream| {
            format!(
                "- {} :: status={} reason={} session={} attempts={} available_at={} locked_at={} last_error={}",
                dream_uri(dream),
                dream.status,
                dream.reason,
                dream.session_id,
                dream.attempts,
                dream.available_at.to_rfc3339(),
                dream.locked_at
                    .map(|time| time.to_rfc3339())
                    .unwrap_or_else(|| "none".to_string()),
                dream.last_error.as_deref().unwrap_or("none")
            )
        }));
    }
    lines.join("\n")
}

fn render_dream(dream: &DreamQueueRecord) -> String {
    [
        format!("Dream {}", dream.id),
        format!("URI: {}", dream_uri(dream)),
        format!("Session: {}", dream.session_id),
        format!("Subject: {}", dream.subject),
        format!("Scope: {}", dream.scope),
        format!("Reason: {}", dream.reason),
        format!("Status: {}", dream.status),
        format!("Dedupe key: {}", dream.dedupe_key),
        format!(
            "Source event seq: {}",
            dream
                .source_event_seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!("Attempts: {}", dream.attempts),
        format!("Enqueued at: {}", dream.enqueued_at.to_rfc3339()),
        format!("Available at: {}", dream.available_at.to_rfc3339()),
        format!(
            "Locked at: {}",
            dream
                .locked_at
                .map(|time| time.to_rfc3339())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "Last error: {}",
            dream.last_error.as_deref().unwrap_or("none")
        ),
    ]
    .join("\n")
}

fn render_memory_root(
    subject: &str,
    scope: &str,
    context: &MemoryContext,
    facts: &[ProfileFactRecord],
    summaries: &[MemorySummaryRecord],
    chunks: &[RecallChunkRecord],
) -> String {
    let mut lines = vec![
        "Memory root".to_string(),
        format!("Subject: {subject}"),
        format!("Scope: {scope}"),
        format!("User model: memory://user-model"),
        format!(
            "Budget: {}/{} estimated tokens; profile facts: {}/{}; summaries: {}/{}; scoped recall: {}/{}; truncated: {}",
            context.budget.used_estimated_tokens,
            context.budget.max_tokens,
            context.budget.included_profile_facts,
            context.budget.available_profile_facts,
            context.budget.included_summaries,
            context.budget.available_summaries,
            context.budget.included_recall_chunks,
            context.budget.available_recall_chunks,
            context.budget.truncated
        ),
    ];
    if facts.is_empty() {
        lines.push("Profile facts: none".to_string());
    } else {
        lines.push("Profile facts:".to_string());
        lines.extend(facts.iter().map(|fact| {
            format!(
                "- {} :: {} {} {} (confidence {:.2}; importance {:.2}; provenance: {})",
                profile_fact_uri(fact),
                fact.subject,
                fact.predicate,
                fact.object,
                fact.confidence,
                fact.importance,
                fact.provenance
            )
        }));
    }
    if summaries.is_empty() {
        lines.push("Recent summaries: none".to_string());
    } else {
        lines.push("Recent summaries:".to_string());
        lines.extend(summaries.iter().map(|summary| {
            format!(
                "- {} :: {} summary: {} (source dream: {})",
                summary_uri(summary),
                summary.kind,
                summary.title,
                summary.source_dream_id
            )
        }));
    }
    if chunks.is_empty() {
        lines.push("Scoped recall chunks: none".to_string());
    } else {
        lines.push("Scoped recall chunks:".to_string());
        lines.extend(chunks.iter().map(|chunk| {
            format!(
                "- {} :: {} (source: {}; importance: {:.2})",
                recall_chunk_uri(chunk),
                chunk.text,
                chunk.source,
                chunk.importance
            )
        }));
    }
    lines.join("\n")
}

fn render_user_model(subject: &str, facts: &[ProfileFactRecord]) -> String {
    let mut lines = vec![
        format!("User model"),
        format!("Subject: {subject}"),
        format!("Facts: {}", facts.len()),
    ];
    if facts.is_empty() {
        lines.push("No active profile facts.".to_string());
    } else {
        lines.extend(facts.iter().map(|fact| {
            format!(
                "- {} :: {} {} {} (confidence {:.2}; importance {:.2}; provenance: {}; valid from: {})",
                profile_fact_uri(fact),
                fact.subject,
                fact.predicate,
                fact.object,
                fact.confidence,
                fact.importance,
                fact.provenance,
                fact.valid_from.to_rfc3339()
            )
        }));
    }
    lines.join("\n")
}

fn render_profile_fact(fact: &ProfileFactRecord) -> String {
    [
        format!("Profile fact {}", fact.id),
        format!("URI: {}", profile_fact_uri(fact)),
        format!("Subject: {}", fact.subject),
        format!("Predicate: {}", fact.predicate),
        format!("Object: {}", fact.object),
        format!("Confidence: {:.2}", fact.confidence),
        format!("Importance: {:.2}", fact.importance),
        format!("Provenance: {}", fact.provenance),
        format!("Valid from: {}", fact.valid_from.to_rfc3339()),
        format!(
            "Valid to: {}",
            fact.valid_to
                .map(|time| time.to_rfc3339())
                .unwrap_or_else(|| "active".to_string())
        ),
    ]
    .join("\n")
}

fn render_recall_chunk(chunk: &RecallChunkRecord) -> String {
    [
        format!("Scoped recall chunk {}", chunk.id),
        format!("URI: {}", recall_chunk_uri(chunk)),
        format!("Scope: {}", chunk.scope),
        format!("Source: {}", chunk.source),
        format!("Importance: {:.2}", chunk.importance),
        format!("Created at: {}", chunk.created_at.to_rfc3339()),
        format!("Text: {}", chunk.text),
    ]
    .join("\n")
}

fn render_summary(summary: &MemorySummaryRecord) -> String {
    let mut lines = vec![
        format!("Memory summary {}", summary.id),
        format!("URI: {}", summary_uri(summary)),
        format!("Kind: {}", summary.kind),
        format!("Subject: {}", summary.subject),
        format!("Scope: {}", summary.scope),
        format!("Title: {}", summary.title),
        format!("Source dream: {}", summary.source_dream_id),
        format!(
            "Source session: {}",
            summary
                .source_session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        format!("Updated at: {}", summary.updated_at.to_rfc3339()),
        "Evidence:".to_string(),
    ];
    if summary.evidence.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(summary.evidence.iter().map(|evidence| {
            format!(
                "- {} event={:?} message={:?} uri={}",
                evidence.label,
                evidence.event_seq,
                evidence.message_seq,
                evidence.uri.as_deref().unwrap_or("none")
            )
        }));
    }
    lines.push("Body:".to_string());
    lines.push(summary.body.clone());
    lines.join("\n")
}

fn render_skill_proposal(proposal: &SkillProposalRecord) -> String {
    let lifecycle = tm_memory::skill_proposal_lifecycle(proposal);
    let mut lines = vec![
        format!("Skill proposal {}", proposal.id),
        format!("URI: {}", skill_proposal_uri(proposal)),
        format!("Name: {}", proposal.name),
        format!("Status: {}", proposal.status),
        format!("Normalized name: {}", lifecycle.normalized_name),
        format!("Candidate version: {}", lifecycle.version),
        format!("Content digest: {}", lifecycle.content_digest),
        format!("Reviewable: {}", lifecycle.reviewable),
        format!("Installable: {}", lifecycle.installable),
        format!("Conflict policy: {:?}", lifecycle.conflict_policy),
        format!("Rollback contract: {:?}", lifecycle.rollback),
        format!("Catalog reload: {:?}", lifecycle.catalog_reload),
        format!("Validation violations: {}", lifecycle.violations.join(", ")),
        format!("Description: {}", proposal.description),
        format!("Trigger: {}", proposal.trigger),
        format!("Use criteria: {}", proposal.use_criteria),
        format!("Source dream: {}", proposal.source_dream_id),
        format!("Source session: {}", proposal.source_session_id),
        format!("Verification passed: {}", proposal.verification.passed),
        format!(
            "Verification checks: {}",
            proposal.verification.checks.join(", ")
        ),
        format!("Self critique: {}", proposal.self_critique),
        "Evidence:".to_string(),
    ];
    if proposal.evidence.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(proposal.evidence.iter().map(|evidence| {
            format!(
                "- {} event={:?} message={:?} uri={}",
                evidence.label,
                evidence.event_seq,
                evidence.message_seq,
                evidence.uri.as_deref().unwrap_or("none")
            )
        }));
    }
    lines.push("Body:".to_string());
    lines.push(proposal.body.clone());
    lines.join("\n")
}

fn select_memory_text(content: &str, selector: Option<&str>) -> HostResult<(String, bool)> {
    let Some(selector) = selector else {
        return Ok((content.to_string(), false));
    };
    let (start, end) = selector
        .split_once('-')
        .ok_or_else(|| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let start: usize = start
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    let end: usize = end
        .parse()
        .map_err(|_| HostError::InvalidArgs(format!("invalid selector {selector}")))?;
    if start == 0 || end < start {
        return Err(HostError::InvalidArgs(format!(
            "invalid selector {selector}"
        )));
    }
    let lines = content.lines().collect::<Vec<_>>();
    let selected = lines
        .iter()
        .skip(start - 1)
        .take(end - start + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Ok((selected, end < lines.len()))
}

fn map_memory_store_error(err: ServerError) -> HostError {
    match err {
        ServerError::NotFound(target) => HostError::NotFound(target),
        ServerError::Policy(message) => HostError::InvalidPath(message),
        ServerError::Forbidden => HostError::InvalidPath("forbidden memory resource".to_string()),
        ServerError::InvalidRequest(message) | ServerError::Conflict(message) => {
            HostError::InvalidArgs(message)
        }
        ServerError::Unauthorized => {
            HostError::CapabilityDenied("resources.read:memory".to_string())
        }
        ServerError::Store(message) | ServerError::Backend(message) => HostError::HostCall(message),
    }
}
