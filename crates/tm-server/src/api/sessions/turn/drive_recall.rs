use super::super::*;
use sha2::{Digest, Sha256};

pub(super) async fn persist_drive_recall_chunks<S, M, C>(
    state: &AppState<S, M, C>,
    scope: &str,
) -> Result<usize>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let Some(store) = state.drive_store.as_ref() else {
        return Ok(0);
    };
    let Some(project) = drive_project_from_scope(scope) else {
        return Ok(0);
    };
    let hits = store
        .search(tm_drive::DriveSearchOptions {
            project: Some(project),
            limit: 20,
            return_snippets: true,
            ..tm_drive::DriveSearchOptions::default()
        })
        .await
        .map_err(|err| ServerError::Store(err.to_string()))?;
    let mut persisted = 0;
    for hit in hits {
        let entry = store
            .list(tm_drive::DriveListOptions {
                path: Some(hit.path.clone()),
                recursive: true,
                limit: 1,
                include_archived: true,
            })
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?
            .into_iter()
            .find(|entry| entry.path == hit.path)
            .ok_or_else(|| ServerError::NotFound(hit.uri.clone()))?;
        let chunk = drive_entry_recall_chunk(scope, &entry);
        state.store.upsert_recall_chunk(chunk).await?;
        persisted += 1;
    }
    Ok(persisted)
}

fn drive_project_from_scope(scope: &str) -> Option<String> {
    scope
        .strip_prefix("project:")
        .filter(|project| !project.trim().is_empty())
        .map(str::to_string)
}

fn drive_entry_recall_chunk(scope: &str, entry: &tm_drive::DriveEntry) -> RecallChunkRecord {
    let title = entry.title.as_deref().unwrap_or(&entry.path);
    let summary = entry.summary.as_deref().unwrap_or("No summary available.");
    let provenance = entry.provenance.last();
    let extractor = provenance
        .map(|item| item.extractor.as_str())
        .or_else(|| entry.attributes.first().map(|attr| attr.extractor.as_str()))
        .unwrap_or("unknown");
    let source_session = provenance
        .and_then(|item| item.session_id.as_deref())
        .unwrap_or("unknown");
    let source_event = provenance
        .and_then(|item| item.event_seq)
        .map(|seq| seq.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    RecallChunkRecord {
        id: stable_drive_recall_id(scope, &entry.content_hash),
        scope: scope.to_string(),
        text: format!(
            "Drive document: {title}\nURI: {}\nContent hash: {}\nExtractor: {extractor}\nDoc kind: {}\nTags: {}\nAttributes: {}\nSummary: {}",
            entry.uri,
            entry.content_hash,
            entry.doc_kind.as_deref().unwrap_or("unknown"),
            drive_recall_tags(entry),
            drive_recall_attributes(entry),
            summary.replace('\n', " ")
        ),
        source: format!(
            "drive:{};content_hash:{};extractor:{};source_session:{};source_event:{}",
            entry.uri, entry.content_hash, extractor, source_session, source_event
        ),
        importance: 0.57,
        created_at: entry.updated_at,
    }
}

fn drive_recall_tags(entry: &tm_drive::DriveEntry) -> String {
    if entry.tags.is_empty() {
        "none".to_string()
    } else {
        entry.tags.join(", ")
    }
}

fn drive_recall_attributes(entry: &tm_drive::DriveEntry) -> String {
    let attributes = entry
        .attributes
        .iter()
        .filter(|attr| !matches!(attr.key.as_str(), "summary" | "content_hash"))
        .take(8)
        .map(|attr| format!("{}={} ({:.2})", attr.key, attr.value, attr.confidence))
        .collect::<Vec<_>>();
    if attributes.is_empty() {
        "none".to_string()
    } else {
        attributes.join("; ")
    }
}

fn stable_drive_recall_id(scope: &str, content_hash: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"tempestmiku:drive-recall:v1\0");
    hasher.update(scope.as_bytes());
    hasher.update(b"\0");
    hasher.update(content_hash.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
