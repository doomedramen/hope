//! Persistence for bounded mDNS/DNS-SD and UPnP/SSDP observations.
//!
//! Collector observations enrich existing inventory records only. mDNS is
//! attached to a current service endpoint when its advertised address/port
//! matches; otherwise it is retained on the current owning device. SSDP is
//! retained on the device owning the datagram source address. No collector
//! value causes an advertised URL to be fetched or an arbitrary endpoint to
//! be created.

use anyhow::Result;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::discovery::service_collectors::{CollectedService, MdnsService, SsdpService};
use crate::inventory::evidence;

const MDNS_CONFIDENCE: f32 = 0.85;
const SSDP_CONFIDENCE: f32 = 0.8;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PersistOutcome {
    pub observations: usize,
    pub evidence_rows_added: usize,
    pub unmatched: usize,
}

/// Persist one collector batch in one transaction. `source_instance` should
/// be the durable collector job ID so retrying a job is idempotent.
pub async fn persist_collected_observations(
    pool: &PgPool,
    source_instance: &str,
    observations: &[CollectedService],
) -> Result<PersistOutcome> {
    let mut tx = pool.begin().await?;
    let mut outcome = PersistOutcome {
        observations: observations.len(),
        ..PersistOutcome::default()
    };

    for observation in observations {
        let added = match observation {
            CollectedService::Mdns { source, service } => {
                persist_mdns(&mut tx, source_instance, *source, service).await?
            }
            CollectedService::Ssdp { source, service } => {
                persist_ssdp(&mut tx, source_instance, *source, service).await?
            }
        };
        outcome.evidence_rows_added += added;
        if added == 0 {
            outcome.unmatched += 1;
        }
    }

    tx.commit().await?;
    Ok(outcome)
}

async fn persist_mdns(
    tx: &mut Transaction<'_, Postgres>,
    source_instance: &str,
    source: std::net::SocketAddr,
    service: &MdnsService,
) -> Result<usize> {
    let mut added = 0;
    for address in &service.addresses {
        let service_id = match service.port {
            Some(port) => find_service_for_endpoint(tx, address, port).await?,
            None => None,
        };
        let value = json!({
            "protocol": "mdns",
            "source": source.to_string(),
            "address": address.to_string(),
            "instance": service.instance,
            "service_type": service.service_type,
            "hostname": service.hostname,
            "port": service.port,
            "txt": service.txt,
        });
        if let Some(service_id) = service_id {
            added += usize::from(
                record_if_new(
                    tx,
                    &EvidenceInput {
                        subject_table: "services",
                        subject_id: service_id,
                        source_type: "mdns",
                        source_instance,
                        attribute: "service_advertisement",
                        value: &value,
                        confidence: MDNS_CONFIDENCE,
                    },
                )
                .await?,
            );
            continue;
        }

        let Some(device_id) = find_device_for_address(tx, address).await? else {
            continue;
        };
        added += usize::from(
            record_if_new(
                tx,
                &EvidenceInput {
                    subject_table: "devices",
                    subject_id: device_id,
                    source_type: "mdns",
                    source_instance,
                    attribute: "service_advertisement",
                    value: &value,
                    confidence: MDNS_CONFIDENCE,
                },
            )
            .await?,
        );
    }
    Ok(added)
}

async fn persist_ssdp(
    tx: &mut Transaction<'_, Postgres>,
    source_instance: &str,
    source: std::net::SocketAddr,
    service: &SsdpService,
) -> Result<usize> {
    let Some(device_id) = find_device_for_address(tx, &source.ip()).await? else {
        return Ok(0);
    };
    let value = json!({
        "protocol": "ssdp",
        "source": source.to_string(),
        "kind": format!("{:?}", service.kind).to_ascii_lowercase(),
        "usn": service.usn,
        "search_target": service.search_target,
        "notification_type": service.notification_type,
        "location": service.location,
        "server": service.server,
        "cache_control": service.cache_control,
    });
    Ok(usize::from(
        record_if_new(
            tx,
            &EvidenceInput {
                subject_table: "devices",
                subject_id: device_id,
                source_type: "upnp",
                source_instance,
                attribute: "ssdp_advertisement",
                value: &value,
                confidence: SSDP_CONFIDENCE,
            },
        )
        .await?,
    ))
}

async fn find_service_for_endpoint(
    tx: &mut Transaction<'_, Postgres>,
    address: &std::net::IpAddr,
    port: u16,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "select service_id from endpoints \
         where address = $1::inet and port = $2 \
           and endpoint_type in ('socket', 'published_port') and is_current \
         order by last_seen desc, created_at desc, id desc limit 1",
    )
    .bind(address.to_string())
    .bind(i32::from(port))
    .fetch_optional(&mut **tx)
    .await
}

async fn find_device_for_address(
    tx: &mut Transaction<'_, Postgres>,
    address: &std::net::IpAddr,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "select i.device_id from addresses a \
         join interfaces i on i.id = a.interface_id \
         join devices d on d.id = i.device_id \
         where a.ip = $1::inet and a.is_current and d.status <> 'merged' \
         order by a.last_seen desc, d.updated_at desc, d.id desc limit 1",
    )
    .bind(address.to_string())
    .fetch_optional(&mut **tx)
    .await
}

