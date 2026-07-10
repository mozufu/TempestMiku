create table if not exists auth_devices(
    id uuid primary key,
    name text not null,
    platform text not null,
    token_hash text not null unique,
    created_at timestamptz not null,
    last_seen_at timestamptz not null,
    revoked_at timestamptz
);

create table if not exists pairing_codes(
    id uuid primary key,
    code_hash text not null unique,
    created_at timestamptz not null,
    expires_at timestamptz not null,
    consumed_at timestamptz,
    created_by_device_id uuid references auth_devices(id) on delete set null
);

create index if not exists auth_devices_active_idx
    on auth_devices(revoked_at, last_seen_at desc);
create index if not exists pairing_codes_redeem_idx
    on pairing_codes(code_hash, expires_at)
    where consumed_at is null;
