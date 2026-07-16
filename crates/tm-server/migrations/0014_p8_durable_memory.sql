-- P8.2: versioned, scoped memory persistence. This migration deliberately does not create the
-- pgvector extension or a vector index: dense retrieval remains a P8.3 concern and must degrade
-- visibly when the extension is absent.

create table if not exists memory_records(
    record_kind text not null check (record_kind in ('episodic', 'semantic')),
    id uuid not null,
    schema_version integer not null check (schema_version = 1),
    owner_subject text not null check (btrim(owner_subject) <> ''),
    memory_scope text not null check (
        memory_scope = 'global'
        or (memory_scope like 'project:%' and btrim(substr(memory_scope, 9)) <> '')
    ),
    text text,
    semantic_subject text,
    predicate text,
    object text,
    evidence_json jsonb not null check (jsonb_typeof(evidence_json) = 'array'),
    confidence real not null check (confidence >= 0 and confidence <= 1),
    importance real not null check (importance >= 0 and importance <= 1),
    observed_at timestamptz not null,
    effective_from timestamptz not null,
    effective_to timestamptz,
    status text not null check (status in (
        'candidate', 'active', 'withheld', 'unsupported', 'corrected', 'superseded'
    )),
    corrects_record_id uuid,
    corrected_by_record_id uuid,
    supersedes_record_id uuid,
    superseded_by_record_id uuid,
    content_key text not null check (btrim(content_key) <> ''),
    version_key text not null check (btrim(version_key) <> ''),
    created_at timestamptz not null,
    primary key(record_kind, id),
    unique(record_kind, owner_subject, memory_scope, content_key),
    unique(version_key),
    check (effective_to is null or effective_to >= effective_from),
    check (
        (record_kind = 'episodic'
         and btrim(coalesce(text, '')) <> ''
         and semantic_subject is null and predicate is null and object is null)
        or
        (record_kind = 'semantic'
         and text is null
         and btrim(coalesce(semantic_subject, '')) <> ''
         and btrim(coalesce(predicate, '')) <> ''
         and btrim(coalesce(object, '')) <> '')
    )
);

create table if not exists memory_record_evidence(
    record_kind text not null,
    record_id uuid not null,
    ordinal integer not null check (ordinal >= 0),
    evidence_json jsonb not null,
    primary key(record_kind, record_id, ordinal),
    foreign key(record_kind, record_id)
        references memory_records(record_kind, id) on delete cascade
);

create table if not exists memory_record_relations(
    record_kind text not null,
    record_id uuid not null,
    relation text not null check (relation in (
        'corrects', 'corrected_by', 'supersedes', 'superseded_by'
    )),
    linked_record_id uuid not null,
    created_at timestamptz not null default now(),
    primary key(record_kind, record_id, relation),
    foreign key(record_kind, record_id)
        references memory_records(record_kind, id) on delete cascade
);

create table if not exists memory_embedding_provenance(
    record_kind text not null,
    record_id uuid not null,
    embedding_version text not null check (btrim(embedding_version) <> ''),
    schema_version integer not null check (schema_version = 1),
    provider text not null check (provider in ('local', 'openai_compatible')),
    model_id text not null check (btrim(model_id) <> ''),
    dimensions integer not null check (dimensions > 0),
    normalization text not null check (normalization in ('none', 'l2')),
    content_hash text not null check (content_hash ~ '^[[:xdigit:]]{64}$'),
    reembedding_state text not null check (reembedding_state in (
        'pending', 'ready', 'failed', 'superseded'
    )),
    created_at timestamptz not null,
    primary key(record_kind, record_id, embedding_version),
    foreign key(record_kind, record_id)
        references memory_records(record_kind, id) on delete cascade
);

create table if not exists memory_embedding_jobs(
    id uuid primary key,
    record_kind text not null,
    record_id uuid not null,
    owner_subject text not null check (btrim(owner_subject) <> ''),
    memory_scope text not null check (
        memory_scope = 'global'
        or (memory_scope like 'project:%' and btrim(substr(memory_scope, 9)) <> '')
    ),
    content_key text not null check (btrim(content_key) <> ''),
    embedding_version text not null check (btrim(embedding_version) <> ''),
    reembedding_key text not null unique check (btrim(reembedding_key) <> ''),
    status text not null check (status in ('queued', 'running', 'completed', 'failed', 'cancelled')),
    attempts integer not null default 0 check (attempts >= 0),
    available_at timestamptz not null,
    locked_at timestamptz,
    cancelled_at timestamptz,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    foreign key(record_kind, record_id)
        references memory_records(record_kind, id) on delete cascade,
    foreign key(record_kind, record_id, embedding_version)
        references memory_embedding_provenance(record_kind, record_id, embedding_version)
        on delete cascade
);

