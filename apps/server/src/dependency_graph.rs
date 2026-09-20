//! Safe dependency-edge edits and bounded graph traversal for M8.
//!
//! PostgreSQL remains authoritative. Traversals load a bounded active-edge
//! snapshot and use deterministic breadth-first walks. Rejected rows remain
//! queryable but never enter that snapshot.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::inventory::pagination::{ListParams, decode_cursor, effective_limit, encode_cursor};
use crate::state::AppState;

/// Maximum graph depth returned by M8 traversal APIs.
pub const MAX_GRAPH_DEPTH: i32 = 16;
/// Maximum rows returned by one graph traversal.
pub const MAX_GRAPH_RESULTS: usize = 200;
/// Maximum active edges inspected by one bounded traversal or cycle check.
pub const MAX_GRAPH_EDGES: i64 = 4_096;

const GRAPH_ADVISORY_LOCK_KEY: i64 = 0x4d38_6465_7067_7261;

/// One bounded upstream path from a consumer to a provider.
///
/// Edge IDs are ordered from the starting consumer toward provider_id.
/// Depth equals edge_ids.len() and starts at one for a direct provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DependencyPath {
    pub provider_kind: String,
    pub provider_id: Uuid,
    pub edge_ids: Vec<Uuid>,
    pub depth: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BlastRadiusPath {
    provider_kind: String,
    provider_id: Uuid,
    consumer_kind: String,
    consumer_id: Uuid,
    edge_ids: Vec<Uuid>,
    depth: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct GraphNode {
    kind: String,
    id: Uuid,
}

#[derive(Debug, Clone)]
struct GraphEdge {
    id: Uuid,
    provider: GraphNode,
    consumer: GraphNode,
}

type DependencyTopologyRow = (String, Uuid, String, Uuid, String, String, String, i32);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyRequest {
    provider_kind: String,
    provider_id: Uuid,
    consumer_kind: String,
    consumer_id: Uuid,
    dependency_kind: String,
    #[serde(default = "default_criticality")]
    criticality: String,
    #[serde(default = "default_origin")]
    origin: String,
    confidence: Option<f32>,
    inference_rule: Option<String>,
    #[serde(default = "default_health_propagation")]
    health_propagation: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyPatch {
    version: i32,
    provider_kind: Option<String>,
    provider_id: Option<Uuid>,
    consumer_kind: Option<String>,
    consumer_id: Option<Uuid>,
    dependency_kind: Option<String>,
    criticality: Option<String>,
    health_propagation: Option<String>,
}

fn default_criticality() -> String {
    "soft".to_string()
}

fn default_origin() -> String {
    "manual".to_string()
}

fn default_health_propagation() -> String {
    "none".to_string()
}

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": message.into()})))
}

fn validation_err(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    err(StatusCode::BAD_REQUEST, message)
}

fn graph_conflict(
    message: impl Into<String>,
    path: Option<Vec<GraphNode>>,
) -> (StatusCode, Json<Value>) {
    let mut body = json!({"error": message.into()});
    if let Some(path) = path {
        body["path"] = json!(
            path.into_iter()
                .map(|node| json!({"kind": node.kind, "id": node.id}))
                .collect::<Vec<_>>()
        );
    }
    (StatusCode::CONFLICT, Json(body))
}

fn validate_kind(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 64 {
        return Err(format!("{field} must contain 1-64 characters"));
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "{field} must not contain whitespace or control characters"
        ));
    }
    Ok(())
}

fn validate_dependency_kind(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 {
        return Err("dependency_kind must contain 1-128 characters".to_string());
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(
            "dependency_kind must not contain whitespace or control characters".to_string(),
        );
    }
    Ok(())
}

fn validate_inference_rule(rule: Option<&str>) -> Result<(), String> {
    if let Some(rule) = rule {
        if rule.is_empty() || rule.len() > 128 {
            return Err("inference_rule must contain 1-128 characters".to_string());
        }
        if rule
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(
                "inference_rule must not contain whitespace or control characters".to_string(),
            );
        }
    }
    Ok(())
}

fn validate_common(
    provider_kind: &str,
    provider_id: Uuid,
    consumer_kind: &str,
    consumer_id: Uuid,
    dependency_kind: &str,
    criticality: &str,
    health_propagation: &str,
) -> Result<(), String> {
    validate_kind(provider_kind, "provider_kind")?;
    validate_kind(consumer_kind, "consumer_kind")?;
    validate_dependency_kind(dependency_kind)?;
    if provider_id.is_nil() || consumer_id.is_nil() {
        return Err("provider_id and consumer_id must not be nil UUIDs".to_string());
    }
    if !matches!(criticality, "hard" | "soft") {
        return Err("criticality must be hard or soft".to_string());
    }
    if !matches!(health_propagation, "none" | "propagate" | "suppress_only") {
        return Err("health_propagation must be none, propagate, or suppress_only".to_string());
    }
    Ok(())
}

fn validate_confidence(confidence: Option<f32>) -> Result<(), String> {
    if let Some(confidence) = confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err("confidence must be between 0.0 and 1.0".to_string());
    }
    Ok(())
}

