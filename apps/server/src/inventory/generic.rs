//! Generic cursor-paginated list/create/get/patch over the M1 entity
//! tables that don't need bespoke logic beyond optimistic-concurrency
//! version checks (§14.1). Devices (merge/split/undo, aggregated reads)
//! and evidence/identity/changes (append-only, no version check) have
//! their own modules.
//!
//! Deliberately JSON-in/JSON-out (`row_to_json`) rather than one Rust
//! struct per table: given M1's ~8 CRUD resources, this keeps the
//! handler surface small and still matches "sqlx runtime-checked
//! queries, not `query!`" — every column name is a compile-time `&str`
//! constant, only the row shape is dynamic.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use ipnet::IpNet;
use serde_json::{Value, json};
use sqlx::postgres::Postgres;
use sqlx::{QueryBuilder, Transaction};
use std::collections::BTreeMap;
use std::net::IpAddr;
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::monitor_proposals;
use crate::inventory::pagination::{ListParams, decode_cursor, effective_limit, encode_cursor};
use crate::state::AppState;

/// One writable column: its name and Postgres type, so a bound
/// `serde_json::Value` can be cast to the right wire type (`$n::<type>`)
/// instead of always arriving as json/jsonb -- see `bind_value`.
#[derive(Clone, Copy)]
pub struct Column {
    pub name: &'static str,
    pub pg_type: &'static str,
}

const fn col(name: &'static str, pg_type: &'static str) -> Column {
    Column { name, pg_type }
}

/// Which columns a resource accepts on create/patch, and its table name.
pub struct Resource {
    pub table: &'static str,
    pub insertable: &'static [Column],
    pub patchable: &'static [Column],
}

pub const NETWORKS: Resource = Resource {
    table: "networks",
    insertable: &[
        col("site_id", "uuid"),
        col("cidr", "cidr"),
        col("vlan", "integer"),
        col("gateway", "inet"),
        col("scan_policy", "jsonb"),
        col("name", "text"),
    ],
    patchable: &[
        col("cidr", "cidr"),
        col("vlan", "integer"),
        col("gateway", "inet"),
        col("scan_policy", "jsonb"),
        col("name", "text"),
    ],
};

pub const INTERFACES: Resource = Resource {
    table: "interfaces",
    insertable: &[
        col("device_id", "uuid"),
        col("mac", "macaddr"),
        col("description", "text"),
    ],
    patchable: &[col("mac", "macaddr"), col("description", "text")],
};

pub const WORKLOADS: Resource = Resource {
    table: "workloads",
    insertable: &[
        col("workload_type", "text"),
        col("host_device_id", "uuid"),
        col("runtime_id", "text"),
        col("image_or_template", "text"),
        col("name", "text"),
        col("status", "text"),
    ],
    patchable: &[
        col("host_device_id", "uuid"),
        col("runtime_id", "text"),
        col("image_or_template", "text"),
        col("name", "text"),
        col("status", "text"),
    ],
};

pub const SERVICES: Resource = Resource {
    table: "services",
    insertable: &[
        col("name", "text"),
        col("protocol", "text"),
        col("product", "text"),
        col("product_version", "text"),
        col("owner_kind", "text"),
        col("owner_id", "uuid"),
    ],
    patchable: &[
        col("name", "text"),
        col("protocol", "text"),
        col("product", "text"),
        col("product_version", "text"),
    ],
};

pub const ENDPOINTS: Resource = Resource {
    table: "endpoints",
    insertable: &[
        col("service_id", "uuid"),
        col("endpoint_type", "text"),
        col("address", "inet"),
        col("port", "integer"),
        col("url", "text"),
        col("dns_name", "text"),
    ],
    patchable: &[
        col("address", "inet"),
        col("port", "integer"),
        col("url", "text"),
        col("dns_name", "text"),
    ],
};

pub const DEPENDENCY_EDGES: Resource = Resource {
    table: "dependency_edges",
    insertable: &[
        col("provider_kind", "text"),
        col("provider_id", "uuid"),
        col("consumer_kind", "text"),
        col("consumer_id", "uuid"),
        col("dependency_kind", "text"),
        col("criticality", "text"),
        col("origin", "text"),
        col("confidence", "real"),
        col("inference_rule", "text"),
        col("health_propagation", "text"),
    ],
    patchable: &[
        col("criticality", "text"),
        col("health_propagation", "text"),
    ],
};