create table if not exists memory_scope_tombstones(
    owner_subject text not null check (btrim(owner_subject) <> ''),
    memory_scope text not null check (
        memory_scope like 'project:%' and btrim(substr(memory_scope, 9)) <> ''
    ),
    link_alias text,
    reason text not null check (btrim(reason) <> ''),
    revoked_at timestamptz not null,
    primary key(owner_subject, memory_scope)
);

create table if not exists memory_legacy_migration_quarantine(
    source_kind text not null,
    source_id uuid not null,
    reason text not null,
    captured_at timestamptz not null default now(),
    primary key(source_kind, source_id)
);

create index if not exists memory_records_authority_active_idx
    on memory_records(owner_subject, memory_scope, importance desc, observed_at desc, id)
    where status = 'active' and effective_to is null;
create index if not exists memory_records_fts_idx
    on memory_records using gin (
        to_tsvector('simple', coalesce(text, '') || ' ' || coalesce(semantic_subject, '') || ' '
            || coalesce(predicate, '') || ' ' || coalesce(object, ''))
    );
create index if not exists memory_record_relations_linked_idx
    on memory_record_relations(linked_record_id, relation);
create index if not exists memory_embedding_jobs_scope_ready_idx
    on memory_embedding_jobs(owner_subject, memory_scope, status, available_at asc);
create index if not exists memory_embedding_jobs_record_idx
    on memory_embedding_jobs(record_kind, record_id, created_at desc);

create or replace function tm_memory_sync_record_evidence()
returns trigger language plpgsql as $$
begin
    delete from memory_record_evidence
     where record_kind = new.record_kind and record_id = new.id;
    insert into memory_record_evidence(record_kind, record_id, ordinal, evidence_json)
    select new.record_kind, new.id, (item.ordinality - 1)::integer, item.value
      from jsonb_array_elements(new.evidence_json) with ordinality as item(value, ordinality);
    return new;
end;
$$;

drop trigger if exists tm_memory_records_evidence_sync on memory_records;
create trigger tm_memory_records_evidence_sync
after insert or update of evidence_json on memory_records
for each row execute function tm_memory_sync_record_evidence();

create or replace function tm_memory_sync_record_relations()
returns trigger language plpgsql as $$
begin
    delete from memory_record_relations
     where record_kind = new.record_kind and record_id = new.id;
    insert into memory_record_relations(record_kind, record_id, relation, linked_record_id)
    select new.record_kind, new.id, relation, linked_record_id
      from (values
          ('corrects'::text, new.corrects_record_id),
          ('corrected_by'::text, new.corrected_by_record_id),
          ('supersedes'::text, new.supersedes_record_id),
          ('superseded_by'::text, new.superseded_by_record_id)
      ) as links(relation, linked_record_id)
     where linked_record_id is not null;
    return new;
end;
$$;

drop trigger if exists tm_memory_records_relations_sync on memory_records;
create trigger tm_memory_records_relations_sync
after insert or update of corrects_record_id, corrected_by_record_id,
    supersedes_record_id, superseded_by_record_id on memory_records
for each row execute function tm_memory_sync_record_relations();

create or replace function tm_memory_sync_legacy_profile_fact()
returns trigger language plpgsql as $$
declare
    digest text;
begin
    if btrim(new.subject) = '' or btrim(new.predicate) = '' or btrim(new.object) = ''
       or new.confidence < 0 or new.confidence > 1
       or new.importance < 0 or new.importance > 1 then
        insert into memory_legacy_migration_quarantine(source_kind, source_id, reason)
        values ('profile_fact', new.id, 'legacy profile fact violates P8.2 record constraints')
        on conflict (source_kind, source_id) do update
            set reason = excluded.reason, captured_at = now();
        return new;
    end if;
    digest := md5(concat_ws(E'\n', new.id::text, new.subject, new.predicate, new.object,
        new.confidence::text, new.importance::text, new.provenance,
        new.valid_from::text, coalesce(new.valid_to::text, '')));
    insert into memory_records(
        record_kind, id, schema_version, owner_subject, memory_scope, text,
        semantic_subject, predicate, object, evidence_json, confidence, importance,
        observed_at, effective_from, effective_to, status, content_key, version_key, created_at
    ) values (
        'semantic', new.id, 1, new.subject, 'global', null,
        new.subject, new.predicate, new.object,
        jsonb_build_array(jsonb_build_object(
            'schemaVersion', 1,
            'label', 'legacy profile fact',
            'source', jsonb_build_object('kind', 'resource',
                'uri', format('memory://legacy/profile/%s', new.id))
        )),
        new.confidence, new.importance, new.valid_from, new.valid_from, new.valid_to,
        case when new.valid_to is null then 'active' else 'superseded' end,
        'legacy-memory-content-v1:' || digest,
        format('legacy-memory-record-v1:semantic:%s:%s', new.id, digest),
        new.valid_from
    ) on conflict (record_kind, id) do update set
        schema_version = excluded.schema_version,
        owner_subject = excluded.owner_subject,
        memory_scope = excluded.memory_scope,
        text = excluded.text,
        semantic_subject = excluded.semantic_subject,
        predicate = excluded.predicate,
        object = excluded.object,
        evidence_json = excluded.evidence_json,
        confidence = excluded.confidence,
        importance = excluded.importance,
        observed_at = excluded.observed_at,
        effective_from = excluded.effective_from,
        effective_to = excluded.effective_to,
        status = excluded.status,
        content_key = excluded.content_key,
        version_key = excluded.version_key;
    delete from memory_legacy_migration_quarantine
     where source_kind = 'profile_fact' and source_id = new.id;
    return new;
