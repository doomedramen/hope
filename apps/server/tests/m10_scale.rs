//! M10 v1 homelab sizing harness.
//!
//! The live test is intentionally ignored. It creates an isolated schema,
//! applies the current migrations, loads the M10 boundary dataset, measures
//! database-backed scheduler claims, and drops the schema after the run.
//! This is a repeatable regression harness, not a production capacity test.

use std::env;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

const WARMUP_SAMPLES: usize = 5;
const MEASURED_SAMPLES: usize = 25;
const MONITOR_CLAIM_P95_TARGET_MS: u128 = 250;
const JOB_CLAIM_P95_TARGET_MS: u128 = 250;
const MONITOR_CLAIM_LEASE_SECONDS: f64 = 90.0;

// Keep this query aligned with monitor_scheduler::claim_due. The scheduler
// helper is private, so this integration test exercises its SQL path here.
const MONITOR_CLAIM_SQL: &str = r#"
with candidate as (
    select m.id from monitors m
    where m.enabled
      and (m.next_run_at is null or m.next_run_at <= now())
      and (m.lease_expires_at is null or m.lease_expires_at <= now())
    order by m.next_run_at nulls first, m.id
    for update skip locked limit 1
), claimed as (
    update monitors m
    set lease_owner = $1,
        lease_expires_at = now() + make_interval(secs => $2),
        updated_at = now(), version = version + 1
    from candidate c where m.id = c.id
    returning m.id
)
select m.id, m.service_id, m.monitor_type, m.config, m.interval_seconds,
       m.timeout_ms, m.failure_threshold, m.recovery_threshold, m.state,
       m.underlying_state, m.consecutive_failures, m.consecutive_successes,
       m.last_result_at, m.last_success_at, m.last_failure_at,
       e.address::text as address, e.port, e.endpoint_type, m.agent_id,
       a.last_heartbeat_at as agent_last_heartbeat_at,
       a.created_at as agent_created_at,
       a.heartbeat_timeout_seconds as agent_heartbeat_timeout_seconds,
       a.revoked_at as agent_revoked_at, i.inventory as agent_inventory
from claimed c
join monitors m on m.id = c.id
left join endpoints e on e.id = m.endpoint_id
left join agents a on a.id = m.agent_id
left join agent_inventory_current i on i.agent_id = m.agent_id
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScalePlan {
    devices: i64,
    services: i64,
    network_monitors: i64,
    agents: i64,
    agent_monitors: i64,
    jobs: i64,
}

impl ScalePlan {
    const fn v1_boundary() -> Self {
        Self {
            devices: 1_000,
            services: 10_000,
            network_monitors: 9_500,
            agents: 500,
            agent_monitors: 500,
            jobs: 10_000,
        }
    }

