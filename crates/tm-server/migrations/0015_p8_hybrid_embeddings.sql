-- P8.3: versioned local embedding generations. pgvector itself remains an explicit deployment
-- prerequisite: ordinary PostgreSQL development databases do not need the extension merely to
-- keep lexical recall alive. Once the extension is installed, the server creates one typed
-- vector(N) + HNSW table per staged embedding version and switches the active pointer only after
-- every snapshot record has a completed vector.

create table if not exists memory_embedding_generations(
    owner_subject text not null check (btrim(owner_subject) <> ''),
    memory_scope text not null check (
        memory_scope = 'global'
        or (memory_scope like 'project:%' and btrim(substr(memory_scope, 9)) <> '')
    ),
    embedding_version text not null check (btrim(embedding_version) <> ''),
    schema_version integer not null check (schema_version = 1),
    provider text not null check (provider in ('local', 'openai_compatible')),
    model_id text not null check (btrim(model_id) <> ''),
    dimensions integer not null check (dimensions > 0 and dimensions <= 2000),
    normalization text not null check (normalization in ('none', 'l2')),
    vector_table text not null check (vector_table ~ '^tm_memory_vec_[a-f0-9]{32}$'),
    expected_records integer not null check (expected_records >= 0),
    completed_records integer not null default 0 check (completed_records >= 0),
    status text not null check (status in ('staging', 'ready', 'failed')),
    created_at timestamptz not null,
    updated_at timestamptz not null,
    primary key(owner_subject, memory_scope, embedding_version)
);

create table if not exists memory_embedding_active_versions(
    owner_subject text not null,
    memory_scope text not null,
    embedding_version text not null,
    activated_at timestamptz not null,
    primary key(owner_subject, memory_scope),
    foreign key(owner_subject, memory_scope, embedding_version)
        references memory_embedding_generations(owner_subject, memory_scope, embedding_version)
        on delete restrict
);

alter table memory_embedding_jobs
    add column if not exists lease_owner uuid,
    add column if not exists lease_epoch integer not null default 0 check (lease_epoch >= 0);

create index if not exists memory_embedding_generations_scope_status_idx
    on memory_embedding_generations(owner_subject, memory_scope, status, created_at desc);
create index if not exists memory_embedding_jobs_generation_claim_idx
    on memory_embedding_jobs(owner_subject, memory_scope, embedding_version, status, available_at asc);
