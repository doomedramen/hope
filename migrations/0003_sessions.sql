-- Postgres-backed session store (replaces tower-sessions' in-memory
-- store, spec §12.3 "secure session cookies"). tower-sessions-sqlx-store
-- 0.15.0 depends on tower-sessions-core 0.14, incompatible with our
-- tower-sessions 0.15 (tower-sessions-core 0.15) -- see
-- apps/server/src/session_store.rs for the hand-rolled SessionStore impl.

create table if not exists sessions (
    id          text primary key,
    data        jsonb not null,
    expiry_date timestamptz not null
);

create index if not exists sessions_expiry_idx on sessions (expiry_date);
