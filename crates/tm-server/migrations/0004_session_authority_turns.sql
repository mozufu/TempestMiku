alter table sessions add column if not exists owner_subject text;
update sessions
   set owner_subject = 'owner'
 where owner_subject is null or btrim(owner_subject) = '';
alter table sessions alter column owner_subject set default 'owner';
alter table sessions alter column owner_subject set not null;
alter table sessions
    add constraint sessions_owner_subject_nonblank check (btrim(owner_subject) <> '');

alter table sessions add column if not exists memory_scope text;
update sessions
   set memory_scope = 'global'
 where memory_scope is null or btrim(memory_scope) = '';
alter table sessions alter column memory_scope set default 'global';
alter table sessions alter column memory_scope set not null;
alter table sessions
    add constraint sessions_memory_scope_nonblank check (btrim(memory_scope) <> '');

create table if not exists server_authority(
    singleton boolean primary key default true check (singleton),
    owner_subject text not null unique check (btrim(owner_subject) <> ''),
    updated_at timestamptz not null
);
insert into server_authority(singleton, owner_subject, updated_at)
values (true, 'owner', now())
on conflict (singleton) do nothing;
alter table sessions
    add constraint sessions_owner_subject_fk
    foreign key (owner_subject) references server_authority(owner_subject) on update cascade;

create table if not exists session_turns(
    id uuid primary key,
    session_id uuid not null references sessions(id) on delete cascade,
    client_message_id text not null check (char_length(client_message_id) between 1 and 128),
    content text not null,
    content_hash text not null check (content_hash ~ '^[0-9a-f]{64}$'),
    status text not null check (status in ('queued', 'running', 'completed', 'failed')),
    created_at timestamptz not null,
    updated_at timestamptz not null,
    started_at timestamptz,
    completed_at timestamptz,
    worker_id uuid,
    error text,
    unique(session_id, client_message_id)
);

alter table messages
    add column if not exists turn_id uuid references session_turns(id) on delete set null;
alter table session_events
    add column if not exists turn_id uuid references session_turns(id) on delete set null;

create unique index if not exists messages_turn_role_unique
    on messages(turn_id, role)
    where turn_id is not null and role in ('user', 'assistant');
create index if not exists session_turns_queue_idx
    on session_turns(status, created_at asc)
    where status = 'queued';
create index if not exists session_turns_session_status_idx
    on session_turns(session_id, status, created_at asc);
create index if not exists session_turns_running_started_idx
    on session_turns(status, started_at asc)
    where status = 'running';
create index if not exists messages_turn_idx
    on messages(turn_id)
    where turn_id is not null;
create index if not exists session_events_turn_idx
    on session_events(turn_id, seq)
    where turn_id is not null;

alter table dream_queue add column if not exists heartbeat_at timestamptz;
alter table dream_queue add column if not exists completed_at timestamptz;
alter table dream_queue add column if not exists error_at timestamptz;
update dream_queue
   set heartbeat_at = locked_at
 where status = 'running' and heartbeat_at is null;
update dream_queue
   set completed_at = available_at
 where status = 'completed' and completed_at is null;
update dream_queue
   set error_at = coalesce(locked_at, enqueued_at)
 where last_error is not null and error_at is null;