fn is_deterministic_inference(dependency_kind: &str, inference_rule: Option<&str>) -> bool {
    if dependency_kind == "ownership" && inference_rule == Some("ownership") {
        return true;
    }
    dependency_kind
        .strip_prefix("containment:")
        .filter(|relation| !relation.is_empty())
        .is_some_and(|relation| inference_rule == Some(format!("containment:{relation}").as_str()))
}

fn graph_node(kind: &str, id: Uuid) -> GraphNode {
    GraphNode {
        kind: kind.to_string(),
        id,
    }
}

fn edge_from_row(row: (String, Uuid, String, Uuid, Uuid)) -> GraphEdge {
    let (provider_kind, provider_id, consumer_kind, consumer_id, id) = row;
    GraphEdge {
        id,
        provider: graph_node(&provider_kind, provider_id),
        consumer: graph_node(&consumer_kind, consumer_id),
    }
}

async fn load_active_edges(pool: &PgPool) -> sqlx::Result<Vec<GraphEdge>> {
    let rows: Vec<(String, Uuid, String, Uuid, Uuid)> = sqlx::query_as(
        "select provider_kind, provider_id, consumer_kind, consumer_id, id \
         from dependency_edges \
         where coalesce(confirmation_state, 'confirmed') <> 'rejected' \
         order by provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, id \
         limit $1",
    )
    .bind(MAX_GRAPH_EDGES)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(edge_from_row).collect())
}

async fn load_active_edges_with_root(pool: &PgPool, root_id: Uuid) -> sqlx::Result<Vec<GraphEdge>> {
    let mut edges = load_active_edges(pool).await?;
    if !edges.iter().any(|edge| edge.id == root_id)
        && let Some(row) = sqlx::query_as::<_, (String, Uuid, String, Uuid, Uuid)>(
            "select provider_kind, provider_id, consumer_kind, consumer_id, id \
             from dependency_edges \
             where id = $1 and coalesce(confirmation_state, 'confirmed') <> 'rejected'",
        )
        .bind(root_id)
        .fetch_optional(pool)
        .await?
    {
        edges.push(edge_from_row(row));
    }
    Ok(edges)
}

fn adjacency(edges: &[GraphEdge], upstream: bool) -> BTreeMap<GraphNode, Vec<GraphEdge>> {
    let mut result: BTreeMap<GraphNode, Vec<GraphEdge>> = BTreeMap::new();
    for edge in edges {
        let key = if upstream {
            edge.consumer.clone()
        } else {
            edge.provider.clone()
        };
        result.entry(key).or_default().push(edge.clone());
    }
    for values in result.values_mut() {
        values.sort_by_key(|edge| {
            (
                if upstream {
                    edge.provider.clone()
                } else {
                    edge.consumer.clone()
                },
                edge.id,
            )
        });
    }
    result
}

#[derive(Debug, Clone)]
struct WalkState {
    node: GraphNode,
    edge_ids: Vec<Uuid>,
    depth: i32,
    visited: BTreeSet<GraphNode>,
}

fn upstream_paths_from_edges(
    edges: &[GraphEdge],
    consumer_kind: &str,
    consumer_id: Uuid,
) -> Vec<DependencyPath> {
    let start = graph_node(consumer_kind, consumer_id);
    let adjacency = adjacency(edges, true);
    let mut queue = VecDeque::from([WalkState {
        node: start.clone(),
        edge_ids: Vec::new(),
        depth: 0,
        visited: BTreeSet::from([start]),
    }]);
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();

    while let Some(state) = queue.pop_front() {
        if state.depth >= MAX_GRAPH_DEPTH || paths.len() >= MAX_GRAPH_RESULTS {
            continue;
        }
        let Some(next_edges) = adjacency.get(&state.node) else {
            continue;
        };
        for edge in next_edges {
            let provider = edge.provider.clone();
            if state.visited.contains(&provider) || !seen.insert(provider.clone()) {
                continue;
            }
            let mut edge_ids = state.edge_ids.clone();
            edge_ids.push(edge.id);
            let depth = state.depth + 1;
            paths.push(DependencyPath {
                provider_kind: provider.kind.clone(),
                provider_id: provider.id,
                edge_ids: edge_ids.clone(),
                depth,
            });
            if paths.len() >= MAX_GRAPH_RESULTS {
                break;
            }
            let mut visited = state.visited.clone();
            visited.insert(provider.clone());
            queue.push_back(WalkState {
                node: provider,
                edge_ids,
                depth,
                visited,
            });
        }
    }
    paths
}

