alter table dream_queue add column if not exists lease_owner uuid;
alter table dream_queue add column if not exists lease_epoch bigint not null default 0;

alter table cron_runs add column if not exists attempts integer not null default 0;
alter table cron_runs add column if not exists available_at timestamptz;
update cron_runs set available_at = started_at where available_at is null;
alter table cron_runs alter column available_at set not null;
alter table cron_runs add column if not exists locked_at timestamptz;
update cron_runs set locked_at = started_at where status = 'running' and locked_at is null;
alter table cron_runs add column if not exists lease_owner uuid;
alter table cron_runs add column if not exists lease_epoch bigint not null default 0;
alter table cron_runs add column if not exists last_error text;

do $$
begin
    if exists (
        select 1
          from cron_runs
         group by job_id, scheduled_for
        having count(*) > 1
    ) then
        raise exception 'cron_runs contains duplicate (job_id, scheduled_for) rows; deduplicate before applying migration 0003';
    end if;
end
$$;

create unique index if not exists cron_runs_job_scheduled_unique
    on cron_runs(job_id, scheduled_for);
create index if not exists cron_runs_ready_idx
    on cron_runs(status, available_at asc)
    where status = 'queued';
create index if not exists cron_runs_running_locked_idx
    on cron_runs(status, locked_at asc)
    where status = 'running';
