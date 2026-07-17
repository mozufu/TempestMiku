use std::collections::HashSet;

use tm_memory::StoredMemoryRecord;

use crate::{Result, ServerError};

use super::Inner;

fn memory_scope_is_tombstoned(inner: &Inner, owner_subject: &str, memory_scope: &str) -> bool {
    inner.memory_scope_tombstones.iter().any(|tombstone| {
        tombstone.owner_subject == owner_subject && tombstone.memory_scope == memory_scope
    })
}

pub(super) fn ensure_memory_scope_is_readable(
    inner: &Inner,
    owner_subject: &str,
    memory_scope: &str,
) -> Result<()> {
    if memory_scope_is_tombstoned(inner, owner_subject, memory_scope) {
        return Err(ServerError::NotFound(format!(
            "memory scope {owner_subject}/{memory_scope}"
        )));
    }
    Ok(())
}

pub(super) fn ensure_memory_record_links_are_scoped(
    inner: &Inner,
    record: &StoredMemoryRecord,
) -> Result<()> {
    for target_id in [
        record.resource.links().corrects_record_id,
        record.resource.links().corrected_by_record_id,
        record.resource.links().supersedes_record_id,
        record.resource.links().superseded_by_record_id,
    ]
    .into_iter()
    .flatten()
    {
        let target = inner
            .memory_records
            .iter()
            .find(|candidate| candidate.id() == target_id)
            .ok_or_else(|| ServerError::NotFound(format!("memory record {target_id}")))?;
        if target.resource.owner_subject() != record.resource.owner_subject()
            || target.resource.memory_scope() != record.resource.memory_scope()
        {
            return Err(ServerError::NotFound(format!(
                "memory record {target_id} in requested authority"
            )));
        }
    }
    Ok(())
}

pub(super) fn memory_record_is_retrievable(inner: &Inner, record: &StoredMemoryRecord) -> bool {
    record.resource.status().is_retrievable()
        && record.resource.effective_to().is_none()
        && !memory_record_has_active_successor(inner, record)
}

fn memory_record_has_active_successor(inner: &Inner, record: &StoredMemoryRecord) -> bool {
    let owner = record.resource.owner_subject();
    let scope = record.resource.memory_scope();
    let mut visited = HashSet::from([record.id()]);
    let mut frontier = direct_memory_successors(inner, record)
        .into_iter()
        .map(StoredMemoryRecord::id)
        .collect::<Vec<_>>();

    while let Some(id) = frontier.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(successor) = inner.memory_records.iter().find(|candidate| {
            candidate.id() == id
                && candidate.resource.owner_subject() == owner
                && candidate.resource.memory_scope() == scope
        }) else {
            continue;
        };
        if successor.resource.status().is_retrievable()
            && successor.resource.effective_to().is_none()
        {
            return true;
        }
        frontier.extend(
            direct_memory_successors(inner, successor)
                .into_iter()
                .map(StoredMemoryRecord::id),
        );
    }
    false
}

fn direct_memory_successors<'a>(
    inner: &'a Inner,
    record: &StoredMemoryRecord,
) -> Vec<&'a StoredMemoryRecord> {
    inner
        .memory_records
        .iter()
        .filter(|candidate| {
            candidate.resource.owner_subject() == record.resource.owner_subject()
                && candidate.resource.memory_scope() == record.resource.memory_scope()
                && (record.resource.links().corrected_by_record_id == Some(candidate.id())
                    || record.resource.links().superseded_by_record_id == Some(candidate.id())
                    || candidate.resource.links().corrects_record_id == Some(record.id())
                    || candidate.resource.links().supersedes_record_id == Some(record.id()))
        })
        .collect()
}