/// Render a JSON scalar/object as the text Postgres will parse via an
/// explicit `::<type>` cast -- avoids every bound parameter arriving as
/// json/jsonb (sqlx's default `Encode` for `serde_json::Value`) regardless
/// of the target column's real type.
fn value_to_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(_) | Value::Object(_) => Some(v.to_string()),
    }
}

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

fn network_field_errors(
    map: &serde_json::Map<String, Value>,
    require_cidr: bool,
) -> BTreeMap<&'static str, String> {
    let mut errors = BTreeMap::new();

    match map.get("cidr") {
        None | Some(Value::Null) if require_cidr => {
            errors.insert("cidr", "CIDR is required.".to_string());
        }
        Some(Value::String(value)) if !value.trim().is_empty() => {
            if value.trim().parse::<IpNet>().is_err() {
                errors.insert("cidr", "CIDR must be a valid network range.".to_string());
            }
        }
        Some(Value::String(_)) if require_cidr => {
            errors.insert("cidr", "CIDR is required.".to_string());
        }
        Some(_) if require_cidr => {
            errors.insert("cidr", "CIDR must be a valid network range.".to_string());
        }
        _ => {}
    }

    if let Some(value) = map.get("gateway").filter(|value| !value.is_null()) {
        match value {
            Value::String(value) if value.trim().is_empty() => {}
            Value::String(value) if value.trim().parse::<IpAddr>().is_ok() => {}
            _ => {
                errors.insert("gateway", "Gateway must be a valid IP address.".to_string());
            }
        }
    }

    if let Some(value) = map.get("vlan").filter(|value| !value.is_null()) {
        let valid = value_to_text(value)
            .and_then(|value| value.trim().parse::<i64>().ok())
            .is_some_and(|value| (1..=4094).contains(&value));
        if !valid {
            errors.insert("vlan", "VLAN must be between 1 and 4094.".to_string());
        }
    }

    errors
}

fn network_error(
    status: StatusCode,
    message: impl Into<String>,
    field_errors: BTreeMap<&'static str, String>,
) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({
            "error": message.into(),
            "field_errors": field_errors,
        })),
    )
}

async fn validate_network(
    state: &AppState,
    map: &serde_json::Map<String, Value>,
    require_cidr: bool,
    exclude_id: Option<Uuid>,
) -> Result<(), (StatusCode, Json<Value>)> {
    let field_errors = network_field_errors(map, require_cidr);
    if !field_errors.is_empty() {
        return Err(network_error(
            StatusCode::BAD_REQUEST,
            "Please correct the highlighted network fields.",
            field_errors,
        ));
    }

    let Some(cidr) = map
        .get("cidr")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let conflict: Result<Option<(String,)>, sqlx::Error> = match exclude_id {
        Some(id) => {
            sqlx::query_as(
                "select cidr::text from networks where cidr && $1::cidr and id <> $2 limit 1",
            )
            .bind(cidr)
            .bind(id)
            .fetch_optional(&state.pool)
            .await
        }
        None => {
            sqlx::query_as("select cidr::text from networks where cidr && $1::cidr limit 1")
                .bind(cidr)
                .fetch_optional(&state.pool)
                .await
        }
    };
    match conflict {
        Ok(Some(_)) => {
            let mut field_errors = BTreeMap::new();
            field_errors.insert(
                "cidr",
                "CIDR overlaps an existing network boundary.".to_string(),
            );
            Err(network_error(
                StatusCode::CONFLICT,
                "Network CIDR overlaps an existing boundary.",
                field_errors,
            ))
        }
        Ok(None) => Ok(()),
        Err(_) => Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Network validation is temporarily unavailable.",
        )),
    }
}