    const fn monitors(self) -> i64 {
        self.network_monitors + self.agent_monitors
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LatencyStats {
    count: usize,
    min_us: u128,
    p50_us: u128,
    p95_us: u128,
    max_us: u128,
}

fn percentile_index(sample_count: usize, percentile: usize) -> usize {
    sample_count
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
}

fn latency_stats(samples: &[Duration]) -> Result<LatencyStats> {
    if samples.is_empty() {
        bail!("latency sample set is empty");
    }

    let mut values: Vec<u128> = samples.iter().map(Duration::as_micros).collect();
    values.sort_unstable();

    Ok(LatencyStats {
        count: values.len(),
        min_us: values[0],
        p50_us: values[percentile_index(values.len(), 50)],
        p95_us: values[percentile_index(values.len(), 95)],
        max_us: values[values.len() - 1],
    })
}

fn assert_p95_target(name: &str, stats: LatencyStats, target_ms: u128) -> Result<()> {
    let target_us = target_ms * 1_000;
    if stats.p95_us > target_us {
        bail!(
            "{name} p95 target exceeded: measured {:.3} ms, target <= {target_ms} ms",
            stats.p95_us as f64 / 1_000.0
        );
    }
    Ok(())
}

fn print_latency(name: &str, stats: LatencyStats, target_ms: u128) {
    println!(
        "m10 latency {name}: samples={} min_ms={:.3} p50_ms={:.3} p95_ms={:.3} max_ms={:.3} target_p95_ms<={target_ms}",
        stats.count,
        stats.min_us as f64 / 1_000.0,
        stats.p50_us as f64 / 1_000.0,
        stats.p95_us as f64 / 1_000.0,
        stats.max_us as f64 / 1_000.0,
    );
}

fn quoted_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn scale_schema_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_nanos();
    format!("m10_scale_{}_{}", std::process::id(), nanos)
}

fn database_url() -> Result<String> {
    env::var("M10_DATABASE_URL")
        .or_else(|_| env::var("DATABASE_URL"))
        .context("set M10_DATABASE_URL (or DATABASE_URL) to a throwaway PostgreSQL database")
}

async fn seed_dataset(pool: &PgPool, plan: ScalePlan) -> Result<()> {
    let statements = [
        (
            "devices",
            r#"
insert into devices (id, device_type, name, status)
select format('00000000-0000-0000-0001-%s', lpad(g::text, 12, '0'))::uuid,
       case g % 5
           when 0 then 'physical_host'
           when 1 then 'vm'
           when 2 then 'lxc'
           when 3 then 'container_host'
           else 'appliance'
       end,
       format('m10-device-%s', g),
       'active'
from generate_series(0, 999) as series(g)
"#,
        ),
        (
            "services",
            r#"
insert into services (id, name, protocol, product, owner_kind, owner_id)
select format('00000000-0000-0000-0002-%s', lpad(g::text, 12, '0'))::uuid,
       format('m10-service-%s', g),
       'tcp',
       'synthetic',
       'device',
       format('00000000-0000-0000-0001-%s', lpad((g % 1000)::text, 12, '0'))::uuid
from generate_series(0, 9999) as series(g)
"#,
        ),
        (
            "endpoints",
            r#"
insert into endpoints (id, service_id, endpoint_type, address, port, is_current)
select format('00000000-0000-0000-0003-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0002-%s', lpad(g::text, 12, '0'))::uuid,
       'socket',
       '192.0.2.1'::inet,
       10000 + (g % 50000),
       true
from generate_series(0, 9999) as series(g)
"#,
        ),
        (
            "agents",
            r#"
insert into agents (
    id, cert_fingerprint, cert_serial, hostname, last_seen, agent_version,
    os, arch, protocol_version, capabilities, last_heartbeat_at
)
select format('00000000-0000-0000-0005-%s', lpad(g::text, 12, '0'))::uuid,
       format('m10-fingerprint-%s', g),
       format('m10-serial-%s', g),
       format('m10-agent-%s', g),
       now(),
       'm10-test',
       'linux',
       'amd64',
       1,
       '["heartbeat", "inventory"]'::jsonb,
       now()
from generate_series(0, 499) as series(g)
"#,
        ),
        (
            "agent inventory snapshots",
            r#"
insert into agent_inventory_snapshots (
    id, agent_id, message_id, protocol_version, sequence, collected_at,
    inventory, complete, source_snapshot_id
)
select format('00000000-0000-0000-0008-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0005-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0009-%s', lpad(g::text, 12, '0'))::uuid,
       1,
       1,
       now(),
       jsonb_build_object('hostname', format('m10-agent-%s', g), 'sequence', 1),
       true,
       format('00000000-0000-0000-0009-%s', lpad(g::text, 12, '0'))::uuid
from generate_series(0, 499) as series(g)
"#,
        ),
        (
            "agent current inventory",
            r#"
insert into agent_inventory_current (
    agent_id, snapshot_id, message_id, protocol_version, sequence,
    collected_at, inventory, complete, source_snapshot_id
)
select format('00000000-0000-0000-0005-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0008-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0009-%s', lpad(g::text, 12, '0'))::uuid,
       1,
       1,
       now(),
       jsonb_build_object('hostname', format('m10-agent-%s', g), 'sequence', 1),
       true,
       format('00000000-0000-0000-0009-%s', lpad(g::text, 12, '0'))::uuid
from generate_series(0, 499) as series(g)
"#,
        ),
        (
            "network monitors",
            r#"
insert into monitors (
    id, service_id, endpoint_id, monitor_type, config, interval_seconds,
    timeout_ms, next_run_at
)
select format('00000000-0000-0000-0004-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0002-%s', lpad(g::text, 12, '0'))::uuid,
       format('00000000-0000-0000-0003-%s', lpad(g::text, 12, '0'))::uuid,
       'tcp',
       '{}'::jsonb,
       30,
       500,
       now() - interval '1 second'
from generate_series(0, 9499) as series(g)
"#,
        ),
        (
            "agent monitors",
            r#"
insert into monitors (
    id, monitor_type, config, interval_seconds, timeout_ms, next_run_at,
    agent_id
)
select format('00000000-0000-0000-0004-%s', lpad((9500 + g)::text, 12, '0'))::uuid,
       'agent_heartbeat',
       '{}'::jsonb,
       30,
       500,
       now() - interval '1 second',
       format('00000000-0000-0000-0005-%s', lpad(g::text, 12, '0'))::uuid
from generate_series(0, 499) as series(g)
"#,
        ),
        (
            "jobs",
            r#"
insert into jobs (id, job_type, idempotency_key, payload, status, run_at)
select format('00000000-0000-0000-0007-%s', lpad(g::text, 12, '0'))::uuid,
       'm10.synthetic',
       format('m10-job-%s', g),
       jsonb_build_object('sequence', g),
       'pending',
       now() - interval '1 second'
from generate_series(0, 9999) as series(g)
"#,
        ),
    ];

    let expected = [
        ("devices", plan.devices),
        ("services", plan.services),
        ("endpoints", plan.services),
        ("agents", plan.agents),
        ("agent_inventory_snapshots", plan.agents),
        ("agent_inventory_current", plan.agents),
        ("monitors", plan.monitors()),
        ("jobs", plan.jobs),
    ];

    for (name, statement) in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .with_context(|| format!("seed {name}"))?;
    }

