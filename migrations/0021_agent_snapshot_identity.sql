-- Preserve the agent-generated snapshot identity so retransmission with a new
-- transport envelope remains idempotent across reconnects.

alter table agent_inventory_snapshots
    add column if not exists source_snapshot_id uuid;

update agent_inventory_snapshots
   set source_snapshot_id = message_id
 where source_snapshot_id is null;

alter table agent_inventory_snapshots
    alter column source_snapshot_id set not null;

create unique index if not exists agent_inventory_snapshots_source_unique
    on agent_inventory_snapshots (agent_id, source_snapshot_id);

alter table agent_inventory_current
    add column if not exists source_snapshot_id uuid;

update agent_inventory_current c
   set source_snapshot_id = s.source_snapshot_id
  from agent_inventory_snapshots s
 where c.snapshot_id = s.id
   and c.source_snapshot_id is null;

alter table agent_inventory_current
    alter column source_snapshot_id set not null;
