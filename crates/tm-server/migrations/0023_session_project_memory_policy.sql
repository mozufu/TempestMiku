insert into projects(id, title, status, created_at, updated_at, default_memory_policy)
select project_id, project_id, 'active', now(), now(), 'project'
  from (
    select nullif(substring(memory_scope from '^project:(.+)$'), '') as project_id
      from sessions
  ) legacy
 where project_id is not null
on conflict (id) do nothing;

alter table sessions add column if not exists project_id text references projects(id);
alter table sessions add column if not exists memory_policy text;
update sessions
   set project_id = nullif(substring(memory_scope from '^project:(.+)$'), ''),
       memory_policy = case when memory_scope = 'global' then 'global' else 'project' end
 where memory_policy is null;
alter table sessions alter column memory_policy set default 'global';
alter table sessions alter column memory_policy set not null;
alter table sessions
    add constraint sessions_memory_policy_check check (memory_policy in ('global', 'project'));
alter table sessions
    add constraint sessions_memory_policy_project_id_check
    check (memory_policy <> 'project' or project_id is not null);
alter table sessions drop column memory_scope;
