alter table projects add column if not exists default_memory_policy text;
update projects set default_memory_policy = 'project' where default_memory_policy is null;
alter table projects alter column default_memory_policy set default 'project';
alter table projects alter column default_memory_policy set not null;
alter table projects
    add constraint projects_default_memory_policy_check
    check (default_memory_policy in ('global', 'project'));
