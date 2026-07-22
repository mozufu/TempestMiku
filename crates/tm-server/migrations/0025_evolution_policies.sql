create table if not exists evolution_policies (
    id uuid primary key,
    owner_subject text not null,
    memory_scope text not null,
    signature text not null,
    trigger text not null,
    procedure text not null,
    verification text not null,
    boundary text not null,
    support_episode_ids jsonb not null default '[]',
    gain real not null default 0,
    status text not null default 'candidate'
        check (status in ('candidate', 'active', 'archived')),
    version integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (owner_subject, memory_scope, signature)
);

create index if not exists idx_evolution_policies_scope_status
    on evolution_policies(owner_subject, memory_scope, status, updated_at desc);

create table if not exists policy_trace_links (
    policy_id uuid not null references evolution_policies(id) on delete cascade,
    trace_id uuid not null,
    episode_id uuid not null,
    value real not null,
    positive boolean not null,
    created_at timestamptz not null default now(),
    primary key (policy_id, trace_id)
);

create index if not exists idx_policy_trace_links_episode
    on policy_trace_links(policy_id, episode_id);
