use tm_artifacts::preview;
use tm_host::{HostError, Result as HostResult};
use tm_memory::{
    DreamQueueRecord, EvolutionEpisodeRecord, MemoryRecordKind, MemorySummaryRecord,
    SkillProposalRecord, StoredMemoryRecord,
};
use uuid::Uuid;

use crate::store::{ProfileFactRecord, RecallChunkRecord};

use super::super::util::{decode_memory_segment, encode_memory_segment};

pub(super) enum MemoryUri {
    Root,
    UserModel,
    Dreams,
    EvolutionAudits,
    EvolutionEpisode { id: Uuid },
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

pub(super) enum MemoryListUri {
    Root,
    UserModel,
    ScopeChunks { scope: String },
    Records,
    Recalls,
    Summaries,
    EvolutionEpisodes,
    Dreams,
    SkillProposals,
    ReviewProposals,
}

pub(super) fn parse_memory_uri(uri: &str) -> HostResult<MemoryUri> {
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
        ["evolution", "episodes", id] => Ok(MemoryUri::EvolutionEpisode {
            id: parse_memory_uuid(id, uri)?,
        }),
        ["review-proposals", id] => Ok(MemoryUri::ReviewProposal {
            id: parse_memory_uuid(id, uri)?,
        }),
        _ => Err(unsupported_memory_uri(uri)),
    }
}

pub(super) fn parse_memory_list_uri(uri: &str) -> HostResult<MemoryListUri> {
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
        ["evolution", "episodes"] => Ok(MemoryListUri::EvolutionEpisodes),
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

pub(super) fn profile_fact_uri(fact: &ProfileFactRecord) -> String {
    format!(
        "memory://profile/{}/facts/{}",
        encode_memory_segment(&fact.subject),
        fact.id
    )
}

pub(super) fn recall_chunk_uri(chunk: &RecallChunkRecord) -> String {
    format!(
        "memory://scopes/{}/chunks/{}",
        encode_memory_segment(&chunk.scope),
        chunk.id
    )
}

pub(super) fn memory_record_uri(record: &StoredMemoryRecord) -> String {
    format!(
        "memory://records/{}/{}",
        record.kind().as_str(),
        record.id()
    )
}

pub(super) fn memory_record_title(record: &StoredMemoryRecord) -> String {
    match &record.resource {
        tm_memory::MemoryRecordResource::Episodic(record) => preview(&record.text, 120),
        tm_memory::MemoryRecordResource::Semantic(record) => format!(
            "{} {} {}",
            record.semantic_subject, record.predicate, record.object
        ),
    }
}

pub(super) fn dream_uri(dream: &DreamQueueRecord) -> String {
    format!("memory://dreams/{}", dream.id)
}

pub(super) fn summary_uri(summary: &MemorySummaryRecord) -> String {
    format!("memory://summaries/{}", summary.id)
}

pub(super) fn skill_proposal_uri(proposal: &SkillProposalRecord) -> String {
    format!("memory://skill-proposals/{}", proposal.id)
}

pub(super) fn evolution_proposal_uri(id: Uuid) -> String {
    format!("memory://evolution-proposals/{id}")
}

pub(super) fn evolution_episode_uri(episode: &EvolutionEpisodeRecord) -> String {
    format!("memory://evolution/episodes/{}", episode.id)
}

pub(super) fn review_proposal_uri(id: Uuid) -> String {
    format!("memory://review-proposals/{id}")
}