pub async fn list_generic(
    resource: &Resource,
    state: &AppState,
    params: ListParams,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(&params.cursor);

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "select coalesce(jsonb_agg(row_to_json(t)), '[]'::jsonb) as items from (select * from {} t",
        resource.table
    ));
    if let Some((created_at, id)) = &cursor {
        qb.push(" where (t.created_at, t.id) > (");
        qb.push_bind(created_at.clone());
        qb.push("::timestamptz, ");
        qb.push_bind(*id);
        qb.push(")");
    }
    qb.push(" order by t.created_at, t.id limit ");
    qb.push_bind(limit);
    qb.push(") t");

    let row: (Value,) = qb
        .build_query_as()
        .fetch_one(&state.pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items = row.0.as_array().cloned().unwrap_or_default();
    let next_cursor = items.last().and_then(|last| {
        let created_at = last.get("created_at")?.as_str()?;
        let id = last.get("id")?.as_str()?;
        Some(encode_cursor(created_at, Uuid::parse_str(id).ok()?))
    });

    Ok(json!({ "items": items, "next_cursor": next_cursor }))
}

pub async fn get_generic(
    resource: &Resource,
    state: &AppState,
    id: Uuid,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let sql = format!(
        "select row_to_json(t) from (select * from {} t where id = $1) t",
        resource.table
    );
    let row: Option<(Option<Value>,)> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match row.and_then(|(v,)| v) {
        Some(v) => Ok(v),
        None => Err(err(StatusCode::NOT_FOUND, "not found")),
    }
}

pub async fn create_generic(
    resource: &Resource,
    state: &AppState,
    body: Value,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let Value::Object(map) = &body else {
        return Err(err(StatusCode::BAD_REQUEST, "body must be a JSON object"));
    };

    if resource.table == NETWORKS.table {
        validate_network(state, map, true, None).await?;
    }

    let cols: Vec<Column> = resource
        .insertable
        .iter()
        .filter(|c| map.get(c.name).is_some_and(|v| !v.is_null()))
        .copied()
        .collect();
    if cols.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "no valid fields provided"));
    }

    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new(format!("insert into {} (", resource.table));
    for (i, col) in cols.iter().enumerate() {
        if i > 0 {
            qb.push(", ");
        }
        qb.push(col.name);
    }
    qb.push(") values (");
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            qb.push(", ");
        }
        let text = value_to_text(&map[c.name]).unwrap_or_default();
        qb.push_bind(text);
        qb.push("::");
        qb.push(c.pg_type);
    }
    qb.push(") returning row_to_json(");
    qb.push(resource.table);
    qb.push(".*) as row");

    let result: Result<(Value,), sqlx::Error> = qb.build_query_as().fetch_one(&state.pool).await;
    match result {
        Ok((row,)) => Ok(row),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

pub async fn patch_generic(
    resource: &Resource,
    state: &AppState,
    id: Uuid,
    body: Value,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let Value::Object(map) = &body else {
        return Err(err(StatusCode::BAD_REQUEST, "body must be a JSON object"));
    };
    let Some(expected_version) = map.get("version").and_then(|v| v.as_i64()) else {
        return Err(err(StatusCode::BAD_REQUEST, "version is required"));
    };

    if resource.table == NETWORKS.table {
        validate_network(state, map, false, Some(id)).await?;
    }

    let cols: Vec<Column> = resource
        .patchable
        .iter()
        .filter(|c| map.contains_key(c.name))
        .copied()
        .collect();

    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new(format!("update {} set ", resource.table));
    let mut first = true;
    for c in &cols {
        if !first {
            qb.push(", ");
        }
        first = false;
        qb.push(c.name);
        qb.push(" = ");
        match value_to_text(&map[c.name]) {
            Some(text) => {
                qb.push_bind(text);
                qb.push("::");
                qb.push(c.pg_type);
            }
            None => {
                qb.push("null::");
                qb.push(c.pg_type);
            }
        }
    }
    if !first {
        qb.push(", ");
    }
    qb.push("version = version + 1, updated_at = now() where id = ");
    qb.push_bind(id);
    qb.push(" and version = ");
    qb.push_bind(expected_version as i32);
    qb.push(" returning row_to_json(");
    qb.push(resource.table);
    qb.push(".*) as row");

    let result: Option<(Value,)> = qb
        .build_query_as()
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Some((row,)) => Ok(row),
        None => Err(err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        )),
    }
}

pub async fn delete_generic(
    resource: &Resource,
    state: &AppState,
    id: Uuid,
) -> Result<(), (StatusCode, Json<Value>)> {
    let result = sqlx::query(&format!("delete from {} where id = $1", resource.table))
        .bind(id)
        .execute(&state.pool)
        .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => Ok(()),
        Ok(_) => Err(err(StatusCode::NOT_FOUND, "not found")),
        Err(error) => {
            let foreign_key = error
                .as_database_error()
                .and_then(|database_error| database_error.code())
                .is_some_and(|code| code == "23503");
            if foreign_key {
                Err(err(
                    StatusCode::CONFLICT,
                    "cannot delete a resource with dependent records",
                ))
            } else {
                Err(err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
            }
        }
    }
}

