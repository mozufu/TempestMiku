create table if not exists environment_cognitions (
    id uuid primary key,
    owner_subject text not null,
    memory_scope text not null,
    title text not null,
    body text not null,
    source_policy_ids jsonb not null default '[]',
    confidence real not null default 0,
    version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (owner_subject, memory_scope)
);
