-- Agent ownership is an explicit relationship, not a read-time identity guess.
-- Keep the original member when devices merge so undo-merge preserves ownership.
alter table agents add column device_id uuid references devices(id);
create index agents_device_id_idx on agents(device_id);

-- Repeated rules for the same device are harmless. Conflicting device owners
-- require review and must not be resolved by arbitrary row order.
with known_links as (
    select a.id, min(ir.device_id::text)::uuid as device_id
    from agents a
    join identity_rules ir on ir.rule_type = 'agent_id' and ir.value = a.id::text
    group by a.id
    having count(distinct ir.device_id) = 1
)
update agents a set device_id = k.device_id from known_links k where a.id = k.id;
