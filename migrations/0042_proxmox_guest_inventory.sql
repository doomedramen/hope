-- Guest inventory keeps the existing workloads.host_device_id foreign key.
-- Missing guests remain historical; failed/partial reports cannot remove them.
alter table workloads
    add column last_seen timestamptz,
    add column is_current boolean not null default true;