fn blast_radius_from_edges(edges: &[GraphEdge], root_id: Uuid) -> Option<Vec<BlastRadiusPath>> {
    let root = edges.iter().find(|edge| edge.id == root_id)?.clone();
    let adjacency = adjacency(edges, false);
    let mut queue = VecDeque::from([WalkState {
        node: root.consumer.clone(),
        edge_ids: vec![root.id],
        depth: 1,
        visited: BTreeSet::from([root.provider.clone(), root.consumer.clone()]),
    }]);
    let mut seen = BTreeSet::from([root.consumer.clone()]);
    let mut paths = Vec::new();

    while let Some(state) = queue.pop_front() {
        paths.push(BlastRadiusPath {
            provider_kind: root.provider.kind.clone(),
            provider_id: root.provider.id,
            consumer_kind: state.node.kind.clone(),
            consumer_id: state.node.id,
            edge_ids: state.edge_ids.clone(),
            depth: state.depth,
        });
        if paths.len() >= MAX_GRAPH_RESULTS || state.depth >= MAX_GRAPH_DEPTH {
            continue;
        }
        let Some(next_edges) = adjacency.get(&state.node) else {
            continue;
        };
        for edge in next_edges {
            let consumer = edge.consumer.clone();
            if state.visited.contains(&consumer) || !seen.insert(consumer.clone()) {
                continue;
            }
            let mut edge_ids = state.edge_ids.clone();
            edge_ids.push(edge.id);
            let mut visited = state.visited.clone();
            visited.insert(consumer.clone());
            queue.push_back(WalkState {
                node: consumer,
                edge_ids,
                depth: state.depth + 1,
                visited,
            });
        }
    }
    Some(paths)
}

/// Return bounded upstream providers for one consumer.
///
/// Only active, non-rejected edges are traversed. Results are bounded by
/// MAX_GRAPH_DEPTH, MAX_GRAPH_RESULTS, and MAX_GRAPH_EDGES.
pub async fn upstream_dependency_paths(
    pool: &PgPool,
    consumer_kind: &str,
    consumer_id: Uuid,
) -> sqlx::Result<Vec<DependencyPath>> {
    let edges = load_active_edges(pool).await?;
    Ok(upstream_paths_from_edges(
        &edges,
        consumer_kind,
        consumer_id,
    ))
}

