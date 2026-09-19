//! Persistence and projection for M3 fingerprints.
//!
//! M2 owns the canonical service and socket endpoint. This module consumes
//! M2's bounded `protocol_classification` evidence, runs the pure domain
//! engine, appends the resulting fingerprint evidence, and projects only safe
//! automatic fields onto the service.

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow};
use domain::fingerprinting::{
    FingerprintCandidate, FingerprintEngine, FingerprintInput, FingerprintProtocol,
    FingerprintReport,
};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use crate::inventory::{events::Recorder, evidence};

/// Evidence attribute written for every processed M2 classification.
pub const FINGERPRINT_EVIDENCE_ATTRIBUTE: &str = "fingerprint";
const M2_CLASSIFICATION_ATTRIBUTE: &str = "protocol_classification";

/// Result of one service fingerprint reconciliation.
#[derive(Debug, Clone)]
pub struct ReconcileOutcome {
    pub service_id: Uuid,
    pub fingerprint_evidence_id: Uuid,
    pub candidate: FingerprintCandidate,
    pub changed_fields: Vec<String>,
}

#[derive(Debug, Clone)]
struct StoredClassification {
    id: Uuid,
    value: Value,
    confidence: f32,
}

#[derive(Debug, Clone)]
struct FieldUpdate {
    name: &'static str,
    before: Option<String>,
    after: String,
}

/// Run M3 reconciliation in a caller-owned transaction.
///
/// `source_instance` normally contains the M2 scan-run ID. Passing `None`
/// consumes the latest non-absent M2 classification for the service. The
/// function is idempotent for one source instance and never mutates evidence.
pub async fn reconcile_service_fingerprint_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    source_instance: Option<&str>,
) -> Result<Option<ReconcileOutcome>> {
    // M2 serializes this same service/address/port path. Keep a service-level
    // lock here too for callers that reconcile a stored signal independently.
    let lock_key = format!("service-fingerprint:{service_id}");
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;

    let Some(classification) = load_classification(tx, service_id, source_instance).await? else {
        return Ok(None);
    };

    let protocol = protocol_from_m2(&classification.value)?;
    let input = FingerprintInput::from_m2_evidence(
        protocol,
        classification.confidence,
        &classification.value,
    )
    .context("invalid persisted M2 protocol classification")?;
    let report = FingerprintEngine::default().fingerprint(&input);
    let candidate = report
        .best()
        .cloned()
        .ok_or_else(|| anyhow!("fingerprint engine returned no candidate"))?;

    let evidence_value = fingerprint_evidence_value(&classification, &report)?;
    let fingerprint_evidence_id =
        match existing_fingerprint_evidence(tx, service_id, source_instance, &evidence_value)
            .await?
        {
            Some(id) => id,
            None => {
                evidence::record_automatic_tx(
                    tx,
                    "services",
                    service_id,
                    "network_scan",
                    source_instance,
                    FINGERPRINT_EVIDENCE_ATTRIBUTE,
                    &evidence_value,
                    candidate.confidence,
                    false,
                )
                .await?
            }
        };

    let (current_product, current_product_version, current_protocol): (
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "select product, product_version, protocol from services where id = $1 for update",
    )
    .bind(service_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow!("service {service_id} not found"))?;

    let manual_fields = load_manual_fields(tx, service_id).await?;
    let mut updates = Vec::new();

    if let Some(product) = candidate.product.as_ref()
        && !manual_fields.contains("product")
        && should_apply_automatic_value(
            tx,
            service_id,
            "product",
            current_product.as_deref(),
            product,
            candidate.confidence,
        )
        .await?
    {
        updates.push(FieldUpdate {
            name: "product",
            before: current_product.clone(),
            after: product.clone(),
        });
    }

    if let Some(version) = candidate.version.as_ref()
        && !manual_fields.contains("product_version")
        && should_apply_automatic_value(
            tx,
            service_id,
            "product_version",
            current_product_version.as_deref(),
            version,
            candidate.confidence,
        )
        .await?
    {
        updates.push(FieldUpdate {
            name: "product_version",
            before: current_product_version.clone(),
            after: version.clone(),
        });
    }

    let protocol_name = candidate.protocol.as_str().to_string();
    if !manual_fields.contains("protocol")
        && should_apply_automatic_value(
            tx,
            service_id,
            "protocol",
            current_protocol.as_deref(),
            &protocol_name,
            candidate.confidence,
        )
        .await?
    {
        updates.push(FieldUpdate {
            name: "protocol",
            before: current_protocol,
            after: protocol_name,
        });
    }

    if !updates.is_empty() {
        update_service_projection(tx, service_id, &updates).await?;
        Recorder::record_change(
            tx,
            "services",
            service_id,
            "service.fingerprint_changed",
            "notice",
            Some(projection_snapshot(&updates, false)),
            Some(projection_snapshot(&updates, true)),
            Some("network_scan"),
        )
        .await?;
    }

    Ok(Some(ReconcileOutcome {
        service_id,
        fingerprint_evidence_id,
        candidate,
        changed_fields: updates
            .into_iter()
            .map(|update| update.name.to_string())
            .collect(),
    }))
}

