create table if not exists device_push_registrations(
    device_id uuid primary key references auth_devices(id) on delete cascade,
    provider text not null check (btrim(provider) <> ''),
    secret_ciphertext bytea not null,
    secret_nonce bytea not null check (octet_length(secret_nonce) = 24),
    secret_version smallint not null default 1 check (secret_version = 1),
    created_at timestamptz not null,
    updated_at timestamptz not null,
    disabled_at timestamptz,
    last_error text
);

create table if not exists approval_push_deliveries(
    id uuid primary key,
    approval_id uuid not null references approval_requests(id) on delete cascade,
    device_id uuid not null references auth_devices(id) on delete cascade,
    kind text not null check (kind in ('approval_requested', 'approval_resolved')),
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
    unique(approval_id, device_id, kind)
);

create index if not exists approval_push_deliveries_ready_idx
    on approval_push_deliveries(status, available_at asc)
    where status in ('pending', 'claimed');
create index if not exists approval_push_deliveries_device_idx
    on approval_push_deliveries(device_id, status);