async fn lock_graph_mutation(tx: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(GRAPH_ADVISORY_LOCK_KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn cycle_path_for_edge(
    tx: &mut Transaction<'_, Postgres>,
    replacing_id: Option<Uuid>,
    provider: GraphNode,
    consumer: GraphNode,
) -> sqlx::Result<Option<Vec<GraphNode>>> {
    if provider == consumer {
        return Ok(Some(vec![provider, consumer]));
    }

    let active_count: (i64,) = sqlx::query_as(
        "select count(*) from dependency_edges \
         where coalesce(confirmation_state, 'confirmed') <> 'rejected' \
           and ($1::uuid is null or id <> $1)",
    )
    .bind(replacing_id)
    .fetch_one(&mut **tx)
    .await?;
    if active_count.0 > MAX_GRAPH_EDGES {
        return Err(sqlx::Error::Protocol(
            "dependency graph exceeds cycle-validation edge bound".to_string(),
        ));
    }

    let rows: Vec<(String, Uuid, String, Uuid, Uuid)> = sqlx::query_as(
        "select provider_kind, provider_id, consumer_kind, consumer_id, id \
         from dependency_edges \
         where coalesce(confirmation_state, 'confirmed') <> 'rejected' \
           and ($1::uuid is null or id <> $1) \
         order by provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, id",
    )
    .bind(replacing_id)
    .fetch_all(&mut **tx)
    .await?;
    let edges: Vec<GraphEdge> = rows.into_iter().map(edge_from_row).collect();
    let adjacency = adjacency(&edges, false);

    let mut queue = VecDeque::from([(consumer.clone(), vec![consumer.clone()])]);
    let mut seen = BTreeSet::from([consumer.clone()]);
    while let Some((node, path)) = queue.pop_front() {
        if node == provider {
            let mut cycle = vec![provider.clone()];
            cycle.extend(path);
            return Ok(Some(cycle));
        }
        if path.len() as i32 > MAX_GRAPH_DEPTH {
            return Err(sqlx::Error::Protocol(
                "dependency graph exceeds cycle-validation depth bound".to_string(),
            ));
        }
        if let Some(next_edges) = adjacency.get(&node) {
            for edge in next_edges {
                let next = edge.consumer.clone();
                if seen.insert(next.clone()) {
                    let mut next_path = path.clone();
                    next_path.push(next.clone());
                    queue.push_back((next, next_path));
                }
            }
        }
    }
    Ok(None)
}

async fn dependency_value_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> sqlx::Result<Option<Value>> {
    sqlx::query_as::<_, (Option<Value>,)>(
        "select row_to_json(t) from (select * from dependency_edges where id = $1) t",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map(|row| row.and_then(|(value,)| value))
}

async fn dependency_value(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Value>> {
    sqlx::query_as::<_, (Option<Value>,)>(
        "select row_to_json(t) from (select * from dependency_edges where id = $1) t",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map(|row| row.and_then(|(value,)| value))
}

fn map_db_error(error: sqlx::Error) -> (StatusCode, Json<Value>) {
    if let sqlx::Error::Protocol(message) = &error
        && message.starts_with("dependency graph exceeds cycle-validation")
    {
        return graph_conflict(message.clone(), None);
    }
    if let sqlx::Error::Database(database) = &error
        && database.code().as_deref() == Some("23505")
    {
        return err(StatusCode::CONFLICT, "dependency edge already exists");
    }
    err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

async fn create_edge(
    state: &AppState,
    user_id: Uuid,
    request: DependencyRequest,
) -> Result<Value, (StatusCode, Json<Value>)> {
    validate_common(
        &request.provider_kind,
        request.provider_id,
        &request.consumer_kind,
        request.consumer_id,
        &request.dependency_kind,
        &request.criticality,
        &request.health_propagation,
    )
    .map_err(validation_err)?;
    if !matches!(request.origin.as_str(), "manual" | "inferred") {
        return Err(validation_err("origin must be manual or inferred"));
    }
    validate_confidence(request.confidence).map_err(validation_err)?;
    validate_inference_rule(request.inference_rule.as_deref()).map_err(validation_err)?;
    if request.origin == "inferred" && request.inference_rule.is_none() {
        return Err(validation_err("inferred edges require inference_rule"));
    }
    let deterministic =
        is_deterministic_inference(&request.dependency_kind, request.inference_rule.as_deref());
    let confidence = if deterministic {
        Some(1.0_f32)
    } else {
        request.confidence
    };
    let confirmation_state = if request.origin == "manual" || deterministic {
        "confirmed"
    } else {
        "suggested"
    };
    let provenance = if request.origin == "manual" {
        "manual"
    } else {
        "inferred"
    };
    let confirmed_by = (request.origin == "manual").then_some(user_id);

    let mut tx = state.pool.begin().await.map_err(map_db_error)?;
    lock_graph_mutation(&mut tx).await.map_err(map_db_error)?;
    let provider = graph_node(&request.provider_kind, request.provider_id);
    let consumer = graph_node(&request.consumer_kind, request.consumer_id);
    if let Some(path) = cycle_path_for_edge(&mut tx, None, provider, consumer)
        .await
        .map_err(map_db_error)?
    {
        return Err(graph_conflict(
            "dependency edge would create a cycle",
            Some(path),
        ));
    }

    let row: (Value,) = sqlx::query_as(
        "insert into dependency_edges \
            (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
             criticality, origin, confidence, inference_rule, health_propagation, \
             confirmation_state, confirmed_by, confirmed_at) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, \
                 case when $11 = 'confirmed' then $12 else null end, \
                 case when $11 = 'confirmed' then now() else null end) \
         returning row_to_json(dependency_edges.*)",
    )
    .bind(&request.provider_kind)
    .bind(request.provider_id)
    .bind(&request.consumer_kind)
    .bind(request.consumer_id)
    .bind(&request.dependency_kind)
    .bind(&request.criticality)
    .bind(&request.origin)
    .bind(confidence)
    .bind(&request.inference_rule)
    .bind(&request.health_propagation)
    .bind(confirmation_state)
    .bind(confirmed_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_db_error)?;
    let id = row
        .0
        .get("id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "created dependency has no id",
            )
        })?;
    Recorder::record_change(
        &mut tx,
        "dependency_edges",
        id,
        "dependency.created",
        "notice",
        None,
        Some(row.0.clone()),
        Some(provenance),
    )
    .await
    .map_err(map_db_error)?;
    Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        "dependency.create",
        Some("dependency_edges"),
        Some(id),
        "success",
        Some(json!({
            "origin": provenance,
            "confirmation_state": confirmation_state,
        })),
    )
    .await
    .map_err(map_db_error)?;
    tx.commit().await.map_err(map_db_error)?;
    Ok(row.0)
}

async fn patch_edge(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    request: DependencyPatch,
) -> Result<Value, (StatusCode, Json<Value>)> {
    if request.version < 1 {
        return Err(validation_err("version must be positive"));
    }
    let mut tx = state.pool.begin().await.map_err(map_db_error)?;
    lock_graph_mutation(&mut tx).await.map_err(map_db_error)?;
    let current: Option<DependencyTopologyRow> = sqlx::query_as(
        "select provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
                    criticality, health_propagation, version \
             from dependency_edges where id = $1 for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(map_db_error)?;
    let Some((
        current_provider_kind,
        current_provider_id,
        current_consumer_kind,
        current_consumer_id,
        current_dependency_kind,
        current_criticality,
        current_health_propagation,
        current_version,
    )) = current
    else {
        return Err(err(StatusCode::NOT_FOUND, "dependency edge not found"));
    };
    if current_version != request.version {
        return Err(err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        ));
    }

    let provider_kind = request
        .provider_kind
        .as_deref()
        .unwrap_or(&current_provider_kind);
    let provider_id = request.provider_id.unwrap_or(current_provider_id);
    let consumer_kind = request
        .consumer_kind
        .as_deref()
        .unwrap_or(&current_consumer_kind);
    let consumer_id = request.consumer_id.unwrap_or(current_consumer_id);
    let dependency_kind = request
        .dependency_kind
        .as_deref()
        .unwrap_or(&current_dependency_kind);
    let criticality = request
        .criticality
        .as_deref()
        .unwrap_or(&current_criticality);
    let health_propagation = request
        .health_propagation
        .as_deref()
        .unwrap_or(&current_health_propagation);
    validate_common(
        provider_kind,
        provider_id,
        consumer_kind,
        consumer_id,
        dependency_kind,
        criticality,
        health_propagation,
    )
    .map_err(validation_err)?;

    let topology_changed = provider_kind != current_provider_kind
        || provider_id != current_provider_id
        || consumer_kind != current_consumer_kind
        || consumer_id != current_consumer_id;
    if topology_changed {
        let provider = graph_node(provider_kind, provider_id);
        let consumer = graph_node(consumer_kind, consumer_id);
        if let Some(path) = cycle_path_for_edge(&mut tx, Some(id), provider, consumer)
            .await
            .map_err(map_db_error)?
        {
            return Err(graph_conflict(
                "dependency edge edit would create a cycle",
                Some(path),
            ));
        }
    }

    let row: Option<(Value,)> = sqlx::query_as(
        "update dependency_edges set provider_kind = $1, provider_id = $2, \
            consumer_kind = $3, consumer_id = $4, dependency_kind = $5, \
            criticality = $6, health_propagation = $7, version = version + 1, updated_at = now() \
         where id = $8 and version = $9 returning row_to_json(dependency_edges.*)",
    )
    .bind(provider_kind)
    .bind(provider_id)
    .bind(consumer_kind)
    .bind(consumer_id)
    .bind(dependency_kind)
    .bind(criticality)
    .bind(health_propagation)
    .bind(id)
    .bind(request.version)
    .fetch_optional(&mut *tx)
    .await
    .map_err(map_db_error)?;
    let Some((after,)) = row else {
        return Err(err(
            StatusCode::CONFLICT,
            "version mismatch (row changed concurrently or does not exist)",
        ));
    };
    let before = json!({
        "id": id,
        "provider_kind": current_provider_kind,
        "provider_id": current_provider_id,
        "consumer_kind": current_consumer_kind,
        "consumer_id": current_consumer_id,
        "dependency_kind": current_dependency_kind,
        "criticality": current_criticality,
        "health_propagation": current_health_propagation,
        "version": request.version,
    });
    Recorder::record_change(
        &mut tx,
        "dependency_edges",
        id,
        "dependency.updated",
        "notice",
        Some(before),
        Some(after.clone()),
        Some("manual"),
    )
    .await
    .map_err(map_db_error)?;
    Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        "dependency.update",
        Some("dependency_edges"),
        Some(id),
        "success",
        None,
    )
    .await
    .map_err(map_db_error)?;
    tx.commit().await.map_err(map_db_error)?;
    Ok(after)
}

