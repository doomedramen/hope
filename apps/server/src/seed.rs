//! `server seed` — demo inventory topology (design §9 slice 11) for
//! trying out the M1 UI/API without wiring up real agents/scans: one
//! network, a physical host with an interface/address, a VM workload on
//! that host, a service+endpoint, evidence (automatic + a manual
//! confirmation), and one dependency edge.

use sqlx::PgPool;
use uuid::Uuid;

pub async fn run(pool: &PgPool) -> anyhow::Result<()> {
    let (network_id,): (Uuid,) = sqlx::query_as(
        "insert into networks (cidr, vlan, name) values ('10.20.0.0/24'::cidr, 20, 'demo-lan') returning id",
    )
    .fetch_one(pool)
    .await?;

    let (host_id,): (Uuid,) = sqlx::query_as(
        "insert into devices (device_type, name, status) values ('physical_host', 'demo-host-01', 'active') returning id",
    )
    .fetch_one(pool)
    .await?;

    let (iface_id,): (Uuid,) = sqlx::query_as(
        "insert into interfaces (device_id, mac, description) \
         values ($1, '52:54:00:aa:bb:01', 'eth0') returning id",
    )
    .bind(host_id)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "insert into addresses (interface_id, ip, address_type, is_current) \
         values ($1, '10.20.0.10'::inet, 'static', true)",
    )
    .bind(iface_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "insert into identity_rules (device_id, rule_type, value) values ($1, 'machine_id', $2)",
    )
    .bind(host_id)
    .bind(format!("demo-machine-{host_id}"))
    .execute(pool)
    .await?;

    // Automatic evidence, then a manual confirmation that outranks it at
    // read time (design Decision 7) -- demonstrates the "Why?" panel.
    sqlx::query(
        "insert into evidence (subject_table, subject_id, source_type, attribute, value, confidence, first_seen, last_seen) \
         values ('devices', $1, 'agent', 'hostname', '\"demo-host-01.local\"'::jsonb, 0.6, now(), now())",
    )
    .bind(host_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into evidence \
            (subject_table, subject_id, source_type, attribute, value, confidence, first_seen, last_seen) \
         values ('devices', $1, 'network_scan', 'open_port', '{\"port\": 22, \"proto\": \"tcp\"}'::jsonb, 0.7, now(), now())",
    )
    .bind(host_id)
    .execute(pool)
    .await?;

    let (vm_device_id,): (Uuid,) = sqlx::query_as(
        "insert into devices (device_type, name, status) values ('vm', 'demo-vm-01', 'active') returning id",
    )
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "insert into workloads (workload_type, host_device_id, runtime_id, name, status) \
         values ('vm', $1, '101', 'demo-vm-01', 'running')",
    )
    .bind(host_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "insert into containment_edges (parent_kind, parent_id, child_kind, child_id, relation) \
         values ('devices', $1, 'devices', $2, 'hosts_vm')",
    )
    .bind(host_id)
    .bind(vm_device_id)
    .execute(pool)
    .await?;

    let (service_id,): (Uuid,) = sqlx::query_as(
        "insert into services (name, protocol, product, owner_kind, owner_id) \
         values ('demo-web', 'tcp', 'nginx', 'device', $1) returning id",
    )
    .bind(vm_device_id)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "insert into endpoints (service_id, endpoint_type, address, port, is_current) \
         values ($1, 'socket', '10.20.0.11'::inet, 443, true)",
    )
    .bind(service_id)
    .execute(pool)
    .await?;

    sqlx::query(
        "insert into dependency_edges \
            (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, criticality, origin) \
         values ('devices', $1, 'devices', $2, 'network', 'hard', 'manual')",
    )
    .bind(host_id)
    .bind(vm_device_id)
    .execute(pool)
    .await?;

    tracing::info!(%network_id, %host_id, %vm_device_id, "demo topology seeded");
    println!("seeded: network={network_id} host={host_id} vm={vm_device_id}");

    Ok(())
}