/// Reconcile the latest stored M2 classification in its own transaction.
#[allow(dead_code)]
pub async fn reconcile_service_fingerprint(
    pool: &PgPool,
    service_id: Uuid,
    source_instance: Option<&str>,
) -> Result<Option<ReconcileOutcome>> {
    let mut tx = pool.begin().await?;
    let outcome = reconcile_service_fingerprint_tx(&mut tx, service_id, source_instance).await?;
    tx.commit().await?;
    Ok(outcome)
}

async fn load_classification(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    source_instance: Option<&str>,
) -> sqlx::Result<Option<StoredClassification>> {
    sqlx::query_as::<_, (Uuid, Value, f32)>(
        "select id, value, confidence from evidence \
         where subject_table = 'services' and subject_id = $1 \
           and source_type = 'network_scan' and attribute = $2 and not absent \
           and ($3::text is null or source_instance = $3) \
         order by last_seen desc, created_at desc, id desc limit 1",
    )
    .bind(service_id)
    .bind(M2_CLASSIFICATION_ATTRIBUTE)
    .bind(source_instance)
    .fetch_optional(&mut **tx)
    .await
    .map(|row| {
        row.map(|(id, value, confidence)| StoredClassification {
            id,
            value,
            confidence,
        })
    })
}

fn protocol_from_m2(value: &Value) -> Result<FingerprintProtocol> {
    match value.get("protocol").and_then(Value::as_str) {
        Some("tcp") => Ok(FingerprintProtocol::Tcp),
        Some("http") => Ok(FingerprintProtocol::Http),
        Some("https") => Ok(FingerprintProtocol::Https),
        Some("tls") => Ok(FingerprintProtocol::Tls),
        Some("ssh") => Ok(FingerprintProtocol::Ssh),
        Some(protocol) => Err(anyhow!("unsupported M2 protocol `{protocol}`")),
        None => Err(anyhow!("M2 classification has no protocol")),
    }
}

fn fingerprint_evidence_value(
    classification: &StoredClassification,
    report: &FingerprintReport,
) -> Result<Value> {
    let candidate = report
        .best()
        .ok_or_else(|| anyhow!("fingerprint engine returned no candidate"))?;
    let mut value = serde_json::to_value(candidate).context("serialize fingerprint candidate")?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("fingerprint candidate did not serialize as an object"))?;
    // Keep service-facing terminology alongside the domain engine's `version`
    // field. This makes evidence directly explain the `services` projection.
    object.insert("product_version".to_string(), json!(candidate.version));
    object.insert(
        "m2_evidence_id".to_string(),
        Value::String(classification.id.to_string()),
    );
    object.insert(
        "candidates".to_string(),
        serde_json::to_value(&report.candidates).context("serialize fingerprint candidates")?,
    );
    Ok(value)
}

async fn existing_fingerprint_evidence(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    source_instance: Option<&str>,
    value: &Value,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "select id from evidence \
         where subject_table = 'services' and subject_id = $1 \
           and source_type = 'network_scan' and source_instance is not distinct from $2 \
           and attribute = $3 and value = $4 and not absent \
         order by created_at desc, id desc limit 1",
    )
    .bind(service_id)
    .bind(source_instance)
    .bind(FINGERPRINT_EVIDENCE_ATTRIBUTE)
    .bind(value)
    .fetch_optional(&mut **tx)
    .await
}

async fn load_manual_fields(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
) -> sqlx::Result<HashSet<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select distinct attribute from evidence \
         where subject_table = 'services' and subject_id = $1 \
           and source_type = 'manual' and confirmed_by is not null and not absent \
           and attribute in ('fingerprint', 'product', 'product_version', \
                             'protocol', 'protocol_classification')",
    )
    .bind(service_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut fields = HashSet::new();
    for (attribute,) in rows {
        if attribute == "fingerprint" {
            fields.extend([
                "product".to_string(),
                "product_version".to_string(),
                "protocol".to_string(),
            ]);
        } else if attribute == "protocol_classification" {
            fields.insert("protocol".to_string());
        } else {
            fields.insert(attribute);
        }
    }
    Ok(fields)
}