async fn resolve_edge(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    requested_state: &str,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let mut tx = state.pool.begin().await.map_err(map_db_error)?;
    let current: Option<(String,)> =
        sqlx::query_as("select confirmation_state from dependency_edges where id = $1 for update")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_db_error)?;
    let Some((state_value,)) = current else {
        return Err(err(StatusCode::NOT_FOUND, "dependency edge not found"));
    };
    if state_value == requested_state {
        let value = dependency_value_tx(&mut tx, id)
            .await
            .map_err(map_db_error)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "dependency edge not found"))?;
        tx.commit().await.map_err(map_db_error)?;
        return Ok(value);
    }
    if requested_state == "confirmed" && state_value == "rejected" {
        return Err(err(
            StatusCode::CONFLICT,
            "rejected dependency edge cannot be confirmed",
        ));
    }
    let before = dependency_value_tx(&mut tx, id)
        .await
        .map_err(map_db_error)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "dependency edge not found"))?;
    let row: (Value,) = sqlx::query_as(
        "update dependency_edges set confirmation_state = $2, \
            confirmed_by = case when $2 = 'confirmed' then $3 else null end, \
            confirmed_at = case when $2 = 'confirmed' then now() else null end, \
            version = version + 1, updated_at = now() \
         where id = $1 returning row_to_json(dependency_edges.*)",
    )
    .bind(id)
    .bind(requested_state)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_db_error)?;
    let action = format!("dependency.{requested_state}");
    Recorder::record_change(
        &mut tx,
        "dependency_edges",
        id,
        &action,
        "notice",
        Some(before),
        Some(row.0.clone()),
        Some("manual"),
    )
    .await
    .map_err(map_db_error)?;
    Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        &action,
        Some("dependency_edges"),
        Some(id),
        "success",
        None,
    )
    .await
    .map_err(map_db_error)?;
    tx.commit().await.map_err(map_db_error)?;
    Ok(row.0)
}

