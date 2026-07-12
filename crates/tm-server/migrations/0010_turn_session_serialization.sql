create unique index if not exists session_turns_one_running_per_session_idx
    on session_turns(session_id)
    where status = 'running';
