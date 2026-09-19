-- M4: preserve the non-stale health state across process restarts.

alter table monitors
    add column if not exists underlying_state text not null default 'unknown'
        check (underlying_state in ('unknown', 'up', 'degraded', 'down'));

update monitors
set underlying_state = case
    when state = 'stale' then 'unknown'
    else state
end
where underlying_state = 'unknown' and state <> 'unknown';
