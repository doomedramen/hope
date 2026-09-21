alter table agents add column public_key_hex text;
create unique index agents_public_key_hex_unique on agents (public_key_hex)
    where public_key_hex is not null;

create table agent_connection_challenges (
    nonce_hash text primary key,
    agent_id uuid not null references agents(id) on delete cascade,
    expires_at timestamptz not null,
    used_at timestamptz,
    created_at timestamptz not null default now()
);
create index agent_connection_challenges_expiry on agent_connection_challenges (expires_at);