end;
$$;

create or replace function tm_memory_sync_legacy_recall_chunk()
returns trigger language plpgsql as $$
declare
    authority_subject text;
    digest text;
begin
    select owner_subject into authority_subject
      from server_authority where singleton = true;
    if authority_subject is null or btrim(new.scope) = ''
       or (new.scope <> 'global' and (
           new.scope not like 'project:%' or btrim(substr(new.scope, 9)) = ''
       ))
       or btrim(new.text) = '' or new.importance < 0 or new.importance > 1 then
        insert into memory_legacy_migration_quarantine(source_kind, source_id, reason)
        values ('recall_chunk', new.id, 'legacy recall chunk lacks P8.2 scope, owner, or score')
        on conflict (source_kind, source_id) do update
            set reason = excluded.reason, captured_at = now();
        return new;
    end if;
    digest := md5(concat_ws(E'\n', new.id::text, authority_subject, new.scope, new.text,
        new.source, new.importance::text, new.created_at::text));
    insert into memory_records(
        record_kind, id, schema_version, owner_subject, memory_scope, text,
        semantic_subject, predicate, object, evidence_json, confidence, importance,
        observed_at, effective_from, effective_to, status, content_key, version_key, created_at
    ) values (
        'episodic', new.id, 1, authority_subject, new.scope, new.text,
        null, null, null,
        jsonb_build_array(jsonb_build_object(
            'schemaVersion', 1,
            'label', 'legacy recall chunk',
            'source', jsonb_build_object('kind', 'resource',
                'uri', format('memory://legacy/recall/%s', new.id))
        )),
        1.0, new.importance, new.created_at, new.created_at, null, 'active',
        'legacy-memory-content-v1:' || digest,
        format('legacy-memory-record-v1:episodic:%s:%s', new.id, digest),
        new.created_at
    ) on conflict (record_kind, id) do update set
        schema_version = excluded.schema_version,
        owner_subject = excluded.owner_subject,
        memory_scope = excluded.memory_scope,
        text = excluded.text,
        semantic_subject = excluded.semantic_subject,
        predicate = excluded.predicate,
        object = excluded.object,
        evidence_json = excluded.evidence_json,
        confidence = excluded.confidence,
        importance = excluded.importance,
        observed_at = excluded.observed_at,
        effective_from = excluded.effective_from,
        effective_to = excluded.effective_to,
        status = excluded.status,
        content_key = excluded.content_key,
        version_key = excluded.version_key;
    delete from memory_legacy_migration_quarantine
     where source_kind = 'recall_chunk' and source_id = new.id;
    return new;
end;
$$;

create or replace function tm_memory_sync_legacy_summary()
returns trigger language plpgsql as $$
declare
    digest text;
