create table if not exists evolution_audits(
    id uuid primary key,
    audit_seq bigserial not null unique,
    idempotency_key text not null unique check (
        btrim(idempotency_key) <> '' and octet_length(idempotency_key) <= 256
    ),
    session_id uuid not null references sessions(id) on delete cascade,
    dream_id uuid,
    actor_id text not null check (btrim(actor_id) <> '' and octet_length(actor_id) <= 128),
    proposal_id uuid not null,
    target_class text not null check (target_class in (
        'profile_fact', 'scoped_memory', 'skill_proposal', 'persona_proposal', 'mode_proposal'
    )),
    target_id text not null check (btrim(target_id) <> '' and octet_length(target_id) <= 128),
    content_digest text not null check (content_digest ~ '^sha256:[0-9a-f]{64}$'),
    configured_tier text not null check (configured_tier in ('off', 'conservative', 'moderate')),
    decision_json jsonb not null,
    approval_id uuid references approval_requests(id) on delete set null,
    effect_id uuid references approval_effects(id) on delete set null,
    status text not null check (status in (
        'attempted', 'denied', 'awaiting_approval', 'approved', 'timed_out', 'superseded',
        'failed', 'applied'
    )),
    error_code text,
    record_json jsonb not null check (octet_length(record_json::text) <= 16384),
    created_at timestamptz not null
);

create index if not exists evolution_audits_session_created_idx
    on evolution_audits(session_id, audit_seq);
create index if not exists evolution_audits_target_idx
    on evolution_audits(target_class, target_id, created_at);
create index if not exists evolution_audits_approval_idx
    on evolution_audits(approval_id, created_at) where approval_id is not null;

-- Upgrade existing durable evolution effects into one bounded snapshot row. New lifecycle writes
-- append one row per transition; this backfill intentionally does not synthesize missing history.
insert into evolution_audits(
    id, idempotency_key, session_id, dream_id, actor_id, proposal_id, target_class,
    target_id, content_digest, configured_tier, decision_json, approval_id, effect_id,
    status, error_code, record_json, created_at
)
select effect.id,
       'approval:' || effect.approval_id::text || ':backfill',
       effect.session_id,
       nullif(effect.payload_json #>> '{evolution,origin,dreamId}', '')::uuid,
       effect.payload_json #>> '{evolution,origin,actorId}',
       (effect.payload_json #>> '{evolution,target,id}')::uuid,
       effect.payload_json #>> '{evolution,target,class}',
       effect.payload_json #>> '{evolution,target,id}',
       effect.payload_json #>> '{evolution,contentDigest}',
       effect.payload_json #>> '{evolution,configuredTier}',
       coalesce(
           effect.payload_json #> '{evolution,decision}',
           '{"outcome":"allowed"}'::jsonb
       ),
       effect.approval_id,
       effect.id,
       case
           when effect.status = 'applied' then 'applied'
           when effect.status = 'failed' then 'failed'
           when request.status = 'approved' then 'approved'
           when request.status = 'denied' then 'denied'
           when request.status = 'timed_out' then 'timed_out'
           when request.status = 'cancelled' then 'superseded'
           else 'awaiting_approval'
       end,
       null,
       jsonb_strip_nulls(jsonb_build_object(
           'version', coalesce((effect.payload_json #>> '{evolution,version}')::integer, 1),
           'id', effect.id,
           'proposalId', (effect.payload_json #>> '{evolution,target,id}')::uuid,
           'origin', effect.payload_json #> '{evolution,origin}',
           'target', effect.payload_json #> '{evolution,target}',
           'contentDigest', effect.payload_json #> '{evolution,contentDigest}',
           'configuredTier', effect.payload_json #> '{evolution,configuredTier}',
           'decision', coalesce(
               effect.payload_json #> '{evolution,decision}',
               '{"outcome":"allowed"}'::jsonb
           ),
           'approvalId', effect.approval_id,
           'effectId', effect.id,
           'status', case
               when effect.status = 'applied' then 'applied'
               when effect.status = 'failed' then 'failed'
               when request.status = 'approved' then 'approved'
               when request.status = 'denied' then 'denied'
               when request.status = 'timed_out' then 'timed_out'
               when request.status = 'cancelled' then 'superseded'
               else 'awaiting_approval'
           end,
           'createdAt', effect.created_at,
           'updatedAt', effect.updated_at
       )),
       effect.created_at
  from approval_effects effect
  join approval_requests request on request.id = effect.approval_id
 where effect.payload_json ? 'evolution'
on conflict (idempotency_key) do nothing;
