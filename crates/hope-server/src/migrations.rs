//! Ordered, transactional migrations, including adoption of pre-migration databases.
use anyhow::{Result, ensure};
use serde_json::Value;
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Version 5 is a data cleanup; version 6 resumes ordered SQL migrations.
const LATEST: i64 = 8;

pub async fn run(db: &SqlitePool) -> Result<()> {
    let mut tx = db.begin().await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *tx)
        .await?;
    let migrations = [
        (1, include_str!("schema.sql")),
        (2, include_str!("migrations/0002_statistics_and_reset.sql")),
        (3, include_str!("migrations/0003_install_jobs.sql")),
        (4, include_str!("migrations/0004_discovery_and_alerts.sql")),
    ];
    ensure!(
        version <= LATEST,
        "Database version is newer than this server"
    );
    for (target, source) in migrations {
        if version < target {
            sqlx::raw_sql(source).execute(&mut *tx).await?;
            sqlx::query(&format!("PRAGMA user_version = {target}"))
                .execute(&mut *tx)
                .await?;
        }
    }
    if version < 5 {
        forget_refusal_only_hosts(&mut tx).await?;
        sqlx::query("PRAGMA user_version = 5")
            .execute(&mut *tx)
            .await?;
    }
    if version < 6 {
        sqlx::raw_sql(include_str!("migrations/0006_metric_metadata_time.sql"))
            .execute(&mut *tx)
            .await?;
        sqlx::query("PRAGMA user_version = 6")
            .execute(&mut *tx)
            .await?;
    }
    if version < 7 {
        sqlx::raw_sql(include_str!("migrations/0007_child_sources.sql"))
            .execute(&mut *tx)
            .await?;
        sqlx::query("PRAGMA user_version = 7")
            .execute(&mut *tx)
            .await?;
    }
    if version < 8 {
        sqlx::raw_sql(include_str!("migrations/0008_source_metric_rollups.sql"))
            .execute(&mut *tx)
            .await?;
        sqlx::query("PRAGMA user_version = 8")
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Scans once counted a refused TCP connection as a device, so a firewall rejecting traffic to a
/// routed range created a host for every address in it. Forget hosts whose only evidence was such
/// a refusal: no agent, no MAC, no open port, no announcement and no administrator-set type.
async fn forget_refusal_only_hosts(db: &mut SqliteConnection) -> Result<()> {
    let rows = sqlx::query("SELECT id,body FROM hosts WHERE agent_id IS NULL")
        .fetch_all(&mut *db)
        .await?;
    let mut forgotten = vec![];
    for row in rows {
        let body: Value = serde_json::from_str(row.get("body"))?;
        if refusal_only(&body) {
            forgotten.push(row.get::<String, _>("id"));
        }
    }
    if forgotten.is_empty() {
        return Ok(());
    }
    for id in &forgotten {
        sqlx::query("DELETE FROM hosts WHERE id=?")
            .bind(id)
            .execute(&mut *db)
            .await?;
        sqlx::query("DELETE FROM alerts WHERE host_id=?")
            .bind(id)
            .execute(&mut *db)
            .await?;
    }
    let map: Option<String> = sqlx::query_scalar("SELECT value FROM meta WHERE key='map'")
        .fetch_optional(&mut *db)
        .await?;
    if let Some(map) = map {
        let mut map: Value = serde_json::from_str(&map)?;
        let gone = |id: Option<&str>| id.is_some_and(|id| forgotten.iter().any(|f| f == id));
        if let Some(links) = map.get_mut("links").and_then(Value::as_array_mut) {
            links.retain(|link| {
                !gone(link.get("source").and_then(Value::as_str))
                    && !gone(link.get("target").and_then(Value::as_str))
            });
        }
        if let Some(positions) = map.get_mut("positions").and_then(Value::as_array_mut) {
            positions.retain(|position| {
                let id = position.get("id").and_then(Value::as_str);
                !gone(id) && !gone(id.and_then(|id| id.rsplit_once(':')).map(|(_, id)| id))
            });
        }
        sqlx::query("UPDATE meta SET value=? WHERE key='map'")
            .bind(serde_json::to_string(&map)?)
            .execute(&mut *db)
            .await?;
    }
    tracing::info!(
        count = forgotten.len(),
        "forgot hosts seen only by a refused connection"
    );
    Ok(())
}

fn refusal_only(host: &Value) -> bool {
    let empty = |value: Option<&Value>| match value {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) => items.is_empty(),
        Some(Value::String(text)) => text.is_empty(),
        _ => false,
    };
    // Reverse DNS names whole ranges, so it is no evidence that an address is in use.
    let announced = host
        .get("evidence")
        .and_then(Value::as_object)
        .is_some_and(|evidence| {
            evidence
                .iter()
                .any(|(key, value)| key != "dns_name" && !empty(Some(value)))
        });
    let manual = host
        .pointer("/device/manual")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    host.get("discovery_method").and_then(Value::as_str) == Some("tcp")
        && empty(host.get("agent_id"))
        && empty(host.get("macs"))
        && empty(host.get("services"))
        && !announced
        && !manual
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn adopts_legacy_database_without_losing_data_and_rejects_future_versions() {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("database");
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&db)
            .await
            .expect("legacy schema");
        sqlx::query("INSERT INTO agents(id,credential_hash) VALUES('identity','hashed-secret')")
            .execute(&db)
            .await
            .expect("agent");
        sqlx::query("INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('identity','sample',1,'{}')").execute(&db).await.expect("history");
        run(&db).await.expect("upgrade");
        run(&db).await.expect("idempotent upgrade");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
            .fetch_one(&db)
            .await
            .expect("count");
        assert_eq!(count, 1);
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&db)
            .await
            .expect("database version");
        assert_eq!(version, 8);
        for table in [
            "child_batch_heads",
            "child_batches",
            "source_devices",
            "source_observations",
            "source_metrics",
            "source_metric_rollups",
            "device_associations",
            "device_relationships",
        ] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&db)
            .await
            .expect("source table");
            assert_eq!(exists, 1, "missing source table {table}");
        }
        let credential: String = sqlx::query_scalar("SELECT credential_hash FROM agents")
            .fetch_one(&db)
            .await
            .expect("identity");
        assert_eq!(credential, "hashed-secret");
        sqlx::query("PRAGMA user_version=999")
            .execute(&db)
            .await
            .expect("future version");
        assert!(run(&db).await.is_err());
    }
    #[tokio::test]
    async fn forgets_only_hosts_seen_solely_by_a_refused_connection() {
        let db = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("database");
        let migrations = include_str!("schema.sql").to_owned()
            + include_str!("migrations/0002_statistics_and_reset.sql")
            + include_str!("migrations/0003_install_jobs.sql")
            + include_str!("migrations/0004_discovery_and_alerts.sql")
            + "PRAGMA user_version = 4;";
        sqlx::raw_sql(&migrations)
            .execute(&db)
            .await
            .expect("version 4");
        let host = |id: &str, extra: Value| {
            let mut body = serde_json::json!({"id":id,"agent_id":null,"name":id,"addresses":["192.168.30.9"],"macs":[],"network_ids":[],"last_seen":0,"last_report":null,"discovery_method":"tcp","inventory":null,"metrics":null,"collectors":[],"services":[],"evidence":{"dns_name":"host.lan"}});
            for (key, value) in extra.as_object().expect("object") {
                body[key] = value.clone();
            }
            body
        };
        let hosts = [
            ("phantom", None, host("phantom", serde_json::json!({}))),
            (
                "pinged",
                None,
                host("pinged", serde_json::json!({"discovery_method":"icmp"})),
            ),
            (
                "mac",
                None,
                host("mac", serde_json::json!({"macs":["aa:bb:cc:dd:ee:ff"]})),
            ),
            (
                "service",
                None,
                host(
                    "service",
                    serde_json::json!({"services":[{"port":22,"name":"SSH","observed_at":0}]}),
                ),
            ),
            (
                "announced",
                None,
                host(
                    "announced",
                    serde_json::json!({"evidence":{"mdns_name":"printer"}}),
                ),
            ),
            (
                "typed",
                None,
                host(
                    "typed",
                    serde_json::json!({"device":{"kind":"nas","label":"NAS","confidence":100,"reasons":[],"manual":true}}),
                ),
            ),
            (
                "agent",
                Some("agent"),
                host("agent", serde_json::json!({"agent_id":"agent"})),
            ),
        ];
        for (id, agent, body) in hosts {
            sqlx::query("INSERT INTO hosts(id,agent_id,name,body) VALUES(?,?,?,?)")
                .bind(id)
                .bind(agent)
                .bind(id)
                .bind(body.to_string())
                .execute(&db)
                .await
                .expect("host");
        }
        sqlx::query("INSERT INTO alerts(key,host_id,kind,message,opened_at) VALUES('a','phantom','down','',0),('b','pinged','down','',0)").execute(&db).await.expect("alerts");
        let map = serde_json::json!({"revision":1,"positions":[{"id":"phantom","x":0,"y":0},{"id":"group:phantom","x":0,"y":0},{"id":"pinged","x":0,"y":0}],"links":[{"id":"l1","source":"pinged","target":"phantom","label":""},{"id":"l2","source":"pinged","target":"mac","label":""}]});
        sqlx::query("INSERT INTO meta(key,value) VALUES('map',?)")
            .bind(map.to_string())
            .execute(&db)
            .await
            .expect("map");
        run(&db).await.expect("cleanup");
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM hosts ORDER BY id")
            .fetch_all(&db)
            .await
            .expect("hosts");
        assert_eq!(
            ids,
            ["agent", "announced", "mac", "pinged", "service", "typed"]
        );
        let alerts: Vec<String> = sqlx::query_scalar("SELECT host_id FROM alerts")
            .fetch_all(&db)
            .await
            .expect("alerts");
        assert_eq!(alerts, ["pinged"]);
        let map: String = sqlx::query_scalar("SELECT value FROM meta WHERE key='map'")
            .fetch_one(&db)
            .await
            .expect("map");
        let map: Value = serde_json::from_str(&map).expect("json");
        assert_eq!(map["positions"].as_array().expect("positions").len(), 1);
        assert_eq!(map["links"].as_array().expect("links").len(), 1);
        assert_eq!(map["links"][0]["id"], "l2");
    }
}