begin
    if btrim(new.subject) = '' or btrim(new.scope) = ''
       or (new.scope <> 'global' and (
           new.scope not like 'project:%' or btrim(substr(new.scope, 9)) = ''
       ))
       or btrim(new.title) = '' or btrim(new.body) = '' then
        insert into memory_legacy_migration_quarantine(source_kind, source_id, reason)
        values ('memory_summary', new.id, 'legacy memory summary violates P8.2 record constraints')
        on conflict (source_kind, source_id) do update
            set reason = excluded.reason, captured_at = now();
        return new;
    end if;
    digest := md5(concat_ws(E'\n', new.id::text, new.subject, new.scope, new.title, new.body,
        new.updated_at::text));
    insert into memory_records(
        record_kind, id, schema_version, owner_subject, memory_scope, text,
        semantic_subject, predicate, object, evidence_json, confidence, importance,
        observed_at, effective_from, effective_to, status, content_key, version_key, created_at
    ) values (
        'episodic', new.id, 1, new.subject, new.scope, new.title || E'\n' || new.body,
        null, null, null,
        jsonb_build_array(jsonb_build_object(
            'schemaVersion', 1,
            'label', 'legacy memory summary',
            'source', jsonb_build_object('kind', 'resource',
                'uri', format('memory://summaries/%s', new.id))
        )),
        1.0, 0.5, new.updated_at, new.created_at, null, 'active',
        'legacy-memory-content-v1:' || digest,
        format('legacy-memory-record-v1:episodic:%s:%s', new.id, digest),
        new.created_at
    ) on conflict (record_kind, id) do update set
        schema_version = excluded.schema_version,
        owner_subject = excluded.owner_subject,
        memory_scope = excluded.memory_scope,
        text = excluded.text,
        semantic_subject = excluded.semantic_subject,
        predicate = excluded.predicate,
        object = excluded.object,
        evidence_json = excluded.evidence_json,
        confidence = excluded.confidence,
        importance = excluded.importance,
        observed_at = excluded.observed_at,
        effective_from = excluded.effective_from,
        effective_to = excluded.effective_to,
        status = excluded.status,
        content_key = excluded.content_key,
        version_key = excluded.version_key;
    delete from memory_legacy_migration_quarantine
     where source_kind = 'memory_summary' and source_id = new.id;
    return new;
end;
$$;

drop trigger if exists tm_memory_legacy_profile_fact_sync on profile_facts;
create trigger tm_memory_legacy_profile_fact_sync
after insert or update on profile_facts
for each row execute function tm_memory_sync_legacy_profile_fact();

drop trigger if exists tm_memory_legacy_recall_chunk_sync on recall_chunks;
create trigger tm_memory_legacy_recall_chunk_sync
after insert or update on recall_chunks
for each row execute function tm_memory_sync_legacy_recall_chunk();

drop trigger if exists tm_memory_legacy_summary_sync on memory_summaries;
create trigger tm_memory_legacy_summary_sync
after insert or update on memory_summaries
for each row execute function tm_memory_sync_legacy_summary();

-- Replay existing legacy rows through the same trigger paths. The legacy tables and lexical query
-- shape remain untouched, so P8.1 output stays the control group until P8.3 explicitly switches
-- candidate generation.
update profile_facts set id = id;
update recall_chunks set id = id;
update memory_summaries set id = id;

create or replace function tm_memory_tombstone_revoked_drive_scope()
returns trigger language plpgsql as $$
declare
    authority_subject text;
    revoked_at_value timestamptz;
begin
    if new.memory_scope not like 'project:%'
       or btrim(substr(new.memory_scope, 9)) = ''
       or (new.status = 'active' and new.revoked_at is null) then
        return new;
    end if;
    select owner_subject into authority_subject
      from server_authority where singleton = true;
    if authority_subject is null then
        return new;
    end if;
    revoked_at_value := coalesce(new.revoked_at, new.updated_at, now());
    insert into memory_scope_tombstones(
        owner_subject, memory_scope, link_alias, reason, revoked_at
    ) values (
        authority_subject, new.memory_scope, new.alias,
        'linked project scope was revoked or invalidated', revoked_at_value
    ) on conflict (owner_subject, memory_scope) do update set
        link_alias = excluded.link_alias,
        reason = excluded.reason,
        revoked_at = least(memory_scope_tombstones.revoked_at, excluded.revoked_at);
    update memory_embedding_jobs
       set status = 'cancelled',
           cancelled_at = coalesce(cancelled_at, revoked_at_value),
           locked_at = null,
           updated_at = now()
     where owner_subject = authority_subject
       and memory_scope = new.memory_scope
       and status in ('queued', 'running');
    return new;
end;
$$;

drop trigger if exists tm_memory_drive_link_tombstone on drive_links;
create trigger tm_memory_drive_link_tombstone
after insert or update of status, revoked_at on drive_links
for each row execute function tm_memory_tombstone_revoked_drive_scope();

insert into memory_scope_tombstones(owner_subject, memory_scope, link_alias, reason, revoked_at)
select authority.owner_subject,
       link.memory_scope,
       link.alias,
       'linked project scope was revoked or invalidated',
       coalesce(link.revoked_at, link.updated_at, now())
  from drive_links link
 cross join server_authority authority
 where link.memory_scope like 'project:%'
   and btrim(substr(link.memory_scope, 9)) <> ''
   and (link.status <> 'active' or link.revoked_at is not null)
on conflict (owner_subject, memory_scope) do update set
    link_alias = excluded.link_alias,
    reason = excluded.reason,
    revoked_at = least(memory_scope_tombstones.revoked_at, excluded.revoked_at);
