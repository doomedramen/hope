-- Keep the operator workflow single-flight per network. A network may have
-- many historical runs, but only one pending/running run at a time.
create unique index if not exists scan_runs_one_active_network_idx
    on scan_runs (network_id)
    where status in ('pending', 'running') and not complete;
