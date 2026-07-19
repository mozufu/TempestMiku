create table if not exists mcp_mutation_effects(
    effect_id text primary key check (effect_id ~ '^[0-9a-f]{64}$'),
    session_id uuid not null references sessions(id) on delete cascade,
    effect_scope_id uuid not null references session_turns(id) on delete cascade,
    actor_id text,
    catalog_generation bigint not null check (catalog_generation >= 0),
    catalog_digest text not null check (catalog_digest ~ '^[0-9a-f]{64}$'),
    server_alias text not null check (char_length(server_alias) between 1 and 48),
    tool_name text not null check (char_length(tool_name) between 1 and 128),
    target_digest text not null check (target_digest ~ '^[0-9a-f]{64}$'),
    request_digest text not null check (request_digest ~ '^[0-9a-f]{64}$'),
    request_bytes bigint not null check (request_bytes >= 0),
    status text not null check (status in ('started', 'succeeded', 'failed', 'uncertain')),
    result_digest text check (result_digest is null or result_digest ~ '^[0-9a-f]{64}$'),
    result_bytes bigint check (result_bytes is null or result_bytes >= 0),
    error_code text check (error_code is null or char_length(error_code) between 1 and 128),
    error_digest text check (error_digest is null or error_digest ~ '^[0-9a-f]{64}$'),
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create index if not exists mcp_mutation_effects_turn_idx
    on mcp_mutation_effects(session_id, effect_scope_id, created_at);
