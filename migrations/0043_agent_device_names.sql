-- Existing installations already have agent ownership links from migration 0041.
-- Fill unnamed devices from those links without waiting for another snapshot.
-- Follow merge redirects without changing ownership, so undo remains possible.
with recursive linked_devices as (
    select a.id as agent_id, a.device_id, a.last_seen, a.created_at,
           coalesce(nullif(btrim(c.inventory #>> '{host,hostname}'), ''),
                    nullif(btrim(c.inventory #>> '{host,host_name}'), ''),
                    nullif(btrim(c.inventory ->> 'hostname'), ''),
                    nullif(btrim(c.inventory ->> 'host_name'), ''),
                    nullif(btrim(a.hostname), '')) as hostname
    from agents a
    left join agent_inventory_current c on c.agent_id = a.id
    where a.device_id is not null and a.revoked_at is null
    union
    select l.agent_id, d.canonical_of, l.last_seen, l.created_at, l.hostname
    from linked_devices l join devices d on d.id = l.device_id
    where d.canonical_of is not null
), names as (
    select distinct on (device_id) device_id, hostname
    from linked_devices
    where hostname is not null
    order by device_id, last_seen desc nulls last, created_at desc, agent_id
)
update devices d
set name = n.hostname, updated_at = now(), version = d.version + 1
from names n
where d.id = n.device_id and nullif(btrim(d.name), '') is null;

-- Do not guess which existing physical_host classifications came from an
-- operator. Future snapshots preserve device_type instead of overwriting it.