async fn should_apply_automatic_value(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    field: &'static str,
    current: Option<&str>,
    candidate: &str,
    candidate_confidence: f32,
) -> sqlx::Result<bool> {
    if current == Some(candidate) {
        return Ok(false);
    }
    let Some(current) = current else {
        return Ok(true);
    };

    let existing_confidence: Option<f32> = match field {
        "protocol" => {
            sqlx::query_scalar(
                "select max(confidence) from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'network_scan' \
               and attribute in ('protocol_classification', 'fingerprint') \
               and not absent and value->>'protocol' = $2",
            )
            .bind(service_id)
            .bind(current)
            .fetch_one(&mut **tx)
            .await?
        }
        "product" => {
            sqlx::query_scalar(
                "select max(confidence) from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = 'fingerprint' \
               and not absent and value->>'product' = $2",
            )
            .bind(service_id)
            .bind(current)
            .fetch_one(&mut **tx)
            .await?
        }
        "product_version" => {
            sqlx::query_scalar(
                "select max(confidence) from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'network_scan' and attribute = 'fingerprint' \
               and not absent and (value->>'product_version' = $2 or value->>'version' = $2)",
            )
            .bind(service_id)
            .bind(current)
            .fetch_one(&mut **tx)
            .await?
        }
        _ => None,
    };

    // A non-null field with no automatic support is treated as an existing
    // manual/legacy projection. Never replace it speculatively.
    Ok(existing_confidence.is_some_and(|confidence| candidate_confidence > confidence))
}

async fn update_service_projection(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    updates: &[FieldUpdate],
) -> sqlx::Result<()> {
    let mut query = QueryBuilder::<Postgres>::new("update services set ");
    let mut first = true;
    for update in updates {
        if !first {
            query.push(", ");
        }
        first = false;
        query.push(update.name).push(" = ").push_bind(&update.after);
    }
    if !first {
        query.push(", ");
    }
    query
        .push("version = version + 1, updated_at = now() where id = ")
        .push_bind(service_id);
    query.build().execute(&mut **tx).await?;
    Ok(())
}

