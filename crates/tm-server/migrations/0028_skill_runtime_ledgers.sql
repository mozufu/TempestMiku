create table if not exists skill_runtime_exposures (
    turn_id uuid not null references session_turns(id) on delete cascade,
    name text not null,
    digest text not null,
    created_at timestamptz not null default now(),
    primary key (turn_id, name, digest)
);

create table if not exists skill_runtime_outcomes (
    episode_id uuid not null references evolution_episodes(id) on delete cascade,
    name text not null,
    digest text not null,
    pass boolean not null,
    created_at timestamptz not null default now(),
    primary key (episode_id, name, digest)
);