/// Reconcile containment and service-owner relations into deterministic edges.
///
/// Repeated calls converge to the same rows. Only inferred deterministic rows
/// whose source relation disappeared are deleted; manual rows are untouched.
pub async fn reconcile_deterministic_edges(pool: &PgPool) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    lock_graph_mutation(&mut tx).await?;

    let containment: Vec<(String, Uuid, String, Uuid, String)> = sqlx::query_as(
        "select parent_kind, parent_id, child_kind, child_id, relation \
         from containment_edges \
         order by parent_kind, parent_id, child_kind, child_id, relation",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (parent_kind, parent_id, child_kind, child_id, relation) in containment {
        let dependency_kind = format!("containment:{relation}");
        sqlx::query(
            "insert into dependency_edges \
                (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
                 criticality, origin, confidence, inference_rule, health_propagation, \
                 confirmation_state, confirmed_at) \
             values ($1, $2, $3, $4, $5, 'hard', 'inferred', 1.0, $6, 'suppress_only', \
                     'confirmed', now()) \
             on conflict (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind) \
             do update set criticality = 'hard', origin = 'inferred', confidence = 1.0, \
                 inference_rule = excluded.inference_rule, health_propagation = 'suppress_only', \
                 confirmation_state = case when dependency_edges.confirmation_state = 'rejected' \
                                           then 'rejected' else 'confirmed' end, \
                 confirmed_at = case when dependency_edges.confirmation_state = 'rejected' \
                                     then dependency_edges.confirmed_at else now() end, \
                 updated_at = now() \
             where dependency_edges.origin = 'inferred'",
        )
        .bind(parent_kind)
        .bind(parent_id)
        .bind(child_kind)
        .bind(child_id)
        .bind(&dependency_kind)
        .bind(&dependency_kind)
        .execute(&mut *tx)
        .await?;
    }

    let ownership: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
        "select id, owner_kind, owner_id from services \
         where owner_kind in ('device', 'workload') \
         order by id",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (service_id, owner_kind, owner_id) in ownership {
        let provider_kind = match owner_kind.as_str() {
            "device" => "devices",
            "workload" => "workloads",
            _ => continue,
        };
        sqlx::query(
            "insert into dependency_edges \
                (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
                 criticality, origin, confidence, inference_rule, health_propagation, \
                 confirmation_state, confirmed_at) \
             values ($1, $2, 'services', $3, 'ownership', 'hard', 'inferred', 1.0, \
                     'ownership', 'suppress_only', 'confirmed', now()) \
             on conflict (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind) \
             do update set criticality = 'hard', origin = 'inferred', confidence = 1.0, \
                 inference_rule = 'ownership', health_propagation = 'suppress_only', \
                 confirmation_state = case when dependency_edges.confirmation_state = 'rejected' \
                                           then 'rejected' else 'confirmed' end, \
                 confirmed_at = case when dependency_edges.confirmation_state = 'rejected' \
                                     then dependency_edges.confirmed_at else now() end, \
                 updated_at = now() \
             where dependency_edges.origin = 'inferred'",
        )
        .bind(provider_kind)
        .bind(owner_id)
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        "delete from dependency_edges d \
         where d.origin = 'inferred' \
           and not exists ( \
               select 1 from incident_notification_suppressions s \
               where s.dependency_edge_id = d.id) \
           and ((d.dependency_kind like 'containment:%' and d.inference_rule = d.dependency_kind \
                 and not exists ( \
                     select 1 from containment_edges c \
                     where c.parent_kind = d.provider_kind and c.parent_id = d.provider_id \
                       and c.child_kind = d.consumer_kind and c.child_id = d.consumer_id \
                       and ('containment:' || c.relation) = d.dependency_kind)) \
             or (d.dependency_kind = 'ownership' and d.inference_rule = 'ownership' \
                 and not exists ( \
                     select 1 from services s \
                     where s.id = d.consumer_id and s.owner_id = d.provider_id \
                       and ((s.owner_kind = 'device' and d.provider_kind = 'devices') \
                            or (s.owner_kind = 'workload' and d.provider_kind = 'workloads')))))",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(&params.cursor);
    let result = if let Some((created_at, id)) = cursor {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from dependency_edges \
             where (created_at, id) > ($1::timestamptz, $2) \
             order by created_at, id limit $3) t",
        )
        .bind(created_at)
        .bind(id)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, (Value,)>(
            "select row_to_json(t) from (select * from dependency_edges \
             order by created_at, id limit $1) t",
        )
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    };
    let rows = match result {
        Ok(rows) => rows,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let items: Vec<Value> = rows.into_iter().map(|(value,)| value).collect();
    let next_cursor = items.last().and_then(|last| {
        let created_at = last.get("created_at")?.as_str()?;
        let id = last.get("id")?.as_str()?;
        Some(encode_cursor(created_at, Uuid::parse_str(id).ok()?))
    });
    (
        StatusCode::OK,
        Json(json!({"items": items, "next_cursor": next_cursor})),
    )
}

pub async fn get(State(state): State<AppState>, Path(id): Path<Uuid>) -> (StatusCode, Json<Value>) {
    match dependency_value(&state.pool, id).await {
        Ok(Some(value)) => (StatusCode::OK, Json(value)),
        Ok(None) => err(StatusCode::NOT_FOUND, "dependency edge not found"),
        Err(error) => err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let request: DependencyRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return validation_err(format!("invalid dependency request: {error}")),
    };
    match create_edge(&state, user.0, request).await {
        Ok(value) => (StatusCode::CREATED, Json(value)),
        Err(response) => response,
    }
}

pub async fn patch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let request: DependencyPatch = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return validation_err(format!("invalid dependency patch: {error}")),
    };
    match patch_edge(&state, user.0, id, request).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(response) => response,
    }
}