    for table in [
        "devices",
        "services",
        "endpoints",
        "agents",
        "agent_inventory_snapshots",
        "agent_inventory_current",
        "monitors",
        "jobs",
    ] {
        sqlx::query(&format!("analyze {table}"))
            .execute(pool)
            .await
            .with_context(|| format!("analyze {table}"))?;
    }

    for (table, expected_count) in expected {
        let actual: i64 = sqlx::query_scalar(&format!("select count(*) from {table}"))
            .fetch_one(pool)
            .await
            .with_context(|| format!("count {table}"))?;
        if actual != expected_count {
            bail!("seed count mismatch for {table}: expected {expected_count}, got {actual}");
        }
    }

    Ok(())
}

async fn measure_monitor_claim(pool: &PgPool) -> Result<LatencyStats> {
    let owner = "m10-monitor-benchmark";

    for _ in 0..WARMUP_SAMPLES {
        claim_monitor_once(pool, owner).await?;
    }

    let mut samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        claim_monitor_once(pool, owner).await?;
        samples.push(started.elapsed());
    }

    latency_stats(&samples)
}

async fn claim_monitor_once(pool: &PgPool, owner: &str) -> Result<()> {
    let mut transaction = pool.begin().await.context("begin monitor claim")?;
    let claimed = sqlx::query(MONITOR_CLAIM_SQL)
        .bind(owner)
        .bind(MONITOR_CLAIM_LEASE_SECONDS)
        .fetch_optional(&mut *transaction)
        .await
        .context("execute monitor claim")?;

    if claimed.is_none() {
        transaction
            .rollback()
            .await
            .context("rollback empty monitor claim")?;
        bail!("monitor claim returned no due monitor");
    }

    transaction
        .rollback()
        .await
        .context("rollback monitor claim")?;
    Ok(())
}

async fn measure_job_claim(pool: &PgPool) -> Result<LatencyStats> {
    let worker = "m10-job-benchmark";

    for _ in 0..WARMUP_SAMPLES {
        claim_job_once(pool, worker).await?;
    }

    let mut samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        claim_job_once(pool, worker).await?;
        samples.push(started.elapsed());
    }

    latency_stats(&samples)
}

async fn claim_job_once(pool: &PgPool, worker: &str) -> Result<()> {
    let claimed = jobs::claim(pool, worker, 90)
        .await
        .context("execute jobs::claim")?
        .context("job claim returned no pending job")?;

    sqlx::query(
        "update jobs set status = 'pending', locked_by = null, locked_at = null, \
         lease_expires_at = null, attempts = 0, updated_at = now() where id = $1",
    )
    .bind(claimed.id)
    .execute(pool)
    .await
    .context("reset measured job")?;

    Ok(())
}

