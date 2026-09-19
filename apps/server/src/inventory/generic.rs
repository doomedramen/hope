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

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::QueryBuilder;
use sqlx::postgres::Postgres;
use uuid::Uuid;

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
        }
    };
}

resource_handlers!(networks, NETWORKS);
resource_handlers!(interfaces, INTERFACES);
resource_handlers!(workloads, WORKLOADS);
resource_handlers!(services, SERVICES);
resource_handlers!(endpoints, ENDPOINTS);
resource_handlers!(dependency_edges, DEPENDENCY_EDGES);
