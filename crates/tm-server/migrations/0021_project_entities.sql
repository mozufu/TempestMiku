-- P11 (§30): promote "project" to a server-owned durable entity with lifecycle.
-- Memory-scope authority moves from the live linked-folder alias to this record; folder unlink no
-- longer revokes scope, and only project archive/delete tombstones it (reusing the 0017-0018
-- serialization spine). Folder attachment stays in drive_links; a link references a project by id.

create table if not exists projects(
    id text primary key check (btrim(id) <> ''),
    title text not null check (btrim(title) <> ''),
    status text not null default 'active' check (status in ('active', 'archived')),
    created_at timestamptz not null,
    updated_at timestamptz not null,
    archived_at timestamptz,
    check ((status = 'archived') = (archived_at is not null))
);

create index if not exists projects_status_idx on projects(status);

-- Backfill one active entity per existing project slug so P5/P8 continuity holds. drive_links.alias
-- and project_items.project_id are already canonical slugs; drive_links.project carries the display
-- title.
insert into projects(id, title, status, created_at, updated_at)
select alias, coalesce(nullif(btrim(max(project)), ''), alias), 'active', now(), now()
  from drive_links
 where btrim(alias) <> ''
 group by alias
on conflict (id) do nothing;

insert into projects(id, title, status, created_at, updated_at)
select project_id, project_id, 'active', now(), now()
  from project_items
 where btrim(project_id) <> ''
 group by project_id
on conflict (id) do nothing;
