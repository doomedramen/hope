-- M4: align monitor defaults with spec §8.3 without rewriting migration 0015.

alter table monitors
    alter column interval_seconds set default 30,
    alter column failure_threshold set default 3;
