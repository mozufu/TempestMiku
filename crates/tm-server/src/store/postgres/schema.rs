use crate::{Result, ServerError};

use super::PostgresStore;

impl PostgresStore {
    pub(super) async fn ensure_schema(&self) -> Result<()> {
        self.client
            .query_one("select pg_advisory_lock($1)", &[&Self::SCHEMA_LOCK_ID])
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;

        let schema_result = self.ensure_schema_unlocked().await;
        let unlock_result = self
            .client
            .query_one("select pg_advisory_unlock($1)", &[&Self::SCHEMA_LOCK_ID])
            .await
            .map(|_| ())
            .map_err(|err| ServerError::Store(err.to_string()));

        match (schema_result, unlock_result) {
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn ensure_schema_unlocked(&self) -> Result<()> {
        self.client
            .batch_execute(
                "create table if not exists sessions(id uuid primary key, created_at timestamptz not null, updated_at timestamptz not null, status text not null, mode jsonb not null, mode_state_json jsonb, persona_status jsonb not null);
                 alter table sessions add column if not exists mode_state_json jsonb;
                 create table if not exists session_events(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, event_type text not null, payload_json jsonb not null, actor_id text, artifact_uri text, history_uri text, created_at timestamptz not null, primary key(session_id, seq));
                 alter table session_events add column if not exists actor_id text;
                 alter table session_events add column if not exists artifact_uri text;
                 alter table session_events add column if not exists history_uri text;
                 update session_events
                    set actor_id = coalesce(actor_id, payload_json ->> 'actor_id', payload_json ->> 'actorId'),
                        artifact_uri = coalesce(artifact_uri, payload_json ->> 'artifact_uri', payload_json ->> 'artifactUri'),
                        history_uri = coalesce(history_uri, payload_json ->> 'history_uri', payload_json ->> 'historyUri')
                  where event_type in ('actor_completed', 'actor_resources_linked')
                    and ((actor_id is null and coalesce(payload_json ->> 'actor_id', payload_json ->> 'actorId') is not null)
                      or (artifact_uri is null and coalesce(payload_json ->> 'artifact_uri', payload_json ->> 'artifactUri') is not null)
                      or (history_uri is null and coalesce(payload_json ->> 'history_uri', payload_json ->> 'historyUri') is not null));
                 create table if not exists messages(session_id uuid not null references sessions(id) on delete cascade, seq bigint not null, role text not null, content text not null, created_at timestamptz not null, primary key(session_id, seq));
                 create table if not exists profile_facts(id uuid primary key, subject text not null, predicate text not null, object text not null, confidence real not null, provenance text not null, valid_from timestamptz not null, valid_to timestamptz);
                 create table if not exists recall_chunks(id uuid primary key, scope text not null, text text not null, source text not null, created_at timestamptz not null);
                 alter table profile_facts add column if not exists importance real not null default 0.5;
                 alter table recall_chunks add column if not exists importance real not null default 0.5;
                 create table if not exists project_items(id uuid primary key, project_id text not null, kind text not null, text text not null, target_uri text not null, source_session_id uuid not null references sessions(id) on delete cascade, source_event_seq bigint, source_uri text, dedupe_key text not null, provenance_json jsonb not null, created_at timestamptz not null, unique(project_id, kind, dedupe_key));
                 create table if not exists dream_queue(id uuid primary key, session_id uuid not null references sessions(id) on delete cascade, subject text not null, scope text not null, reason text not null, status text not null, dedupe_key text not null unique, source_event_seq bigint, attempts integer not null default 0, enqueued_at timestamptz not null, available_at timestamptz not null, locked_at timestamptz, last_error text);
                 create table if not exists memory_summaries(id uuid primary key, kind text not null, subject text not null, scope text not null, title text not null, body text not null, evidence_json jsonb not null, source_dream_id uuid not null, source_session_id uuid references sessions(id) on delete set null, dedupe_key text not null unique, created_at timestamptz not null, updated_at timestamptz not null);
                 create table if not exists skill_proposals(id uuid primary key, name text not null, description text not null, body text not null, trigger text not null, use_criteria text not null, evidence_json jsonb not null, self_critique text not null, verification_json jsonb not null, status text not null, dedupe_key text not null unique, source_dream_id uuid not null, source_session_id uuid not null references sessions(id) on delete cascade, created_at timestamptz not null, updated_at timestamptz not null);
                 create table if not exists cron_jobs(id text primary key, name text not null, schedule text not null, enabled boolean not null, cron_mode text not null, max_turns integer not null, script_timeout_seconds integer not null, next_run_at timestamptz, updated_at timestamptz not null);
                 create table if not exists cron_runs(id uuid primary key, job_id text not null references cron_jobs(id) on delete cascade, scheduled_for timestamptz not null, status text not null, session_id uuid references sessions(id) on delete set null, started_at timestamptz not null, completed_at timestamptz, result_json jsonb not null);
                 create table if not exists drive_entries(id uuid primary key, path text not null unique, uri text not null unique, blob_uri text not null, content_hash text not null, mime text not null, size_bytes bigint not null, title text, doc_kind text, project text, source_uri text, provenance_json jsonb not null default '[]'::jsonb, summary text, status text not null, created_at timestamptz not null, updated_at timestamptz not null);
                 create table if not exists drive_attributes(entry_id uuid not null references drive_entries(id) on delete cascade, idx integer not null, key text not null, value text not null, confidence real not null, evidence_json jsonb, extractor text not null, source_uri text, session_id text, event_seq bigint, content_hash text, primary key(entry_id, idx));
                 create table if not exists drive_tags(entry_id uuid not null references drive_entries(id) on delete cascade, tag text not null, primary key(entry_id, tag));
                 create table if not exists drive_proposals(id uuid primary key, action text not null, entry_id uuid references drive_entries(id) on delete set null, source_path text not null, proposed_path text, proposed_tags jsonb not null default '[]'::jsonb, proposed_doc_kind text, proposed_project text, evidence_json jsonb not null default '[]'::jsonb, confidence real not null, policy_decision text not null, approval_id text, status text not null, source_run_id uuid not null, replay_metadata jsonb not null default '{}'::jsonb, created_at timestamptz not null, updated_at timestamptz not null);
                 create table if not exists drive_links(alias text primary key, canonical_root text not null, mode text not null, linked_uri text not null, memory_scope text not null, project text not null, status text not null, metadata_json jsonb not null default '{}'::jsonb, created_at timestamptz not null, updated_at timestamptz not null, revoked_at timestamptz);
                 create index if not exists profile_facts_subject_idx on profile_facts(subject);
                 create index if not exists recall_chunks_scope_created_idx on recall_chunks(scope, created_at desc);
                 create index if not exists recall_chunks_text_fts_idx on recall_chunks using gin (to_tsvector('simple', text));
                 create index if not exists project_items_project_kind_idx on project_items(project_id, kind, created_at desc);
                 create index if not exists project_items_source_session_kind_idx on project_items(source_session_id, kind, created_at desc);
                 create index if not exists session_events_actor_outputs_idx on session_events(session_id, seq) where artifact_uri is not null or history_uri is not null;
                 create index if not exists dream_queue_session_idx on dream_queue(session_id, enqueued_at asc);
                 create index if not exists dream_queue_scope_enqueued_idx on dream_queue(scope, enqueued_at desc);
                 create index if not exists dream_queue_ready_idx on dream_queue(status, available_at asc) where status = 'queued';
                 create index if not exists dream_queue_status_available_idx on dream_queue(status, available_at asc);
                 create index if not exists dream_queue_running_locked_idx on dream_queue(status, locked_at asc) where status = 'running';
                 create index if not exists memory_summaries_scope_updated_idx on memory_summaries(scope, updated_at desc);
                 create index if not exists memory_summaries_source_session_idx on memory_summaries(source_session_id, updated_at desc);
                 create index if not exists skill_proposals_source_session_idx on skill_proposals(source_session_id, updated_at desc);
                 create index if not exists skill_proposals_status_updated_idx on skill_proposals(status, updated_at desc);
                 create index if not exists cron_jobs_next_run_idx on cron_jobs(enabled, next_run_at);
                 create index if not exists cron_runs_job_started_idx on cron_runs(job_id, started_at desc);
                 create index if not exists drive_entries_hash_idx on drive_entries(content_hash);
                 create index if not exists drive_entries_project_idx on drive_entries(project, updated_at desc);
                 create index if not exists drive_entries_doc_kind_idx on drive_entries(doc_kind, updated_at desc);
                 create index if not exists drive_entries_updated_idx on drive_entries(updated_at desc);
                 create index if not exists drive_entries_search_fts_idx on drive_entries using gin (to_tsvector('simple', coalesce(title, '') || ' ' || coalesce(summary, '') || ' ' || path));
                 create index if not exists drive_attributes_key_value_idx on drive_attributes(key, value);
                 create index if not exists drive_tags_tag_idx on drive_tags(tag);
                 create index if not exists drive_proposals_status_updated_idx on drive_proposals(status, updated_at desc);
                 create index if not exists drive_links_status_updated_idx on drive_links(status, updated_at desc);
                 create index if not exists sessions_updated_at_idx on sessions(updated_at desc);",
            )
            .await
            .map_err(|err| ServerError::Store(err.to_string()))?;
        Ok(())
    }
}