async fn print_database_identity(pool: &PgPool) -> Result<()> {
    let (database, server_version, shared_buffers, max_connections): (
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "select current_database(), current_setting('server_version'), \
                    current_setting('shared_buffers'), current_setting('max_connections')",
    )
    .fetch_one(pool)
    .await
    .context("read PostgreSQL identity")?;

    println!(
        "m10 database: name={database} server_version={server_version} shared_buffers={shared_buffers} max_connections={max_connections}"
    );
    Ok(())
}

async fn run_database_benchmark(pool: &PgPool, plan: ScalePlan) -> Result<()> {
    print_database_identity(pool).await?;
    println!(
        "m10 dataset: devices={} services={} network_monitors={} agent_monitors={} agents={} jobs={}",
        plan.devices,
        plan.services,
        plan.network_monitors,
        plan.agent_monitors,
        plan.agents,
        plan.jobs,
    );
    println!(
        "m10 sampling: warmups={WARMUP_SAMPLES} measured={MEASURED_SAMPLES} pool_connections=1"
    );

    let monitor_stats = measure_monitor_claim(pool).await?;
    print_latency(
        "monitor_claim_transaction",
        monitor_stats,
        MONITOR_CLAIM_P95_TARGET_MS,
    );
    assert_p95_target(
        "monitor claim transaction",
        monitor_stats,
        MONITOR_CLAIM_P95_TARGET_MS,
    )?;

    let job_stats = measure_job_claim(pool).await?;
    print_latency("job_claim_transaction", job_stats, JOB_CLAIM_P95_TARGET_MS);
    assert_p95_target("job claim transaction", job_stats, JOB_CLAIM_P95_TARGET_MS)?;

    Ok(())
}

async fn drop_scale_schema(pool: &PgPool, schema: &str) -> Result<()> {
    sqlx::query("set search_path to public")
        .execute(pool)
        .await
        .context("reset search_path")?;
    sqlx::query(&format!(
        "drop schema {} cascade",
        quoted_identifier(schema)
    ))
    .execute(pool)
    .await
    .context("drop M10 benchmark schema")?;
    Ok(())
}

#[test]
fn m10_harness_smoke_is_deterministic() {
    let plan = ScalePlan::v1_boundary();
    assert_eq!(plan.devices, 1_000);
    assert_eq!(plan.services, 10_000);
    assert_eq!(plan.network_monitors + plan.agent_monitors, 10_000);
    assert_eq!(plan.agents, 500);
    assert_eq!(plan.jobs, 10_000);
    assert_eq!(percentile_index(MEASURED_SAMPLES, 50), 12);
    assert_eq!(percentile_index(MEASURED_SAMPLES, 95), 23);

    let samples = [
        Duration::from_millis(5),
        Duration::from_millis(1),
        Duration::from_millis(3),
        Duration::from_millis(2),
        Duration::from_millis(4),
    ];
    let stats = latency_stats(&samples).expect("non-empty smoke samples");
    assert_eq!(stats.count, 5);
    assert_eq!(stats.min_us, 1_000);
    assert_eq!(stats.p50_us, 3_000);
    assert_eq!(stats.p95_us, 5_000);
    assert_eq!(stats.max_us, 5_000);
    assert_p95_target("smoke", stats, 5).expect("smoke sample meets target");
}

#[tokio::test]
#[ignore = "requires M10_DATABASE_URL or DATABASE_URL and a throwaway PostgreSQL database"]
async fn m10_scale_database_scheduler_path() -> Result<()> {
    let database_url = database_url()?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&database_url)
        .await
        .context("connect to M10 PostgreSQL database")?;
    let schema = scale_schema_name();
    let quoted_schema = quoted_identifier(&schema);

    sqlx::query(&format!("create schema {quoted_schema}"))
        .execute(&pool)
        .await
        .with_context(|| format!("create isolated schema {schema}"))?;

    let benchmark_result = async {
        sqlx::query(&format!("set search_path to {quoted_schema}, public"))
            .execute(&pool)
            .await
            .context("set isolated schema search_path")?;
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .context("apply migrations in isolated schema")?;

        let plan = ScalePlan::v1_boundary();
        seed_dataset(&pool, plan).await?;
        run_database_benchmark(&pool, plan).await
    }
    .await;

    let cleanup_result = drop_scale_schema(&pool, &schema).await;
    pool.close().await;

    benchmark_result?;
    cleanup_result
}
