use super::*;
use sha2::{Digest, Sha256};
use tm_core::ToolMediator;
#[derive(Debug, Deserialize)]
pub struct PostMessageRequest {
    pub content: String,
    #[serde(default = "default_subject")]
    pub subject: String,
    #[serde(default)]
    pub scope: Option<String>,
}

pub(crate) fn default_subject() -> String {
    "brian".to_string()
}

pub(crate) async fn post_message<S, M, C>(
    State(state): State<AppState<S, M, C>>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PostMessageRequest>,
) -> Result<Json<Value>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    state.auth.authorize(&headers)?;
    let session = state.store.get_session(session_id).await?;
    // Mode changes are no longer silently auto-applied from keyword matches: they come only
    // from the user's picker (apply/override/lock) or, once wired, a model-proposed and
    // user-confirmed `mode_suggest`. This keeps capability modes sticky across turns.
    let turn_profile = mode_profile(&state.persona, &session.mode_state.mode);
    let scope = payload
        .scope
        .clone()
        .unwrap_or_else(|| turn_profile.default_scope.clone());
    let persona_prompt = build_turn_prompt(&state, &session.mode_state.mode, &payload.content);
    state
        .store
        .append_message(session_id, "user", &payload.content)
        .await?;
    let memory = state
        .memory
        .context_for_turn(&payload.subject, &scope, &payload.content)
        .await?;
    let mut recall_blocks = Vec::new();
    if !memory.is_empty() {
        recall_blocks.push(memory.render_prompt_block());
    }
    if let Some(block) = drive_recall_block(&state.drive_store, &scope) {
        recall_blocks.push(block);
    }
    let user_prompt = if recall_blocks.is_empty() {
        payload.content.clone()
    } else {
        format!(
            "{}\n\n[recall]\n{}",
            payload.content,
            recall_blocks.join("\n\n")
        )
    };
    // The model can propose a mode switch mid-turn (`mode_suggest`) whenever a turn actually
    // runs through the ChatRunner path (i.e. not the native coding backend, which has no
    // mediator seam in v1 — see §21.4 / ModeSuggestMediator docs). Built once and shared by
    // both ChatRunner call sites below.
    let mode_suggest_mediator = || -> Option<Arc<dyn ToolMediator>> {
        if session.mode_state.lock_source.is_some() {
            return None;
        }
        let mode_sink: Arc<dyn CodingEventSink> = Arc::new(StoreCodingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        Some(Arc::new(ModeSuggestMediator::new(
            state.clone(),
            session_id,
            mode_sink,
            MODE_SUGGEST_APPROVAL_TIMEOUT,
        )))
    };

    let response = if turn_profile.has_capability("backend.coding") {
        if let Some(backend) = &state.coding_backend {
            let sink = Arc::new(StoreCodingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            backend
                .run_turn(
                    CodingTurn {
                        session_id,
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode.clone(),
                        scope: scope.clone(),
                        capabilities: turn_profile.capabilities.clone(),
                    },
                    sink,
                )
                .await?
                .final_text
        } else {
            let sink = Arc::new(PersistingEventSink::new(
                session_id,
                Arc::clone(&state.store),
                state.sender(session_id),
            ));
            let response = state
                .chat
                .run_turn(
                    ChatTurn {
                        session_id,
                        user_prompt,
                        system_prompt: persona_prompt.system_prompt.clone(),
                        mode: session.mode_state.mode.clone(),
                        scope: scope.clone(),
                    },
                    sink.clone(),
                    mode_suggest_mediator(),
                )
                .await?;
            sink.flush().await?;
            response
        }
    } else {
        let sink = Arc::new(PersistingEventSink::new(
            session_id,
            Arc::clone(&state.store),
            state.sender(session_id),
        ));
        let response = state
            .chat
            .run_turn(
                ChatTurn {
                    session_id,
                    user_prompt,
                    system_prompt: persona_prompt.system_prompt,
                    mode: session.mode_state.mode.clone(),
                    scope: scope.clone(),
                },
                sink.clone(),
                mode_suggest_mediator(),
            )
            .await?;
        sink.flush().await?;
        response
    };

    state
        .store
        .append_message(session_id, "assistant", &response)
        .await?;
    persist_drive_recall_chunks(&state, &scope).await?;
    if turn_profile.has_capability("backend.coding") && !response.trim().is_empty() {
        state
            .store
            .add_recall_chunk(RecallChunkRecord {
                id: Uuid::new_v4(),
                scope: scope.clone(),
                text: format!(
                    "Project summary/open loop from session {session_id}: {}",
                    response.trim()
                ),
                source: format!("session:{session_id}:assistant"),
                importance: 0.58,
                created_at: Utc::now(),
            })
            .await?;
        record_project_observations(
            &state,
            session_id,
            project_id_from_scope(&scope),
            &payload.content,
            &response,
        )
        .await?;
    }
    super::memory_write::spawn_personal_assistant_state_capture(
        state.clone(),
        session_id,
        turn_profile,
        payload.subject,
        scope,
        payload.content,
    );
    Ok(Json(json!({ "status": "accepted" })))
}

fn drive_recall_block(
    drive_store: &Option<tm_drive::InMemoryDriveStore>,
    scope: &str,
) -> Option<String> {
    let store = drive_store.as_ref()?;
    let project = drive_project_from_scope(scope)?;
    let hits = store
        .search(tm_drive::DriveSearchOptions {
            project: Some(project),
            limit: 3,
            return_snippets: true,
            ..tm_drive::DriveSearchOptions::default()
        })
        .ok()?;
    if hits.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    lines.push(format!(
        "Drive recall (scope: {scope}; included docs: {}; raw content remains behind drive:// resources).",
        hits.len()
    ));
    for hit in hits {
        let title = hit.title.as_deref().unwrap_or(&hit.path);
        let snippet = hit
            .snippet
            .unwrap_or_else(|| "No summary available.".to_string());
        lines.push(format!(
            "- [{}; hash: {}; selector: {}] {} -- {}",
            hit.uri,
            hit.content_hash,
            hit.selector.as_deref().unwrap_or("preview"),
            title,
            snippet.replace('\n', " ")
        ));
    }
    Some(lines.join("\n"))
}

async fn persist_drive_recall_chunks<S, M, C>(
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
        .map_err(|err| ServerError::Store(err.to_string()))?;
    let mut persisted = 0;
    for hit in hits {
        let entry = store
            .get(&hit.uri)
            .map_err(|err| ServerError::Store(err.to_string()))?;
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

