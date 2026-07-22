create table if not exists evolution_episodes (
    id uuid primary key,
    session_id uuid not null,
    turn_id uuid not null unique,
    owner_subject text not null,
    memory_scope text not null,
    status text not null default 'captured'
        check (status in ('captured', 'valued', 'evolved', 'failed')),
    terminal_reward real,
    reward_source text check (reward_source in ('explicit', 'runtime')),
    feedback_outcome text check (feedback_outcome in ('accepted', 'corrected', 'rejected')),
    trace_count integer not null default 0,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index if not exists idx_evolution_episodes_scope
    on evolution_episodes(owner_subject, memory_scope, updated_at desc);

create table if not exists experience_traces (
    id uuid primary key,
    episode_id uuid not null references evolution_episodes(id) on delete cascade,
    ordinal integer not null,
    kind text not null check (kind in ('cell', 'effect', 'terminal')),
    capability text,
    action_summary text not null,
    observation_summary text not null,
    error_signature text,
    value real,
    event_seq bigint not null,
    result_event_seq bigint,
    created_at timestamptz not null default now(),
    unique (episode_id, ordinal)
);

create table if not exists turn_feedback (
    turn_id uuid primary key,
    session_id uuid not null,
    outcome text not null check (outcome in ('accepted', 'corrected', 'rejected')),
    comment text,
    created_at timestamptz not null default now()
);
