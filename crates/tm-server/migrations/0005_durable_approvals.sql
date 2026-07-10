create table if not exists approval_requests(
    id uuid primary key,
    session_id uuid not null references sessions(id) on delete cascade,
    turn_id uuid references session_turns(id) on delete set null,
    requester_id uuid not null,
    origin text not null check (btrim(origin) <> ''),
    action text not null check (btrim(action) <> ''),
    scope_json jsonb not null,
    options_json jsonb not null check (jsonb_typeof(options_json) = 'array'),
    status text not null check (status in ('pending', 'approved', 'denied', 'timed_out', 'cancelled')),
    resumable boolean not null default false,
    created_at timestamptz not null,
    expires_at timestamptz not null,
    heartbeat_at timestamptz not null,
    resolved_at timestamptz,
    selected_option_id text,
    resolution_json jsonb,
    request_event_seq bigint,
    resolution_event_seq bigint,
    resolution_version bigint not null default 0,
    check (expires_at > created_at),
    check (
        (status = 'pending' and resolved_at is null)
        or (status <> 'pending' and resolved_at is not null)
    )
);

create table if not exists approval_effects(
    id uuid primary key,
    approval_id uuid not null unique references approval_requests(id) on delete cascade,
    session_id uuid not null references sessions(id) on delete cascade,
    effect_type text not null check (btrim(effect_type) <> ''),
    payload_json jsonb not null,
    status text not null check (status in ('blocked', 'pending', 'claimed', 'applied', 'failed')),
    attempts integer not null default 0 check (attempts >= 0),
    available_at timestamptz not null,
    locked_at timestamptz,
    lease_owner uuid,
    lease_epoch bigint not null default 0,
    applied_at timestamptz,
    error_at timestamptz,
    last_error text,
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create index if not exists approval_requests_session_status_idx
    on approval_requests(session_id, status, created_at desc);
create index if not exists approval_requests_expiry_idx
    on approval_requests(status, expires_at asc)
    where status = 'pending';
create index if not exists approval_requests_stale_nonresumable_idx
    on approval_requests(status, resumable, heartbeat_at asc)
    where status = 'pending' and resumable = false;
create index if not exists approval_effects_ready_idx
    on approval_effects(status, available_at asc)
    where status = 'pending';
create index if not exists approval_effects_claimed_idx
    on approval_effects(status, locked_at asc)
    where status = 'claimed';