async fn record_if_new(
    tx: &mut Transaction<'_, Postgres>,
    input: &EvidenceInput<'_>,
) -> sqlx::Result<bool> {
    let lock_key = format!(
        "collector-evidence:{}:{}:{}:{}:{}",
        input.subject_table,
        input.subject_id,
        input.source_type,
        input.source_instance,
        input.attribute,
    );
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "select id from evidence \
         where subject_table = $1 and subject_id = $2 and source_type = $3 \
           and source_instance = $4 and attribute = $5 and value = $6 and not absent \
         order by created_at desc, id desc limit 1",
    )
    .bind(input.subject_table)
    .bind(input.subject_id)
    .bind(input.source_type)
    .bind(input.source_instance)
    .bind(input.attribute)
    .bind(input.value)
    .fetch_optional(&mut **tx)
    .await?;
    if exists.is_some() {
        return Ok(false);
    }
    evidence::record_automatic_tx(
        tx,
        input.subject_table,
        input.subject_id,
        input.source_type,
        Some(input.source_instance),
        input.attribute,
        input.value,
        input.confidence,
        false,
    )
    .await?;
    Ok(true)
}

struct EvidenceInput<'a> {
    subject_table: &'a str,
    subject_id: Uuid,
    source_type: &'a str,
    source_instance: &'a str,
    attribute: &'a str,
    value: &'a Value,
    confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::service_collectors::{MdnsService, SsdpMessageKind};
    use sqlx::PgPool;
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[tokio::test]
    async fn mdns_evidence_matches_service_endpoint_and_is_idempotent() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let address_bytes = Uuid::new_v4().into_bytes();
        let address = Ipv4Addr::new(198, 18, address_bytes[0], address_bytes[1]);
        let address_text = address.to_string();
        let (interface_id,): (Uuid,) =
            sqlx::query_as("insert into interfaces (device_id) values ($1) returning id")
                .bind(device_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "insert into addresses (interface_id, ip, is_current) values ($1, $2::inet, true)",
        )
        .bind(interface_id)
        .bind(&address_text)
        .execute(&pool)
        .await
        .unwrap();
        let (service_id,): (Uuid,) = sqlx::query_as(
            "insert into services (protocol, owner_kind, owner_id) values ('tcp', 'device', $1) returning id",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into endpoints (service_id, endpoint_type, address, port, is_current) \
             values ($1, 'socket', $2::inet, 8080, true)",
        )
        .bind(service_id)
        .bind(&address_text)
        .execute(&pool)
        .await
        .unwrap();

        let observation = CollectedService::Mdns {
            source: SocketAddr::new(IpAddr::V4(address), 5353),
            service: MdnsService {
                instance: "Example._http._tcp.local".to_string(),
                service_type: "_http._tcp.local".to_string(),
                hostname: Some("example.local".to_string()),
                port: Some(8080),
                addresses: vec![IpAddr::V4(address)],
                txt: BTreeMap::new(),
            },
        };
        let first = persist_collected_observations(
            &pool,
            "collector-test",
            std::slice::from_ref(&observation),
        )
        .await
        .unwrap();
        let second = persist_collected_observations(
            &pool,
            "collector-test",
            std::slice::from_ref(&observation),
        )
        .await
        .unwrap();
        assert_eq!(first.evidence_rows_added, 1);
        assert_eq!(second.evidence_rows_added, 0);
        let count: (i64,) = sqlx::query_as(
            "select count(*) from evidence where subject_table = 'services' and subject_id = $1 \
             and source_type = 'mdns' and source_instance = 'collector-test'",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn ssdp_evidence_is_kept_on_source_device_without_fetching_location() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let address_bytes = Uuid::new_v4().into_bytes();
        let address = Ipv4Addr::new(198, 19, address_bytes[0], address_bytes[1]);
        let address_text = address.to_string();
        let (interface_id,): (Uuid,) =
            sqlx::query_as("insert into interfaces (device_id) values ($1) returning id")
                .bind(device_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "insert into addresses (interface_id, ip, is_current) values ($1, $2::inet, true)",
        )
        .bind(interface_id)
        .bind(&address_text)
        .execute(&pool)
        .await
        .unwrap();
        let observation = CollectedService::Ssdp {
            source: SocketAddr::new(IpAddr::V4(address), 1900),
            service: SsdpService {
                kind: SsdpMessageKind::Response,
                usn: "uuid:test".to_string(),
                search_target: Some("ssdp:all".to_string()),
                notification_type: None,
                location: Some(format!("http://{address}/device.xml")),
                server: Some("Hope/1.0".to_string()),
                cache_control: Some("max-age=1800".to_string()),
            },
        };
        let outcome = persist_collected_observations(&pool, "collector-ssdp-test", &[observation])
            .await
            .unwrap();
        assert_eq!(outcome.evidence_rows_added, 1);
        let row: (String, Value) = sqlx::query_as(
            "select source_type, value from evidence where subject_table = 'devices' and subject_id = $1 \
             and source_instance = 'collector-ssdp-test'",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "upnp");
        assert_eq!(row.1["location"], format!("http://{address}/device.xml"));
    }
}
