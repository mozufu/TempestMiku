alter table skill_proposals add column if not exists source_policy_id uuid;
alter table skill_proposals add column if not exists estimated_gain real;
alter table skill_proposals add column if not exists support_episodes integer not null default 0;

create table if not exists skill_runtime_stats (
    name text not null,
    digest text not null,
    exposures bigint not null default 0,
    passes bigint not null default 0,
    fails bigint not null default 0,
    last_selected_at timestamptz,
    last_outcome_at timestamptz,
    primary key (name, digest)
);
