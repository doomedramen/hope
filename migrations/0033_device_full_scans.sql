-- An explicit full TCP run may target one approved, current device address.
alter table scan_runs add column if not exists target_address inet;
alter table scan_runs add column if not exists target_device_id uuid references devices(id);
alter table scan_runs add constraint device_full_scan_target_check
    check ((target_address is null and target_device_id is null)
        or (target_address is not null and target_device_id is not null
            and kind = 'full_tcp' and targets_planned = 1));
create index if not exists scan_runs_target_device_idx
    on scan_runs (target_device_id, created_at desc)
    where target_device_id is not null;
