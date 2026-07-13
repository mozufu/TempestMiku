create table if not exists evolution_review_proposals(
    id uuid primary key,
    session_id uuid not null references sessions(id) on delete cascade,
    target_class text not null check (target_class in ('persona_proposal', 'mode_proposal')),
    target_id text not null check (char_length(target_id) between 1 and 128),
    status text not null check (status in ('pending', 'approved', 'denied', 'timed_out', 'cancelled')),
    base_version bigint not null check (base_version > 0),
    base_digest text not null check (char_length(base_digest) between 1 and 128),
    content_digest text not null check (char_length(content_digest) between 1 and 128),
    record_json jsonb not null check (octet_length(record_json::text) <= 16384),
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create index if not exists evolution_review_proposals_session_created_idx
    on evolution_review_proposals(session_id, created_at, id);
