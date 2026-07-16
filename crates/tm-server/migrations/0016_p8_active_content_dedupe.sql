-- P8 review hardening: inactive history must not prevent approved content from becoming active
-- again. Dedupe only the current active truth within one typed record kind and authority scope.
alter table memory_records
    drop constraint if exists memory_records_record_kind_owner_subject_memory_scope_content_key_key;
alter table memory_records
    drop constraint if exists memory_records_record_kind_owner_subject_memory_scope_conte_key;

create unique index if not exists memory_records_active_content_key_idx
    on memory_records(record_kind, owner_subject, memory_scope, content_key)
    where status = 'active' and effective_to is null;
