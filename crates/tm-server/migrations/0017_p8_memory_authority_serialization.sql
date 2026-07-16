-- P8 review hardening: serialize every durable memory write against project-scope revocation.
-- A guard row is the linearization point shared by ordinary writes, legacy mirror triggers, drive
-- unlink tombstones, and explicit revocation. TM001 is intentionally stable so the server can map
-- a database-enforced denial back to the same not-found authority boundary as reads.

create table if not exists memory_scope_authority_guards(
    owner_subject text not null check (btrim(owner_subject) <> ''),
    memory_scope text not null check (
        memory_scope = 'global'
        or (memory_scope like 'project:%' and btrim(substr(memory_scope, 9)) <> '')
    ),
    revoked_at timestamptz,
    primary key(owner_subject, memory_scope)
);

insert into memory_scope_authority_guards(owner_subject, memory_scope, revoked_at)
select owner_subject, memory_scope, null::timestamptz from memory_records
union
select owner_subject, memory_scope, null::timestamptz from memory_embedding_jobs
union
select owner_subject, memory_scope, null::timestamptz from memory_embedding_generations
union
select owner_subject, memory_scope, null::timestamptz from memory_embedding_active_versions
on conflict (owner_subject, memory_scope) do nothing;

insert into memory_scope_authority_guards(owner_subject, memory_scope, revoked_at)
select owner_subject, memory_scope, revoked_at from memory_scope_tombstones
on conflict (owner_subject, memory_scope) do update set
    revoked_at = case
        when memory_scope_authority_guards.revoked_at is null then excluded.revoked_at
        else least(memory_scope_authority_guards.revoked_at, excluded.revoked_at)
    end;

create or replace function tm_memory_lock_active_scope(
    requested_owner_subject text,
    requested_memory_scope text
) returns void language plpgsql as $$
declare
    scope_revoked_at timestamptz;
begin
    insert into memory_scope_authority_guards(owner_subject, memory_scope, revoked_at)
    values (requested_owner_subject, requested_memory_scope, null)
    on conflict (owner_subject, memory_scope) do nothing;

    select revoked_at into scope_revoked_at
      from memory_scope_authority_guards
     where owner_subject = requested_owner_subject
       and memory_scope = requested_memory_scope
     for update;

    if scope_revoked_at is not null then
        raise exception using
            errcode = 'TM001',
            message = format('memory scope %s/%s is revoked',
                requested_owner_subject, requested_memory_scope);
    end if;
end;
$$;

create or replace function tm_memory_guard_scoped_write()
returns trigger language plpgsql as $$
begin
    if tg_table_name = 'memory_embedding_jobs'
       and tg_op = 'UPDATE' and to_jsonb(new)->>'status' = 'cancelled' then
        return new;
    end if;
    perform tm_memory_lock_active_scope(new.owner_subject, new.memory_scope);
    return new;
end;
$$;

create or replace function tm_memory_guard_provenance_write()
returns trigger language plpgsql as $$
declare
    record_owner_subject text;
    record_memory_scope text;
begin
    select owner_subject, memory_scope
      into record_owner_subject, record_memory_scope
      from memory_records
     where record_kind = new.record_kind and id = new.record_id;
    if record_owner_subject is null then
        raise exception using errcode = '23503', message = 'memory provenance record is missing';
    end if;
    perform tm_memory_lock_active_scope(record_owner_subject, record_memory_scope);
    return new;
end;
$$;

create or replace function tm_memory_serialize_tombstone()
returns trigger language plpgsql as $$
begin
    insert into memory_scope_authority_guards(owner_subject, memory_scope, revoked_at)
    values (new.owner_subject, new.memory_scope, new.revoked_at)
    on conflict (owner_subject, memory_scope) do update set
        revoked_at = case
            when memory_scope_authority_guards.revoked_at is null then excluded.revoked_at
            else least(memory_scope_authority_guards.revoked_at, excluded.revoked_at)
        end;
    return new;
end;
$$;

create or replace function tm_memory_cancel_tombstoned_jobs()
returns trigger language plpgsql as $$
begin
    update memory_embedding_jobs
       set status = 'cancelled',
           cancelled_at = coalesce(cancelled_at, new.revoked_at),
           locked_at = null,
           lease_owner = null,
           updated_at = now()
     where owner_subject = new.owner_subject
       and memory_scope = new.memory_scope
       and status in ('queued', 'running');
    return new;
end;
$$;

drop trigger if exists tm_memory_records_authority_guard on memory_records;
create trigger tm_memory_records_authority_guard
before insert or update on memory_records
for each row execute function tm_memory_guard_scoped_write();

drop trigger if exists tm_memory_jobs_authority_guard on memory_embedding_jobs;
create trigger tm_memory_jobs_authority_guard
before insert or update on memory_embedding_jobs
for each row execute function tm_memory_guard_scoped_write();

drop trigger if exists tm_memory_generations_authority_guard on memory_embedding_generations;
create trigger tm_memory_generations_authority_guard
before insert or update on memory_embedding_generations
for each row execute function tm_memory_guard_scoped_write();

drop trigger if exists tm_memory_active_versions_authority_guard
    on memory_embedding_active_versions;
create trigger tm_memory_active_versions_authority_guard
before insert or update on memory_embedding_active_versions
for each row execute function tm_memory_guard_scoped_write();

drop trigger if exists tm_memory_provenance_authority_guard on memory_embedding_provenance;
create trigger tm_memory_provenance_authority_guard
before insert or update on memory_embedding_provenance
for each row execute function tm_memory_guard_provenance_write();

drop trigger if exists tm_memory_tombstone_serialize on memory_scope_tombstones;
create trigger tm_memory_tombstone_serialize
before insert or update on memory_scope_tombstones
for each row execute function tm_memory_serialize_tombstone();

drop trigger if exists tm_memory_tombstone_cancel_jobs on memory_scope_tombstones;
create trigger tm_memory_tombstone_cancel_jobs
after insert or update on memory_scope_tombstones
for each row execute function tm_memory_cancel_tombstoned_jobs();