pub async fn blast_radius(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let exists = match sqlx::query_scalar::<_, bool>(
        "select exists(select 1 from dependency_edges where id = $1)",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(exists) => exists,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if !exists {
        return err(StatusCode::NOT_FOUND, "dependency edge not found");
    }
    let edges = match load_active_edges_with_root(&state.pool, id).await {
        Ok(edges) => edges,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let items = blast_radius_from_edges(&edges, id).unwrap_or_default();
    (
        StatusCode::OK,
        Json(json!({
            "dependency_edge_id": id,
            "items": items,
            "max_depth": MAX_GRAPH_DEPTH,
            "max_results": MAX_GRAPH_RESULTS,
        })),
    )
}

pub async fn confirm(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match resolve_edge(&state, user.0, id, "confirmed").await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(response) => response,
    }
}

pub async fn reject(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match resolve_edge(&state, user.0, id, "rejected").await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    fn edge(id: Uuid, provider: (&str, Uuid), consumer: (&str, Uuid)) -> GraphEdge {
        GraphEdge {
            id,
            provider: graph_node(provider.0, provider.1),
            consumer: graph_node(consumer.0, consumer.1),
        }
    }

    fn cycle_path_for_test(
        edges: &[GraphEdge],
        provider: GraphNode,
        consumer: GraphNode,
    ) -> Option<Vec<GraphNode>> {
        if provider == consumer {
            return Some(vec![provider, consumer]);
        }
        let adjacency = adjacency(edges, false);
        let mut queue = VecDeque::from([(consumer.clone(), vec![consumer.clone()])]);
        let mut seen = BTreeSet::from([consumer.clone()]);
        while let Some((node, path)) = queue.pop_front() {
            if node == provider {
                let mut cycle = vec![provider.clone()];
                cycle.extend(path);
                return Some(cycle);
            }
            if let Some(next_edges) = adjacency.get(&node) {
                for edge in next_edges {
                    let next = edge.consumer.clone();
                    if seen.insert(next.clone()) {
                        let mut next_path = path.clone();
                        next_path.push(next.clone());
                        queue.push_back((next, next_path));
                    }
                }
            }
        }
        None
    }

    #[test]
    fn self_cycle_rejected() {
        let node = graph_node("services", Uuid::new_v4());
        let result = cycle_path_for_test(&[], node.clone(), node.clone());
        assert_eq!(result, Some(vec![node.clone(), node]));
    }

    #[test]
    fn indirect_cycle_rejected() {
        let a = ("services", Uuid::new_v4());
        let b = ("services", Uuid::new_v4());
        let c = ("services", Uuid::new_v4());
        let edges = vec![edge(Uuid::new_v4(), a, b), edge(Uuid::new_v4(), b, c)];
        let path = cycle_path_for_test(&edges, graph_node(c.0, c.1), graph_node(a.0, a.1));
        assert!(path.is_some());
    }

    #[test]
    fn rejected_edge_is_excluded_from_upstream_paths() {
        let consumer = Uuid::new_v4();
        let provider = Uuid::new_v4();
        let edge_id = Uuid::new_v4();
        let active = vec![edge(edge_id, ("devices", provider), ("services", consumer))];
        let paths = upstream_paths_from_edges(&active, "services", consumer);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].edge_ids, vec![edge_id]);
        let rejected_snapshot: Vec<GraphEdge> = Vec::new();
        assert!(upstream_paths_from_edges(&rejected_snapshot, "services", consumer).is_empty());
    }

    #[test]
    fn traversal_bounds_depth_and_results() {
        let start = Uuid::new_v4();
        let mut current = start;
        let mut edges = Vec::new();
        for _ in 0..(MAX_GRAPH_DEPTH + 4) {
            let provider = Uuid::new_v4();
            edges.push(edge(
                Uuid::new_v4(),
                ("services", provider),
                ("services", current),
            ));
            current = provider;
        }
        let paths = upstream_paths_from_edges(&edges, "services", start);
        assert!(paths.iter().all(|path| path.depth <= MAX_GRAPH_DEPTH));
        assert!(paths.len() <= MAX_GRAPH_RESULTS);
    }

    #[test]
    fn blast_radius_contains_direct_and_indirect_consumers() {
        let provider = Uuid::new_v4();
        let direct = Uuid::new_v4();
        let indirect = Uuid::new_v4();
        let root = Uuid::new_v4();
        let next = Uuid::new_v4();
        let edges = vec![
            edge(root, ("devices", provider), ("services", direct)),
            edge(next, ("services", direct), ("services", indirect)),
        ];
        let paths = blast_radius_from_edges(&edges, root).expect("root edge exists");
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].depth, 1);
        assert_eq!(paths[1].depth, 2);
        assert_eq!(paths[1].edge_ids, vec![root, next]);
    }

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

    #[tokio::test]
    async fn upstream_paths_exclude_rejected_database_edges() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let provider = Uuid::new_v4();
        let consumer = Uuid::new_v4();
        let rejected: (Uuid,) = sqlx::query_as(
            "insert into dependency_edges \
                (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
                 origin, confirmation_state) \
             values ('devices', $1, 'services', $2, 'network', 'inferred', 'rejected') \
             returning id",
        )
        .bind(provider)
        .bind(consumer)
        .fetch_one(&pool)
        .await
        .expect("create rejected edge");
        let paths = upstream_dependency_paths(&pool, "services", consumer)
            .await
            .expect("load upstream paths");
        assert!(paths.iter().all(|path| path.edge_ids != vec![rejected.0]));
    }

    #[tokio::test]
    async fn deterministic_reconciliation_is_idempotent_and_cleans_stale_edges() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let parent = sqlx::query_scalar::<_, Uuid>(
            "insert into devices (device_type) values ('unknown') returning id",
        )
        .fetch_one(&pool)
        .await
        .expect("create parent");
        let child = sqlx::query_scalar::<_, Uuid>(
            "insert into devices (device_type) values ('unknown') returning id",
        )
        .fetch_one(&pool)
        .await
        .expect("create child");
        sqlx::query(
            "insert into containment_edges (parent_kind, parent_id, child_kind, child_id, relation) \
             values ('devices', $1, 'devices', $2, 'hosts_vm')",
        )
        .bind(parent)
        .bind(child)
        .execute(&pool)
        .await
        .expect("create containment relation");
        reconcile_deterministic_edges(&pool)
            .await
            .expect("reconcile deterministic edges");
        reconcile_deterministic_edges(&pool)
            .await
            .expect("reconcile deterministically twice");
        let count: (i64,) = sqlx::query_as(
            "select count(*) from dependency_edges where provider_id = $1 and consumer_id = $2 \
             and dependency_kind = 'containment:hosts_vm'",
        )
        .bind(parent)
        .bind(child)
        .fetch_one(&pool)
        .await
        .expect("count deterministic edge");
        assert_eq!(count.0, 1);
        sqlx::query(
            "delete from containment_edges where parent_id = $1 and child_id = $2 and relation = 'hosts_vm'",
        )
        .bind(parent)
        .bind(child)
        .execute(&pool)
        .await
        .expect("remove source relation");
        reconcile_deterministic_edges(&pool)
            .await
            .expect("reconcile stale deterministic edge");
        let remaining: (i64,) = sqlx::query_as(
            "select count(*) from dependency_edges where provider_id = $1 and consumer_id = $2 \
             and dependency_kind = 'containment:hosts_vm'",
        )
        .bind(parent)
        .bind(child)
        .fetch_one(&pool)
        .await
        .expect("count stale deterministic edge");
        assert_eq!(remaining.0, 0);
    }

    #[tokio::test]
    async fn stale_edge_with_suppression_reason_is_preserved() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let device = sqlx::query_scalar::<_, Uuid>(
            "insert into devices (device_type) values ('unknown') returning id",
        )
        .fetch_one(&pool)
        .await
        .expect("create suppression device");
        let service = sqlx::query_scalar::<_, Uuid>(
            "insert into services (owner_kind, owner_id) values ('device', $1) returning id",
        )
        .bind(device)
        .fetch_one(&pool)
        .await
        .expect("create suppression service");
        let endpoint = sqlx::query_scalar::<_, Uuid>(
            "insert into endpoints (service_id, endpoint_type) values ($1, 'socket') returning id",
        )
        .bind(service)
        .fetch_one(&pool)
        .await
        .expect("create suppression endpoint");
        let monitor = sqlx::query_scalar::<_, Uuid>(
            "insert into monitors (service_id, endpoint_id, monitor_type) \
             values ($1, $2, 'tcp') returning id",
        )
        .bind(service)
        .bind(endpoint)
        .fetch_one(&pool)
        .await
        .expect("create suppression monitor");
        let incident = sqlx::query_scalar::<_, Uuid>(
            "insert into incidents (monitor_id) values ($1) returning id",
        )
        .bind(monitor)
        .fetch_one(&pool)
        .await
        .expect("create suppression incident");
        let stale_edge = sqlx::query_scalar::<_, Uuid>(
            "insert into dependency_edges \
                (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind, \
                 origin, confidence, inference_rule, health_propagation, confirmation_state) \
             values ('devices', $1, 'services', $2, 'containment:hosts_vm', 'inferred', \
                     1.0, 'containment:hosts_vm', 'suppress_only', 'confirmed') \
             returning id",
        )
        .bind(Uuid::new_v4())
        .bind(service)
        .fetch_one(&pool)
        .await
        .expect("create stale edge");
        sqlx::query(
            "insert into incident_notification_suppressions \
                (incident_id, root_incident_id, dependency_edge_id, provider_kind, provider_id, \
                 event_type, reason) \
             values ($1, $1, $2, 'devices', $3, 'incident.opened', 'test suppression')",
        )
        .bind(incident)
        .bind(stale_edge)
        .bind(device)
        .execute(&pool)
        .await
        .expect("create suppression reason");
        reconcile_deterministic_edges(&pool)
            .await
            .expect("reconcile edge referenced by suppression");
        let remaining: (i64,) =
            sqlx::query_as("select count(*) from dependency_edges where id = $1")
                .bind(stale_edge)
                .fetch_one(&pool)
                .await
                .expect("count preserved edge");
        assert_eq!(remaining.0, 1);
    }
}