// Service PATCH keeps projection and confirmed manual provenance in one
// optimistic-concurrency transaction. Other resources retain generic PATCH.
async fn patch_service(
    state: &AppState,
    id: Uuid,
    body: Value,
    user_id: Uuid,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let Value::Object(map) = &body else {
        return Err(err(StatusCode::BAD_REQUEST, "body must be a JSON object"));
    };
    let Some(expected_version) = map.get("version").and_then(|v| v.as_i64()) else {
        return Err(err(StatusCode::BAD_REQUEST, "version is required"));
    };

    let cols: Vec<Column> = SERVICES
        .patchable
        .iter()
        .filter(|c| map.contains_key(c.name))
        .copied()
        .collect();
    let expected_version = expected_version as i32;
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    type ServicePatchState = (Option<String>, Option<String>, Option<String>, i32);
    let current: Option<ServicePatchState> = sqlx::query_as(
        "select product, product_version, protocol, version \
         from services where id = $1 for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((current_product, current_product_version, current_protocol, current_version)) =
        current
    else {
        return Err(err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        ));
    };
    if current_version != expected_version {
        return Err(err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        ));
    }

    let manual_values: Vec<(&str, Value)> = ["product", "product_version", "protocol"]
        .into_iter()
        .filter_map(|field| {
            let requested = map.get(field)?;
            let current = match field {
                "product" => current_product.as_deref(),
                "product_version" => current_product_version.as_deref(),
                "protocol" => current_protocol.as_deref(),
                _ => unreachable!("manual service field list is fixed"),
            };
            let requested_text = value_to_text(requested);
            (requested_text.as_deref() != current).then(|| {
                (
                    field,
                    requested_text.map(Value::String).unwrap_or(Value::Null),
                )
            })
        })
        .collect();

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("update services set ");
    let mut first = true;
    for c in &cols {
        if !first {
            qb.push(", ");
        }
        first = false;
        qb.push(c.name);
        qb.push(" = ");
        match value_to_text(&map[c.name]) {
            Some(text) => {
                qb.push_bind(text);
                qb.push("::");
                qb.push(c.pg_type);
            }
            None => {
                qb.push("null::");
                qb.push(c.pg_type);
            }
        }
    }
    if !first {
        qb.push(", ");
    }
    qb.push("version = version + 1, updated_at = now() where id = ");
    qb.push_bind(id);
    qb.push(" and version = ");
    qb.push_bind(expected_version);
    qb.push(" returning row_to_json(services.*) as row");

    let row: Option<(Value,)> = qb
        .build_query_as()
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((row,)) = row else {
        return Err(err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        ));
    };

    record_manual_service_evidence(&mut tx, id, user_id, &manual_values)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    monitor_proposals::generate_for_service_tx(&mut tx, id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(row)
}

