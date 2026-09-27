-- Keep setup milestones independently of token cleanup and telemetry retention.
create table agent_enrollment_attempts (
    id uuid primary key default gen_random_uuid(),
    created_at timestamptz not null default now(),
    expires_at timestamptz not null,
    agent_id uuid unique references agents(id) on delete set null,
    authenticated_at timestamptz,
    inventory_at timestamptz,
    metrics_at timestamptz
);
alter table enrollment_tokens add column attempt_id uuid references agent_enrollment_attempts(id) on delete set null;
create index agent_enrollment_attempts_created on agent_enrollment_attempts(created_at);