fn projection_snapshot(updates: &[FieldUpdate], after: bool) -> Value {
    let mut object = Map::new();
    for update in updates {
        let value = if after {
            Value::String(update.after.clone())
        } else {
            update
                .before
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null)
        };
        object.insert(update.name.to_string(), value);
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        Some(pool)
    }

    async fn service_fixture(pool: &PgPool, protocol: &str) -> (Uuid, Uuid) {
        let (device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(pool)
                .await
                .expect("create fingerprint device");
        let (service_id,): (Uuid,) = sqlx::query_as(
            "insert into services (protocol, owner_kind, owner_id) \
             values ($1, 'device', $2) returning id",
        )
        .bind(protocol)
        .bind(device_id)
        .fetch_one(pool)
        .await
        .expect("create fingerprint service");
        let (endpoint_id,): (Uuid,) = sqlx::query_as(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', '192.0.2.50'::inet, 18080) returning id",
        )
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("create fingerprint endpoint");
        (service_id, endpoint_id)
    }

    async fn insert_classification(
        pool: &PgPool,
        service_id: Uuid,
        source_instance: &str,
        protocol: &str,
        value: Value,
        confidence: f32,
    ) {
        let mut value = value;
        value["protocol"] = Value::String(protocol.to_string());
        sqlx::query(
            "insert into evidence \
             (subject_table, subject_id, source_type, source_instance, attribute, value, confidence) \
             values ('services', $1, 'network_scan', $2, $3, $4, $5)",
        )
        .bind(service_id)
        .bind(source_instance)
        .bind(M2_CLASSIFICATION_ATTRIBUTE)
        .bind(value)
        .bind(confidence)
        .execute(pool)
        .await
        .expect("insert M2 classification");
    }

    #[tokio::test]
    async fn repeated_signals_enrich_one_service_idempotently() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = service_fixture(&pool, "http").await;
        let classification = json!({
            "status": 200,
            "headers": {"server": "Plex Media Server/1.32.5"},
            "title": "Plex",
            "body_sample": "Plex Media Server"
        });
        insert_classification(
            &pool,
            service_id,
            "scan-one",
            "http",
            classification.clone(),
            0.98,
        )
        .await;
        insert_classification(&pool, service_id, "scan-two", "http", classification, 0.98).await;

        let first = reconcile_service_fingerprint(&pool, service_id, Some("scan-one"))
            .await
            .expect("reconcile first signal")
            .expect("first signal exists");
        let repeated = reconcile_service_fingerprint(&pool, service_id, Some("scan-one"))
            .await
            .expect("reconcile repeated signal")
            .expect("repeated signal exists");
        let second = reconcile_service_fingerprint(&pool, service_id, Some("scan-two"))
            .await
            .expect("reconcile second signal")
            .expect("second signal exists");

        assert_eq!(first.candidate.product.as_deref(), Some("Plex"));
        assert_eq!(first.candidate.version.as_deref(), Some("1.32.5"));
        assert_eq!(first.changed_fields, ["product", "product_version"]);
        assert!(repeated.changed_fields.is_empty());
        assert!(second.changed_fields.is_empty());

        let (service_count, product, version, evidence_count, event_count): (
            i64,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ) = sqlx::query_as(
            "select \
                (select count(*) from services where id = $1), \
                (select product from services where id = $1), \
                (select product_version from services where id = $1), \
                (select count(*) from evidence where subject_table = 'services' \
                    and subject_id = $1 and attribute = 'fingerprint'), \
                (select count(*) from change_events where entity_kind = 'services' \
                    and entity_id = $1 and category = 'service.fingerprint_changed')",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read enriched service");
        assert_eq!(service_count, 1);
        assert_eq!(product.as_deref(), Some("Plex"));
        assert_eq!(version.as_deref(), Some("1.32.5"));
        assert_eq!(evidence_count, 2, "one append-only row per scan instance");
        assert_eq!(event_count, 1, "projection change is emitted once");
    }

    #[tokio::test]
    async fn generic_unknown_signal_stays_visible_without_product_projection() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = service_fixture(&pool, "tcp").await;
        insert_classification(
            &pool,
            service_id,
            "scan-generic",
            "tcp",
            json!({"transport": "tcp", "probe": "bounded_tcp"}),
            0.45,
        )
        .await;

        let outcome = reconcile_service_fingerprint(&pool, service_id, Some("scan-generic"))
            .await
            .expect("reconcile generic signal")
            .expect("generic signal exists");
        assert_eq!(outcome.candidate.rule_id, "fallback.tcp");
        assert!(outcome.candidate.product.is_none());
        assert!(outcome.changed_fields.is_empty());

        let (product, product_version, protocol, rule_id, fingerprint_count): (
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            i64,
        ) = sqlx::query_as(
            "select s.product, s.product_version, s.protocol, \
                    (select value->>'rule_id' from evidence \
                     where subject_table = 'services' and subject_id = s.id \
                       and attribute = 'fingerprint' limit 1), \
                    (select count(*) from evidence where subject_table = 'services' \
                       and subject_id = s.id and attribute = 'fingerprint') \
             from services s where s.id = $1",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read generic service");
        assert!(product.is_none());
        assert!(product_version.is_none());
        assert_eq!(protocol.as_deref(), Some("tcp"));
        assert_eq!(rule_id, "fallback.tcp");
        assert_eq!(fingerprint_count, 1);
    }

    #[tokio::test]
    async fn manual_values_and_evidence_are_protected() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let (service_id, _) = service_fixture(&pool, "http").await;
        let user_id: (Uuid,) = sqlx::query_as(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("fingerprint-{}@example.com", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("create fingerprint test user");
        sqlx::query(
            "update services set product = 'Manual Service', product_version = '9.9.9', \
             protocol = 'custom' where id = $1",
        )
        .bind(service_id)
        .execute(&pool)
        .await
        .expect("set manual service values");
        for (attribute, value) in [
            ("product", json!("Manual Service")),
            ("product_version", json!("9.9.9")),
            ("protocol", json!("custom")),
        ] {
            sqlx::query(
                "insert into evidence \
                 (subject_table, subject_id, source_type, attribute, value, confidence, confirmed_by) \
                 values ('services', $1, 'manual', $2, $3, 1.0, $4)",
            )
            .bind(service_id)
            .bind(attribute)
            .bind(value)
            .bind(user_id.0)
            .execute(&pool)
            .await
            .expect("insert manual service evidence");
        }
        insert_classification(
            &pool,
            service_id,
            "scan-conflicting",
            "http",
            json!({
                "status": 200,
                "headers": {"server": "Plex Media Server/1.32.5"},
                "title": "Plex",
                "body_sample": "Plex Media Server"
            }),
            0.98,
        )
        .await;

        let outcome = reconcile_service_fingerprint(&pool, service_id, Some("scan-conflicting"))
            .await
            .expect("reconcile conflicting signal")
            .expect("conflicting signal exists");
        assert_eq!(outcome.candidate.product.as_deref(), Some("Plex"));
        assert!(outcome.changed_fields.is_empty());

        let values: (Option<String>, Option<String>, Option<String>, i64, i64) = sqlx::query_as(
            "select product, product_version, protocol, \
                    (select count(*) from evidence where subject_table = 'services' \
                       and subject_id = $1 and source_type = 'manual' \
                       and confirmed_by is not null), \
                    (select count(*) from evidence where subject_table = 'services' \
                       and subject_id = $1 and attribute = 'fingerprint') \
             from services where id = $1",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read protected service");
        assert_eq!(
            values,
            (
                Some("Manual Service".to_string()),
                Some("9.9.9".to_string()),
                Some("custom".to_string()),
                3,
                1
            )
        );
    }
}
