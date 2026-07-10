drop index if exists session_turns_running_started_idx;

create index if not exists session_turns_running_heartbeat_idx
    on session_turns(updated_at asc)
    where status = 'running';