async fn record_manual_service_evidence(
    tx: &mut Transaction<'_, Postgres>,
    service_id: Uuid,
    user_id: Uuid,
    fields: &[(&str, Value)],
) -> sqlx::Result<()> {
    for (attribute, value) in fields {
        sqlx::query(
            "insert into evidence \
                (subject_table, subject_id, source_type, attribute, value, confidence, confirmed_by) \
             values ('services', $1, 'manual', $2, $3, 1.0, $4)",
        )
        .bind(service_id)
        .bind(attribute)
        .bind(value)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

// --- axum handlers, one per resource (thin wrappers so routing stays
// declarative in main.rs) ---

macro_rules! resource_handlers {
    ($module:ident, $resource:expr) => {
        pub mod $module {
            use super::*;

            pub async fn list(
                State(state): State<AppState>,
                Query(params): Query<ListParams>,
            ) -> (StatusCode, Json<Value>) {
                match list_generic(&$resource, &state, params).await {
                    Ok(v) => (StatusCode::OK, Json(v)),
                    Err((status, body)) => (status, body),
                }
            }

            pub async fn get(
                State(state): State<AppState>,
                Path(id): Path<Uuid>,
            ) -> (StatusCode, Json<Value>) {
                match get_generic(&$resource, &state, id).await {
                    Ok(v) => (StatusCode::OK, Json(v)),
                    Err((status, body)) => (status, body),
                }
            }

            pub async fn create(
                State(state): State<AppState>,
                Json(body): Json<Value>,
            ) -> (StatusCode, Json<Value>) {
                match create_generic(&$resource, &state, body).await {
                    Ok(v) => (StatusCode::CREATED, Json(v)),
                    Err((status, body)) => (status, body),
                }
            }

            pub async fn patch(
                State(state): State<AppState>,
                Path(id): Path<Uuid>,
                Json(body): Json<Value>,
            ) -> (StatusCode, Json<Value>) {
                match patch_generic(&$resource, &state, id, body).await {
                    Ok(v) => (StatusCode::OK, Json(v)),
                    Err((status, body)) => (status, body),
                }
            }

            #[allow(dead_code)]
            pub async fn delete(
                State(state): State<AppState>,
                Path(id): Path<Uuid>,
            ) -> (StatusCode, Json<Value>) {
                match delete_generic(&$resource, &state, id).await {
                    Ok(()) => (StatusCode::NO_CONTENT, Json(Value::Null)),
                    Err((status, body)) => (status, body),
                }
            }
        }
    };
}

resource_handlers!(networks, NETWORKS);
resource_handlers!(interfaces, INTERFACES);
resource_handlers!(workloads, WORKLOADS);
pub mod services {
    use super::*;

    pub async fn list(
        State(state): State<AppState>,
        Query(params): Query<ListParams>,
    ) -> (StatusCode, Json<Value>) {
        match list_generic(&SERVICES, &state, params).await {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err((status, body)) => (status, body),
        }
    }

    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> (StatusCode, Json<Value>) {
        match get_generic(&SERVICES, &state, id).await {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err((status, body)) => (status, body),
        }
    }

    pub async fn create(
        State(state): State<AppState>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        match create_generic(&SERVICES, &state, body).await {
            Ok(v) => (StatusCode::CREATED, Json(v)),
            Err((status, body)) => (status, body),
        }
    }

    pub async fn patch(
        State(state): State<AppState>,
        Extension(user): Extension<CurrentUser>,
        Path(id): Path<Uuid>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        match patch_service(&state, id, body, user.0).await {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err((status, body)) => (status, body),
        }
    }
}
resource_handlers!(endpoints, ENDPOINTS);
resource_handlers!(dependency_edges, DEPENDENCY_EDGES);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::fingerprinting;
    use serde_json::json;
    use sqlx::PgPool;

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

    #[test]
    fn network_validation_reports_invalid_fields() {
        let body = json!({
            "cidr": "not-a-cidr",
            "gateway": "not-an-ip",
            "vlan": 4096,
        });

        let errors = network_field_errors(body.as_object().expect("object"), true);

        assert_eq!(
            errors.get("cidr").map(String::as_str),
            Some("CIDR must be a valid network range.")
        );
        assert_eq!(
            errors.get("gateway").map(String::as_str),
            Some("Gateway must be a valid IP address.")
        );
        assert_eq!(
            errors.get("vlan").map(String::as_str),
            Some("VLAN must be between 1 and 4094.")
        );
    }

    #[test]
    fn network_validation_accepts_optional_fields_and_valid_values() {
        let body = json!({
            "cidr": "192.168.1.0/24",
            "gateway": "192.168.1.1",
            "vlan": 20,
        });

        assert!(network_field_errors(body.as_object().expect("object"), true).is_empty());
    }

    #[tokio::test]
    async fn network_validation_rejects_overlapping_cidr() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let state = AppState { pool: pool.clone() };
        let name = format!("generic-network-{}", Uuid::new_v4());
        sqlx::query("insert into networks (cidr, name) values ($1::cidr, $2)")
            .bind("198.51.100.0/24")
            .bind(&name)
            .execute(&pool)
            .await
            .expect("insert network fixture");

        let result = create_generic(
            &NETWORKS,
            &state,
            json!({"cidr": "198.51.100.128/25", "name": "overlap"}),
        )
        .await;

        let (status, body) = result.expect_err("overlapping CIDR should be rejected");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body.0["field_errors"]["cidr"],
            json!("CIDR overlaps an existing network boundary.")
        );
        sqlx::query("delete from networks where name = $1")
            .bind(name)
            .execute(&pool)
            .await
            .expect("delete network fixture");
    }

    #[tokio::test]
    async fn network_delete_removes_the_requested_boundary() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let state = AppState { pool: pool.clone() };
        let name = format!("delete-network-{}", Uuid::new_v4());
        let id: (Uuid,) = sqlx::query_as(
            "insert into networks (cidr, name) values ('203.0.113.0/24', $1) returning id",
        )
        .bind(&name)
        .fetch_one(&pool)
        .await
        .expect("insert network fixture");

        delete_generic(&NETWORKS, &state, id.0)
            .await
            .expect("delete network fixture");
        let remaining: Option<(Uuid,)> = sqlx::query_as("select id from networks where id = $1")
            .bind(id.0)
            .fetch_optional(&pool)
            .await
            .expect("read deleted network");
        assert!(remaining.is_none());
    }

    #[tokio::test]
    async fn service_patch_records_manual_provenance_before_fingerprint_reconciliation() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let state = AppState { pool: pool.clone() };
        let (device_id,): (Uuid,) =
            sqlx::query_as("insert into devices (device_type) values ('unknown') returning id")
                .fetch_one(&pool)
                .await
                .expect("create generic service test device");
        let (service_id,): (Uuid,) = sqlx::query_as(
            "insert into services (protocol, product, product_version, owner_kind, owner_id) \
             values ('http', 'Automatic Product', '1.0.0', 'device', $1) returning id",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("create generic service test service");
        let (user_id,): (Uuid,) = sqlx::query_as(
            "insert into users (email, password_hash) values ($1, 'test') returning id",
        )
        .bind(format!("generic-{}@example.com", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("create generic service test user");

        let conflict = patch_service(
            &state,
            service_id,
            json!({"product": "Stale Product", "version": 99}),
            user_id,
        )
        .await;
        assert!(matches!(conflict, Err((status, _)) if status == StatusCode::CONFLICT));

        let patched = patch_service(
            &state,
            service_id,
            json!({
                "product": "Manual Product",
                "product_version": "9.9.9",
                "protocol": "custom",
                "version": 1
            }),
            user_id,
        )
        .await
        .expect("patch service with manual fields");
        assert_eq!(patched["version"], json!(2));

        let manual_rows: Vec<(String, Value, Uuid)> = sqlx::query_as(
            "select attribute, value, confirmed_by from evidence \
             where subject_table = 'services' and subject_id = $1 \
               and source_type = 'manual' and confirmed_by is not null \
             order by attribute",
        )
        .bind(service_id)
        .fetch_all(&pool)
        .await
        .expect("read manual service evidence");
        assert_eq!(
            manual_rows,
            vec![
                ("product".to_string(), json!("Manual Product"), user_id),
                ("product_version".to_string(), json!("9.9.9"), user_id),
                ("protocol".to_string(), json!("custom"), user_id),
            ]
        );

        sqlx::query(
            "insert into evidence \
                (subject_table, subject_id, source_type, source_instance, attribute, value, confidence) \
             values ('services', $1, 'network_scan', 'scan-after-edit', \
                'protocol_classification', $2, 0.99)",
        )
        .bind(service_id)
        .bind(json!({
            "protocol": "http",
            "status": 200,
            "headers": {"server": "Plex Media Server/1.32.5"},
            "title": "Plex",
            "body_sample": "Plex Media Server"
        }))
        .execute(&pool)
        .await
        .expect("insert conflicting M2 classification");

        let outcome = fingerprinting::reconcile_service_fingerprint(
            &pool,
            service_id,
            Some("scan-after-edit"),
        )
        .await
        .expect("reconcile conflicting fingerprint")
        .expect("classification exists");
        assert!(outcome.changed_fields.is_empty());

        let service: (Option<String>, Option<String>, Option<String>, i32) = sqlx::query_as(
            "select product, product_version, protocol, version from services where id = $1",
        )
        .bind(service_id)
        .fetch_one(&pool)
        .await
        .expect("read manually edited service");
        assert_eq!(
            service,
            (
                Some("Manual Product".to_string()),
                Some("9.9.9".to_string()),
                Some("custom".to_string()),
                2,
            )
        );
    }
}
