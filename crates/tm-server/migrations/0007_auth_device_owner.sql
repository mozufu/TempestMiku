alter table auth_devices add column if not exists owner_subject text;

update auth_devices
   set owner_subject = authority.owner_subject
  from server_authority authority
 where authority.singleton = true
   and (auth_devices.owner_subject is null or btrim(auth_devices.owner_subject) = '');

alter table auth_devices alter column owner_subject set not null;
alter table auth_devices
    add constraint auth_devices_owner_subject_nonblank
    check (btrim(owner_subject) <> '');
alter table auth_devices
    add constraint auth_devices_owner_subject_fk
    foreign key (owner_subject) references server_authority(owner_subject) on update cascade;

create index if not exists auth_devices_owner_active_idx
    on auth_devices(owner_subject, revoked_at, last_seen_at desc);
