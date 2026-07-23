-- §30.7: memory pools — a symmetric group of projects whose recall fan-out includes each other's
-- active scope. A pool carries no membership list of its own; each project points at its pool via
-- `pool_id` instead, and belongs to at most one active pool at a time (nullable column, not a join
-- table). Archiving a pool is a pure status flip: member `pool_id`s are left untouched, and fan-out
-- is recomputed at read time from the pool's status.

create table if not exists memory_pools(
    id text primary key check (btrim(id) <> ''),
    title text not null check (btrim(title) <> ''),
    status text not null default 'active' check (status in ('active', 'archived')),
    created_at timestamptz not null,
    updated_at timestamptz not null,
    archived_at timestamptz,
    check ((status = 'archived') = (archived_at is not null))
);

create index if not exists memory_pools_status_idx on memory_pools(status);

alter table projects add column if not exists pool_id text references memory_pools(id);

create index if not exists projects_pool_id_idx on projects(pool_id);
