create table if not exists egress_mutation_effects(
    effect_id text primary key check (effect_id ~ '^[0-9a-f]{64}$'),
    session_id uuid not null references sessions(id) on delete cascade,
    effect_scope_id uuid not null references session_turns(id) on delete cascade,
    session_digest text not null check (session_digest ~ '^[0-9a-f]{64}$'),
    actor_digest text not null check (actor_digest ~ '^[0-9a-f]{64}$'),
    destination_id text not null check (char_length(destination_id) between 1 and 128),
    destination_version bigint not null check (destination_version > 0),
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

create index if not exists egress_mutation_effects_turn_idx
    on egress_mutation_effects(session_id, effect_scope_id, created_at);

create table if not exists egress_session_usage(
    session_id uuid primary key references sessions(id) on delete cascade,
    requests bigint not null default 0 check (requests >= 0),
    request_bytes bigint not null default 0 check (request_bytes >= 0),
    response_bytes bigint not null default 0 check (response_bytes >= 0),
    response_reserved bigint not null default 0 check (response_reserved >= 0),
    time_ms bigint not null default 0 check (time_ms >= 0),
    time_reserved_ms bigint not null default 0 check (time_reserved_ms >= 0),
    updated_at timestamptz not null
);

create table if not exists egress_destination_usage(
    session_id uuid not null references sessions(id) on delete cascade,
    destination_id text not null check (char_length(destination_id) between 1 and 128),
    requests bigint not null default 0 check (requests >= 0),
    request_bytes bigint not null default 0 check (request_bytes >= 0),
    response_bytes bigint not null default 0 check (response_bytes >= 0),
    response_reserved bigint not null default 0 check (response_reserved >= 0),
    time_ms bigint not null default 0 check (time_ms >= 0),
    time_reserved_ms bigint not null default 0 check (time_reserved_ms >= 0),
    updated_at timestamptz not null,
    primary key(session_id, destination_id)
);

create table if not exists egress_budget_reservations(
    reservation_id text primary key check (reservation_id ~ '^[0-9a-f]{32}$'),
    session_id uuid not null references sessions(id) on delete cascade,
    destination_id text not null check (char_length(destination_id) between 1 and 128),
    request_bytes bigint not null check (request_bytes >= 0),
    response_reserved bigint not null check (response_reserved >= 0),
    time_reserved_ms bigint not null check (time_reserved_ms >= 0),
    settled boolean not null default false,
    response_bytes bigint check (response_bytes is null or response_bytes >= 0),
    elapsed_ms bigint check (elapsed_ms is null or elapsed_ms >= 0),
    created_at timestamptz not null,
    settled_at timestamptz
);

create index if not exists egress_budget_reservations_session_idx
    on egress_budget_reservations(session_id, settled, created_at);
