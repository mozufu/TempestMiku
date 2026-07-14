create table push_deliveries(
    id uuid primary key,
    device_id uuid not null references auth_devices(id) on delete cascade,
    kind text not null check (kind in ('approval_requested', 'approval_resolved', 'session_ready')),
    session_id uuid not null references sessions(id) on delete cascade,
    approval_id uuid references approval_requests(id) on delete cascade,
    event_seq bigint,
    expires_at timestamptz not null,
    status text not null check (status in ('pending', 'claimed', 'delivered', 'failed')),
    attempts integer not null default 0 check (attempts >= 0),
    available_at timestamptz not null,
    locked_at timestamptz,
    lease_owner uuid,
    lease_epoch bigint not null default 0,
    delivered_at timestamptz,
    failed_at timestamptz,
    last_error text,
    created_at timestamptz not null,
    updated_at timestamptz not null,
    check (
        (kind in ('approval_requested', 'approval_resolved') and approval_id is not null and event_seq is null)
        or (kind = 'session_ready' and approval_id is null and event_seq is not null)
    )
);

insert into push_deliveries(
    id, device_id, kind, session_id, approval_id, event_seq, expires_at, status, attempts,
    available_at, locked_at, lease_owner, lease_epoch, delivered_at, failed_at, last_error,
    created_at, updated_at
)
select delivery.id, delivery.device_id, delivery.kind, approval.session_id,
       delivery.approval_id, null, approval.expires_at, delivery.status, delivery.attempts,
       delivery.available_at, delivery.locked_at, delivery.lease_owner, delivery.lease_epoch,
       delivery.delivered_at, delivery.failed_at, delivery.last_error,
       delivery.created_at, delivery.updated_at
  from approval_push_deliveries delivery
  join approval_requests approval on approval.id = delivery.approval_id
on conflict (id) do nothing;

drop table approval_push_deliveries;

create unique index push_deliveries_approval_unique
    on push_deliveries(approval_id, device_id, kind)
    where approval_id is not null;
create unique index push_deliveries_session_event_unique
    on push_deliveries(session_id, event_seq, device_id, kind)
    where event_seq is not null;
create index push_deliveries_ready_idx
    on push_deliveries(status, available_at asc)
    where status in ('pending', 'claimed');
create index push_deliveries_device_idx on push_deliveries(device_id, status);
