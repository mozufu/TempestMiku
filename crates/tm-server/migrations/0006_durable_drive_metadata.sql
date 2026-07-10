alter table drive_entries add column if not exists version bigint not null default 1;
alter table drive_entries add column if not exists entities_json jsonb not null default '[]'::jsonb;
alter table drive_entries add column if not exists dates_json jsonb not null default '[]'::jsonb;
alter table drive_entries add column if not exists amounts_json jsonb not null default '[]'::jsonb;
alter table drive_entries add column if not exists embedding text;
alter table drive_entries add column if not exists record_json jsonb;
alter table drive_entries add constraint drive_entries_version_positive check (version > 0);

alter table drive_proposals add column if not exists version bigint not null default 1;
alter table drive_proposals add column if not exists entry_id_snapshot uuid;
alter table drive_proposals add column if not exists record_json jsonb;
update drive_proposals set entry_id_snapshot = entry_id where entry_id_snapshot is null;
alter table drive_proposals add constraint drive_proposals_version_positive check (version > 0);

alter table drive_links add column if not exists version bigint not null default 1;
alter table drive_links add column if not exists record_json jsonb;
alter table drive_links add constraint drive_links_version_positive check (version > 0);

update drive_entries e
   set record_json = jsonb_build_object(
       'id', e.id, 'version', e.version, 'path', e.path, 'uri', e.uri,
       'blobUri', e.blob_uri, 'contentHash', e.content_hash, 'mime', e.mime,
       'sizeBytes', e.size_bytes, 'title', e.title, 'docKind', e.doc_kind,
       'project', e.project, 'entities', e.entities_json, 'dates', e.dates_json,
       'amounts', e.amounts_json,
       'tags', coalesce((select jsonb_agg(t.tag order by t.tag) from drive_tags t where t.entry_id=e.id), '[]'::jsonb),
       'embedding', e.embedding, 'sourceUri', e.source_uri,
       'provenance', e.provenance_json, 'createdAt', e.created_at,
       'updatedAt', e.updated_at, 'status', e.status,
       'attributes', coalesce((
           select jsonb_agg(jsonb_build_object(
               'key', a.key, 'value', a.value, 'confidence', a.confidence,
               'evidence', a.evidence_json, 'extractor', a.extractor,
               'sourceUri', a.source_uri, 'sessionId', a.session_id,
               'eventSeq', a.event_seq, 'contentHash', a.content_hash
           ) order by a.idx)
             from drive_attributes a where a.entry_id=e.id
       ), '[]'::jsonb),
       'summary', e.summary
   )
 where e.record_json is null;
alter table drive_entries alter column record_json set not null;

update drive_proposals p
   set record_json = jsonb_build_object(
       'id', p.id, 'version', p.version, 'action', p.action,
       'entryId', coalesce(p.entry_id_snapshot, p.entry_id, '00000000-0000-0000-0000-000000000000'::uuid),
       'sourcePath', p.source_path, 'proposedPath', p.proposed_path,
       'proposedTags', p.proposed_tags, 'proposedDocKind', p.proposed_doc_kind,
       'proposedProject', p.proposed_project, 'evidence', p.evidence_json,
       'confidence', p.confidence, 'policyDecision', p.policy_decision,
       'approvalId', p.approval_id, 'status', p.status,
       'sourceRunId', p.source_run_id, 'replayMetadata', p.replay_metadata,
       'createdAt', p.created_at, 'updatedAt', p.updated_at
   )
 where p.record_json is null;
alter table drive_proposals alter column record_json set not null;

update drive_links l
   set record_json = jsonb_build_object(
       'alias', l.alias, 'version', l.version, 'canonicalRoot', l.canonical_root,
       'mode', l.mode, 'linkedUri', l.linked_uri, 'memoryScope', l.memory_scope,
       'project', l.project, 'status', l.status, 'metadata', l.metadata_json,
       'createdAt', l.created_at, 'updatedAt', l.updated_at,
       'revokedAt', l.revoked_at
   )
 where l.record_json is null;
alter table drive_links alter column record_json set not null;

create table if not exists drive_organizer_runs(
    id uuid primary key,
    version bigint not null,
    trigger text not null,
    status text not null,
    attempts integer not null,
    proposal_ids jsonb not null default '[]'::jsonb,
    created_at timestamptz not null,
    available_at timestamptz not null,
    locked_at timestamptz,
    completed_at timestamptz,
    last_error text,
    record_json jsonb not null,
    constraint drive_organizer_runs_version_positive check (version > 0)
);

create table if not exists drive_corrections(
    id uuid primary key,
    version bigint not null,
    from_path text not null,
    to_path text not null,
    created_at timestamptz not null,
    record_json jsonb not null,
    constraint drive_corrections_version_positive check (version > 0)
);

create table if not exists drive_entry_tombstones(
    id uuid primary key,
    version bigint not null,
    path text not null,
    entry_json jsonb not null,
    deleted_at timestamptz not null,
    constraint drive_entry_tombstones_version_positive check (version > 0)
);

create index if not exists drive_organizer_runs_ready_idx
    on drive_organizer_runs(status, available_at asc);
create unique index if not exists drive_organizer_runs_single_active_idx
    on drive_organizer_runs((1))
    where status in ('queued', 'running');
create index if not exists drive_organizer_runs_locked_idx
    on drive_organizer_runs(status, locked_at asc)
    where status = 'running';
create index if not exists drive_corrections_created_idx
    on drive_corrections(created_at asc);
create index if not exists drive_entry_tombstones_path_idx
    on drive_entry_tombstones(path, deleted_at desc);
