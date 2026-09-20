//! Maintenance planning and reservation API for milestone 9.
//!
//! PostgreSQL owns event, resource, and occurrence state. This module keeps
//! recurrence parsing at one `rrule` boundary and uses Jiff for all
//! application time arithmetic.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use jiff::civil::DateTime as CivilDateTime;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use rrule::RRuleSet;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::time::OffsetDateTime;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::dependency_graph;
use crate::inventory::events::Recorder;
use crate::inventory::pagination::{decode_cursor, effective_limit, encode_cursor};
use crate::notifications;
use crate::state::AppState;

pub const MAINTENANCE_RECONCILE_JOB_TYPE: &str = "maintenance.reconcile";

const DEFAULT_HORIZON_DAYS: i64 = 90;
const MAX_HORIZON_DAYS: i64 = 366;
const MAX_RECURRENCE_OCCURRENCES: u16 = 10_000;
const MAX_RESOURCE_COUNT: usize = 128;
const MAX_EVENT_COUNT: i64 = 256;
const MAX_OCCURRENCE_COUNT: i64 = 8_192;
const MAX_CONFLICT_COUNT: usize = 256;
const MAX_GRAPH_DEPTH: usize = 16;
const MAX_GRAPH_EDGES: usize = 4_096;
const MAX_SERVICE_IMPACTS: usize = 256;
const MAINTENANCE_ADVISORY_LOCK_KEY: i64 = 0x4d39_6d61_696e_7465;

#[derive(Debug, Clone, Serialize)]
pub struct ActiveMaintenanceImpact {
    pub event_id: Uuid,
    pub occurrence_id: Uuid,
    pub name: String,
    pub state: String,
    pub resource_role: String,
    pub resource_kind: Option<String>,
    pub resource_id: Option<Uuid>,
    pub resource_key: Option<String>,
    pub expected_failure: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceResourceRequest {
    role: String,
    #[serde(alias = "resource_kind")]
    kind: Option<String>,
    #[serde(alias = "resource_id")]
    id: Option<Uuid>,
    #[serde(alias = "resource_key")]
    key: Option<String>,
    #[serde(default)]
    expected_failure: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceEventRequest {
    name: String,
    description: Option<String>,
    timezone: String,
    #[serde(alias = "start_at")]
    start: String,
    #[serde(alias = "end_at")]
    end: Option<String>,
    duration_seconds: Option<i64>,
    #[serde(alias = "rrule")]
    recurrence_rule: Option<String>,
    #[serde(alias = "lead_in")]
    lead_in_seconds: Option<i64>,
    #[serde(alias = "cooldown")]
    cooldown_seconds: Option<i64>,
    disruptive: Option<bool>,
    notification_policy: Option<Value>,
    owner: Option<String>,
    source: Option<String>,
    notes: Option<String>,
    links: Option<Vec<String>>,
    resources: Vec<MaintenanceResourceRequest>,
    state: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceEventPatch {
    version: i32,
    name: Option<String>,
    description: Option<String>,
    timezone: Option<String>,
    #[serde(alias = "start_at")]
    start: Option<String>,
    #[serde(alias = "end_at")]
    end: Option<String>,
    duration_seconds: Option<i64>,
    #[serde(default)]
    recurrence_rule: Option<Option<String>>,
    lead_in_seconds: Option<i64>,
    cooldown_seconds: Option<i64>,
    disruptive: Option<bool>,
    notification_policy: Option<Value>,
    owner: Option<String>,
    source: Option<String>,
    notes: Option<String>,
    links: Option<Vec<String>>,
    resources: Option<Vec<MaintenanceResourceRequest>>,
    state: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleRequest {
    version: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MaintenanceListQuery {
    cursor: Option<String>,
    limit: Option<i64>,
    state: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedResource {
    role: String,
    kind: Option<String>,
    id: Option<Uuid>,
    key: Option<String>,
    expected_failure: bool,
}

#[derive(Debug, Clone)]
struct NormalizedEvent {
    name: String,
    description: Option<String>,
    timezone: String,
    start: Timestamp,
    end: Timestamp,
    recurrence_rule: Option<String>,
    lead_in_seconds: i64,
    cooldown_seconds: i64,
    disruptive: bool,
    notification_policy: Value,
    owner: Option<String>,
    source: Option<String>,
    notes: Option<String>,
    links: Vec<String>,
    state: String,
    resources: Vec<NormalizedResource>,
}

#[derive(Debug, Clone)]
struct OccurrenceValue {
    id: Uuid,
    occurrence_key: String,
    occurrence_index: i32,
    start: Timestamp,
    end: Timestamp,
    reservation_start: Timestamp,
    reservation_end: Timestamp,
    timezone: String,
}

#[derive(Debug, Clone)]
struct ResourceValue {
    role: String,
    kind: Option<String>,
    id: Option<Uuid>,
    key: Option<String>,
    expected_failure: bool,
}

#[derive(Debug, Clone)]
struct EventSnapshot {
    id: Uuid,
    name: String,
    state: String,
    disruptive: bool,
    occurrences: Vec<OccurrenceValue>,
    resources: Vec<ResourceValue>,
}

#[derive(Debug, Clone, Serialize)]
struct Conflict {
    event_id: Uuid,
    occurrence_id: Uuid,
    conflicting_event_id: Uuid,
    conflicting_occurrence_id: Uuid,
    resource_role: String,
    resource_kind: Option<String>,
    resource_id: Option<Uuid>,
    resource_key: Option<String>,
    related_resource_role: Option<String>,
    related_resource_kind: Option<String>,
    related_resource_id: Option<Uuid>,
    related_resource_key: Option<String>,
    relationship: String,
    overlap_start: String,
    overlap_end: String,
    reason: String,
    suggestion: String,
    suggested_move: SuggestedMove,
}

#[derive(Debug, Clone, Serialize)]
struct SuggestedMove {
    event_id: Uuid,
    occurrence_id: Uuid,
    start_after: String,
}

#[derive(Debug, Clone)]
struct DependencyEdgeValue {
    provider_kind: String,
    provider_id: Uuid,
    consumer_kind: String,
    consumer_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct EntityNode {
    kind: String,
    id: Uuid,
}

fn error_response(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": message.into()})))
}

fn validation_error(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    error_response(StatusCode::BAD_REQUEST, message)
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn conflict_error(conflicts: Vec<Conflict>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "maintenance event conflicts with existing reservations",
            "conflicts": conflicts,
        })),
    )
}

fn version_conflict(current_version: Option<i32>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "version mismatch (row changed concurrently or does not exist)",
            "current_version": current_version,
        })),
    )
}

fn bounded_text(value: &str, field: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(format!("{field} must contain 1-{max_bytes} bytes"));
    }
    if value.chars().any(|character| character.is_control()) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(())
}

fn optional_text(value: Option<&str>, field: &str, max_bytes: usize) -> Result<(), String> {
    if let Some(value) = value {
        bounded_text(value, field, max_bytes)?;
    }
    Ok(())
}

fn reject_secret_like_text(value: &str, field: &str) -> Result<(), String> {
    let lower = value.to_ascii_lowercase();
    const MARKERS: [&str; 9] = [
        "-----begin ",
        "password=",
        "token=",
        "secret=",
        "api_key=",
        "apikey=",
        "private_key=",
        "client_secret=",
        "authorization: bearer ",
    ];
    if MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Err(format!("{field} must not contain secret material"));
    }
    Ok(())
}

fn validate_safe_optional_text(
    value: Option<&str>,
    field: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<(), String> {
    if let Some(value) = value {
        if value.is_empty() && allow_empty {
            return Ok(());
        }
        bounded_text(value, field, max_bytes)?;
        reject_secret_like_text(value, field)?;
    }
    Ok(())
}

fn validate_resource(request: &MaintenanceResourceRequest) -> Result<NormalizedResource, String> {
    if !matches!(
        request.role.as_str(),
        "target" | "required" | "affected" | "exclusive"
    ) {
        return Err("resource role must be target, required, affected, or exclusive".to_string());
    }
    let has_entity = request.kind.is_some() || request.id.is_some();
    let has_key = request.key.is_some();
    if has_entity == has_key {
        return Err("resource must contain either kind/id or key".to_string());
    }
    if has_entity && (request.kind.is_none() || request.id.is_none()) {
        return Err("entity resource requires both kind and id".to_string());
    }
    if let Some(kind) = request.kind.as_deref() {
        bounded_text(kind, "resource kind", 64)?;
        if kind.chars().any(char::is_whitespace) {
            return Err("resource kind must not contain whitespace".to_string());
        }
    }
    if let Some(id) = request.id
        && id.is_nil()
    {
        return Err("resource id must not be nil UUID".to_string());
    }
    if let Some(key) = request.key.as_deref() {
        bounded_text(key, "resource key", 128)?;
        if key.chars().any(char::is_whitespace) {
            return Err("resource key must not contain whitespace".to_string());
        }
        reject_secret_like_text(key, "resource key")?;
    }
    Ok(NormalizedResource {
        role: request.role.clone(),
        kind: request.kind.clone(),
        id: request.id,
        key: request.key.clone(),
        expected_failure: request.expected_failure,
    })
}

fn validate_resources(
    resources: &[MaintenanceResourceRequest],
) -> Result<Vec<NormalizedResource>, String> {
    if resources.len() > MAX_RESOURCE_COUNT {
        return Err(format!(
            "resources cannot exceed {MAX_RESOURCE_COUNT} items"
        ));
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(resources.len());
    for resource in resources {
        let resource = validate_resource(resource)?;
        let identity = (
            resource.role.clone(),
            resource.kind.clone(),
            resource.id,
            resource.key.clone(),
        );
        if !seen.insert(identity) {
            return Err("duplicate maintenance resource".to_string());
        }
        normalized.push(resource);
    }
    Ok(normalized)
}

fn validate_links(links: &[String]) -> Result<(), String> {
    if links.len() > 32 {
        return Err("links cannot exceed 32 items".to_string());
    }
    for link in links {
        bounded_text(link, "link", 2048)?;
        reject_secret_like_text(link, "link")?;
        if !(link.starts_with("https://") || link.starts_with("http://")) {
            return Err("links must use http or https".to_string());
        }
    }
    Ok(())
}

fn parse_timezone(name: &str) -> Result<TimeZone, String> {
    bounded_text(name, "timezone", 128)?;
    TimeZone::get(name).map_err(|error| format!("unsupported timezone `{name}`: {error}"))
}

fn parse_schedule_time(value: &str, timezone: &str) -> Result<Timestamp, String> {
    let tz = parse_timezone(timezone)?;
    let timestamp = if let Ok(timestamp) = value.parse::<Timestamp>() {
        timestamp
    } else {
        let civil = value
            .parse::<CivilDateTime>()
            .map_err(|error| format!("invalid local datetime `{value}`: {error}"))?;
        if civil.nanosecond() != 0 {
            return Err("datetime fractional seconds are not supported".to_string());
        }
        tz.to_ambiguous_zoned(civil)
            .unambiguous()
            .map_err(|error| format!("local datetime is ambiguous or nonexistent: {error}"))?
            .timestamp()
    };
    let local = timestamp.to_zoned(tz);
    if local.datetime().nanosecond() != 0 {
        return Err("datetime fractional seconds are not supported".to_string());
    }
    Ok(timestamp)
}

fn local_ical(timestamp: Timestamp, timezone: &str) -> Result<String, String> {
    let tz = parse_timezone(timezone)?;
    let local = timestamp.to_zoned(tz).datetime();
    Ok(format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second()
    ))
}

fn timestamp_from_db(value: OffsetDateTime) -> Result<Timestamp, String> {
    Timestamp::from_nanosecond(value.unix_timestamp_nanos()).map_err(|error| error.to_string())
}

fn timestamp_to_db(value: Timestamp) -> Result<OffsetDateTime, String> {
    OffsetDateTime::from_unix_timestamp_nanos(value.as_nanosecond())
        .map_err(|error| error.to_string())
}

fn timestamp_span_seconds(start: Timestamp, end: Timestamp) -> Result<i64, String> {
    let duration = end.duration_since(start);
    if duration.subsec_nanos() != 0 || duration.as_secs() <= 0 {
        return Err("event duration must be a positive whole number of seconds".to_string());
    }
    Ok(duration.as_secs())
}

fn checked_add_seconds(value: Timestamp, seconds: i64) -> Result<Timestamp, String> {
    value
        .checked_add(Span::new().seconds(seconds))
        .map_err(|error| error.to_string())
}

fn checked_sub_seconds(value: Timestamp, seconds: i64) -> Result<Timestamp, String> {
    value
        .checked_sub(Span::new().seconds(seconds))
        .map_err(|error| error.to_string())
}

fn horizon_days() -> i64 {
    env::var("HOPE_MAINTENANCE_HORIZON_DAYS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=MAX_HORIZON_DAYS).contains(value))
        .unwrap_or(DEFAULT_HORIZON_DAYS)
}

fn global_disruptive_lock_enabled() -> bool {
    env::var("HOPE_MAINTENANCE_GLOBAL_DISRUPTIVE_LOCK")
        .ok()
        .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(true)
}

fn normalize_recurrence_rule(rule: Option<&str>) -> Result<Option<String>, String> {
    let Some(rule) = rule else {
        return Ok(None);
    };
    bounded_text(rule, "recurrence_rule", 2000)?;
    let rule = rule.strip_prefix("RRULE:").unwrap_or(rule).trim();
    if rule.is_empty() || rule.contains('\n') || rule.contains('\r') {
        return Err("recurrence_rule must contain one RRULE property".to_string());
    }
    if rule.contains("DTSTART") || rule.contains("RDATE") || rule.contains("EXDATE") {
        return Err("recurrence_rule must contain only RRULE content".to_string());
    }
    Ok(Some(rule.to_string()))
}

fn normalize_event(request: &MaintenanceEventRequest) -> Result<NormalizedEvent, String> {
    bounded_text(&request.name, "name", 200)?;
    optional_text(request.description.as_deref(), "description", 4000)?;
    optional_text(request.owner.as_deref(), "owner", 256)?;
    optional_text(request.source.as_deref(), "source", 256)?;
    validate_safe_optional_text(request.notes.as_deref(), "notes", 8000, true)?;
    let links = request.links.clone().unwrap_or_default();
    validate_links(&links)?;

    let start = parse_schedule_time(&request.start, &request.timezone)?;
    let end = match (&request.end, request.duration_seconds) {
        (Some(end), None) => parse_schedule_time(end, &request.timezone)?,
        (None, Some(duration)) => {
            if !(1..=31_536_000).contains(&duration) {
                return Err("duration_seconds must be between 1 and 31536000".to_string());
            }
            checked_add_seconds(start, duration)?
        }
        (Some(_), Some(_)) => return Err("provide end or duration_seconds, not both".to_string()),
        (None, None) => return Err("end or duration_seconds is required".to_string()),
    };
    let duration = timestamp_span_seconds(start, end)?;
    if duration > 31_536_000 {
        return Err("event duration cannot exceed 31536000 seconds".to_string());
    }
    let lead_in_seconds = request.lead_in_seconds.unwrap_or(0);
    let cooldown_seconds = request.cooldown_seconds.unwrap_or(0);
    if !(0..=2_678_400).contains(&lead_in_seconds) {
        return Err("lead_in_seconds must be between 0 and 2678400".to_string());
    }
    if !(0..=2_678_400).contains(&cooldown_seconds) {
        return Err("cooldown_seconds must be between 0 and 2678400".to_string());
    }
    let recurrence_rule = normalize_recurrence_rule(request.recurrence_rule.as_deref())?;
    let timezone = request.timezone.clone();
    parse_timezone(&timezone)?;
    let state = request.state.as_deref().unwrap_or("scheduled").to_string();
    if !matches!(state.as_str(), "draft" | "scheduled" | "upcoming") {
        return Err("maintenance event state must be draft, scheduled, or upcoming".to_string());
    }
    let notification_policy = request
        .notification_policy
        .clone()
        .unwrap_or_else(|| json!({}));
    if !notification_policy.is_object() {
        return Err("notification_policy must be a JSON object".to_string());
    }
    if notification_policy.to_string().len() > 16_384 {
        return Err("notification_policy is too large".to_string());
    }
    let resources = validate_resources(&request.resources)?;
    if recurrence_rule.is_some() && resources.is_empty() {
        return Err("recurring maintenance event requires at least one resource".to_string());
    }
    Ok(NormalizedEvent {
        name: request.name.clone(),
        description: request.description.clone(),
        timezone,
        start,
        end,
        recurrence_rule,
        lead_in_seconds,
        cooldown_seconds,
        disruptive: request.disruptive.unwrap_or(false),
        notification_policy,
        owner: request.owner.clone(),
        source: request.source.clone(),
        notes: request.notes.clone(),
        links,
        state,
        resources,
    })
}

fn rrule_input(event: &NormalizedEvent) -> Result<String, String> {
    let start = local_ical(event.start, &event.timezone)?;
    let rule = event
        .recurrence_rule
        .as_deref()
        .ok_or_else(|| "recurrence rule is missing".to_string())?;
    if event.timezone.eq_ignore_ascii_case("UTC") {
        Ok(format!("DTSTART:{start}Z\nRRULE:{rule}"))
    } else {
        Ok(format!(
            "DTSTART;TZID={}:{}\nRRULE:{rule}",
            event.timezone, start
        ))
    }
}

fn stable_occurrence_id(event_id: Uuid, occurrence_key: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(event_id.as_bytes());
    digest.update([0]);
    digest.update(occurrence_key.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn expand_occurrences(event: &NormalizedEvent) -> Result<Vec<OccurrenceValue>, String> {
    let duration = timestamp_span_seconds(event.start, event.end)?;
    let horizon_end = checked_add_seconds(event.start, horizon_days() * 86_400)?;
    let mut starts = Vec::new();
    if event.recurrence_rule.is_none() {
        starts.push(event.start);
    } else {
        let input = rrule_input(event)?;
        let schedule: RRuleSet = input
            .parse()
            .map_err(|error| format!("invalid recurrence_rule: {error}"))?;
        let bounded_horizon = checked_add_seconds(horizon_end, 1)?;
        let horizon_local = local_ical(bounded_horizon, &event.timezone)?;
        let horizon_input = if event.timezone.eq_ignore_ascii_case("UTC") {
            format!("DTSTART:{horizon_local}Z\nRRULE:FREQ=DAILY;COUNT=1")
        } else {
            format!(
                "DTSTART;TZID={}:{}\nRRULE:FREQ=DAILY;COUNT=1",
                event.timezone, horizon_local
            )
        };
        let horizon_schedule: RRuleSet = horizon_input
            .parse()
            .map_err(|error| format!("invalid recurrence horizon: {error}"))?;
        let result = schedule
            .before(*horizon_schedule.get_dt_start())
            .all(MAX_RECURRENCE_OCCURRENCES);
        if result.limited {
            return Err(format!(
                "recurrence expansion exceeds {MAX_RECURRENCE_OCCURRENCES} occurrences"
            ));
        }
        let tz = parse_timezone(&event.timezone)?;
        for value in result.dates {
            let local_text = value.format("%Y-%m-%dT%H:%M:%S").to_string();
            let local = local_text
                .parse::<CivilDateTime>()
                .map_err(|error| format!("invalid recurrence datetime: {error}"))?;
            let start = tz
                .to_ambiguous_zoned(local)
                .unambiguous()
                .map_err(|error| {
                    format!("recurrence contains ambiguous or nonexistent local time: {error}")
                })?
                .timestamp();
            if start <= horizon_end {
                starts.push(start);
            }
        }
        if starts.is_empty() {
            return Err("recurrence has no occurrence within expansion horizon".to_string());
        }
    }
    starts.sort_unstable();
    starts.dedup();
    if starts.len() > i32::MAX as usize {
        return Err("recurrence occurrence count exceeds database limit".to_string());
    }
    starts
        .into_iter()
        .enumerate()
        .map(|(index, start)| {
            let end = checked_add_seconds(start, duration)?;
            let reservation_start = checked_sub_seconds(start, event.lead_in_seconds)?;
            let reservation_end = checked_add_seconds(end, event.cooldown_seconds)?;
            let occurrence_key = start.to_string();
            Ok(OccurrenceValue {
                id: Uuid::nil(),
                occurrence_key,
                occurrence_index: i32::try_from(index)
                    .map_err(|error| format!("occurrence index overflow: {error}"))?,
                start,
                end,
                reservation_start,
                reservation_end,
                timezone: event.timezone.clone(),
            })
        })
        .collect()
}

fn apply_occurrence_ids(event_id: Uuid, occurrences: &mut [OccurrenceValue]) {
    for occurrence in occurrences {
        occurrence.id = stable_occurrence_id(event_id, &occurrence.occurrence_key);
    }
}

fn legal_transition(from: &str, to: &str) -> bool {
    match from {
        "draft" => matches!(to, "scheduled" | "cancelled"),
        "scheduled" => matches!(to, "upcoming" | "active" | "cancelled"),
        "upcoming" => matches!(to, "active" | "cancelled"),
        "active" => matches!(to, "overrunning" | "completed" | "cancelled"),
        "overrunning" => matches!(to, "active" | "completed" | "cancelled"),
        "completed" | "cancelled" => false,
        _ => false,
    }
}

fn resource_identity(resource: &ResourceValue) -> String {
    match (&resource.kind, resource.id, &resource.key) {
        (Some(kind), Some(id), _) => format!("{kind}/{id}"),
        (_, _, Some(key)) => key.clone(),
        _ => "invalid-resource".to_string(),
    }
}

fn same_resource(left: &ResourceValue, right: &ResourceValue) -> bool {
    left.kind == right.kind && left.id == right.id && left.key == right.key
}

fn overlap(left: &OccurrenceValue, right: &OccurrenceValue) -> Option<(Timestamp, Timestamp)> {
    let start = left.reservation_start.max(right.reservation_start);
    let end = left.reservation_end.min(right.reservation_end);
    (start < end).then_some((start, end))
}

fn entity_downstream(
    affected: &EntityNode,
    target: &EntityNode,
    edges: &[DependencyEdgeValue],
) -> bool {
    if affected == target {
        return true;
    }
    let mut adjacency: BTreeMap<EntityNode, Vec<EntityNode>> = BTreeMap::new();
    for edge in edges.iter().take(MAX_GRAPH_EDGES) {
        adjacency
            .entry(EntityNode {
                kind: edge.provider_kind.clone(),
                id: edge.provider_id,
            })
            .or_default()
            .push(EntityNode {
                kind: edge.consumer_kind.clone(),
                id: edge.consumer_id,
            });
    }
    let mut queue = VecDeque::from([(affected.clone(), 0_usize)]);
    let mut visited = BTreeSet::from([affected.clone()]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= MAX_GRAPH_DEPTH {
            continue;
        }
        let Some(children) = adjacency.get(&node) else {
            continue;
        };
        for child in children {
            if child == target {
                return true;
            }
            if visited.insert(child.clone()) {
                queue.push_back((child.clone(), depth + 1));
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn conflict_from_resources(
    left: &EventSnapshot,
    left_occurrence: &OccurrenceValue,
    right: &EventSnapshot,
    right_occurrence: &OccurrenceValue,
    left_resource: &ResourceValue,
    right_resource: Option<&ResourceValue>,
    relationship: &str,
    overlap_start: Timestamp,
    overlap_end: Timestamp,
) -> Conflict {
    let later_is_right = left_occurrence.reservation_start <= right_occurrence.reservation_start;
    let (later_event, later_occurrence) = if later_is_right {
        (right, right_occurrence)
    } else {
        (left, left_occurrence)
    };
    let earlier_occurrence = if later_is_right {
        left_occurrence
    } else {
        right_occurrence
    };
    let start_after = earlier_occurrence.reservation_end.max(overlap_end);
    let resource_name = resource_identity(left_resource);
    let related_name = right_resource.map(resource_identity);
    let related_detail = related_name
        .as_deref()
        .map(|name| format!(" resource {name}"))
        .unwrap_or_default();
    let reason = format!(
        "event {} ({}) resource {}{} overlaps event {} ({}) reservation from {} to {}",
        left.name,
        left.id,
        resource_name,
        related_detail,
        right.name,
        right.id,
        overlap_start,
        overlap_end
    );
    let suggestion = format!(
        "move later event {} occurrence {} to start after {}",
        later_event.id, later_occurrence.id, start_after
    );
    Conflict {
        event_id: left.id,
        occurrence_id: left_occurrence.id,
        conflicting_event_id: right.id,
        conflicting_occurrence_id: right_occurrence.id,
        resource_role: left_resource.role.clone(),
        resource_kind: left_resource.kind.clone(),
        resource_id: left_resource.id,
        resource_key: left_resource.key.clone(),
        related_resource_role: right_resource.map(|resource| resource.role.clone()),
        related_resource_kind: right_resource.and_then(|resource| resource.kind.clone()),
        related_resource_id: right_resource.and_then(|resource| resource.id),
        related_resource_key: right_resource.and_then(|resource| resource.key.clone()),
        relationship: relationship.to_string(),
        overlap_start: overlap_start.to_string(),
        overlap_end: overlap_end.to_string(),
        reason,
        suggestion,
        suggested_move: SuggestedMove {
            event_id: later_event.id,
            occurrence_id: later_occurrence.id,
            start_after: start_after.to_string(),
        },
    }
}

fn conflicts_between(
    left: &EventSnapshot,
    right: &EventSnapshot,
    edges: &[DependencyEdgeValue],
) -> Result<Vec<Conflict>, String> {
    if matches!(left.state.as_str(), "completed" | "cancelled")
        || matches!(right.state.as_str(), "completed" | "cancelled")
    {
        return Ok(Vec::new());
    }
    let mut conflicts = Vec::new();
    for left_occurrence in &left.occurrences {
        for right_occurrence in &right.occurrences {
            let Some((overlap_start, overlap_end)) = overlap(left_occurrence, right_occurrence)
            else {
                continue;
            };
            if global_disruptive_lock_enabled() && left.disruptive && right.disruptive {
                let left_resource = ResourceValue {
                    role: "exclusive".to_string(),
                    kind: None,
                    id: None,
                    key: Some("global-disruptive-maintenance".to_string()),
                    expected_failure: false,
                };
                let right_resource = left_resource.clone();
                conflicts.push(conflict_from_resources(
                    left,
                    left_occurrence,
                    right,
                    right_occurrence,
                    &left_resource,
                    Some(&right_resource),
                    "global_disruptive_lock",
                    overlap_start,
                    overlap_end,
                ));
                if conflicts.len() >= MAX_CONFLICT_COUNT {
                    return Ok(conflicts);
                }
            }
            for left_resource in &left.resources {
                for right_resource in &right.resources {
                    let relationship = if left_resource.role == "affected"
                        && right_resource.role == "required"
                        && same_resource(left_resource, right_resource)
                    {
                        Some("affected_required")
                    } else if left_resource.role == "required"
                        && right_resource.role == "affected"
                        && same_resource(left_resource, right_resource)
                    {
                        Some("required_affected")
                    } else if left_resource.role == "exclusive"
                        && right_resource.role == "exclusive"
                        && same_resource(left_resource, right_resource)
                    {
                        Some("exclusive_shared")
                    } else {
                        None
                    };
                    if let Some(relationship) = relationship {
                        conflicts.push(conflict_from_resources(
                            left,
                            left_occurrence,
                            right,
                            right_occurrence,
                            left_resource,
                            Some(right_resource),
                            relationship,
                            overlap_start,
                            overlap_end,
                        ));
                    }
                    if left_resource.role == "target"
                        && right_resource.role == "affected"
                        && let (Some(left_kind), Some(left_id), Some(right_kind), Some(right_id)) = (
                            left_resource.kind.as_deref(),
                            left_resource.id,
                            right_resource.kind.as_deref(),
                            right_resource.id,
                        )
                        && entity_downstream(
                            &EntityNode {
                                kind: right_kind.to_string(),
                                id: right_id,
                            },
                            &EntityNode {
                                kind: left_kind.to_string(),
                                id: left_id,
                            },
                            edges,
                        )
                    {
                        conflicts.push(conflict_from_resources(
                            left,
                            left_occurrence,
                            right,
                            right_occurrence,
                            left_resource,
                            Some(right_resource),
                            "target_depends_on_affected",
                            overlap_start,
                            overlap_end,
                        ));
                    }
                    if right_resource.role == "target"
                        && left_resource.role == "affected"
                        && let (Some(left_kind), Some(left_id), Some(right_kind), Some(right_id)) = (
                            left_resource.kind.as_deref(),
                            left_resource.id,
                            right_resource.kind.as_deref(),
                            right_resource.id,
                        )
                        && entity_downstream(
                            &EntityNode {
                                kind: left_kind.to_string(),
                                id: left_id,
                            },
                            &EntityNode {
                                kind: right_kind.to_string(),
                                id: right_id,
                            },
                            edges,
                        )
                    {
                        conflicts.push(conflict_from_resources(
                            left,
                            left_occurrence,
                            right,
                            right_occurrence,
                            left_resource,
                            Some(right_resource),
                            "target_depends_on_affected",
                            overlap_start,
                            overlap_end,
                        ));
                    }
                    if conflicts.len() >= MAX_CONFLICT_COUNT {
                        return Ok(conflicts);
                    }
                }
            }
        }
    }
    Ok(deduplicate_conflicts(conflicts))
}

fn deduplicate_conflicts(conflicts: Vec<Conflict>) -> Vec<Conflict> {
    let mut seen = BTreeSet::new();
    conflicts
        .into_iter()
        .filter(|conflict| {
            seen.insert((
                conflict.event_id,
                conflict.occurrence_id,
                conflict.conflicting_event_id,
                conflict.conflicting_occurrence_id,
                conflict.relationship.clone(),
                conflict.resource_kind.clone(),
                conflict.resource_id,
                conflict.resource_key.clone(),
                conflict.related_resource_kind.clone(),
                conflict.related_resource_id,
                conflict.related_resource_key.clone(),
            ))
        })
        .collect()
}

#[derive(Debug, sqlx::FromRow)]
struct MaintenanceEventRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    timezone: String,
    start_at: OffsetDateTime,
    end_at: OffsetDateTime,
    recurrence_rule: Option<String>,
    lead_in_seconds: i64,
    cooldown_seconds: i64,
    disruptive: bool,
    notification_policy: Value,
    owner: Option<String>,
    source: Option<String>,
    notes: Option<String>,
    links: Value,
    state: String,
    version: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct MaintenanceResourceRow {
    event_id: Uuid,
    role: String,
    resource_kind: Option<String>,
    resource_id: Option<Uuid>,
    resource_key: Option<String>,
    expected_failure: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct MaintenanceOccurrenceRow {
    id: Uuid,
    event_id: Uuid,
    occurrence_key: String,
    occurrence_index: i32,
    start_at: OffsetDateTime,
    end_at: OffsetDateTime,
    reservation_start: OffsetDateTime,
    reservation_end: OffsetDateTime,
    timezone: String,
}

fn db_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

fn event_row_to_normalized(
    row: &MaintenanceEventRow,
    resources: Vec<ResourceValue>,
) -> Result<NormalizedEvent, String> {
    let links = row
        .links
        .as_array()
        .ok_or_else(|| "stored links is not an array".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "stored link is not a string".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let start = timestamp_from_db(row.start_at)?;
    let end = timestamp_from_db(row.end_at)?;
    let normalized_resources = resources
        .into_iter()
        .map(|resource| NormalizedResource {
            role: resource.role,
            kind: resource.kind,
            id: resource.id,
            key: resource.key,
            expected_failure: resource.expected_failure,
        })
        .collect();
    Ok(NormalizedEvent {
        name: row.name.clone(),
        description: row.description.clone(),
        timezone: row.timezone.clone(),
        start,
        end,
        recurrence_rule: row.recurrence_rule.clone(),
        lead_in_seconds: row.lead_in_seconds,
        cooldown_seconds: row.cooldown_seconds,
        disruptive: row.disruptive,
        notification_policy: row.notification_policy.clone(),
        owner: row.owner.clone(),
        source: row.source.clone(),
        notes: row.notes.clone(),
        links,
        state: row.state.clone(),
        resources: normalized_resources,
    })
}

fn resource_row_to_value(row: MaintenanceResourceRow) -> ResourceValue {
    ResourceValue {
        role: row.role,
        kind: row.resource_kind,
        id: row.resource_id,
        key: row.resource_key,
        expected_failure: row.expected_failure,
    }
}

fn occurrence_row_to_value(row: MaintenanceOccurrenceRow) -> Result<OccurrenceValue, String> {
    Ok(OccurrenceValue {
        id: row.id,
        occurrence_key: row.occurrence_key,
        occurrence_index: row.occurrence_index,
        start: timestamp_from_db(row.start_at)?,
        end: timestamp_from_db(row.end_at)?,
        reservation_start: timestamp_from_db(row.reservation_start)?,
        reservation_end: timestamp_from_db(row.reservation_end)?,
        timezone: row.timezone,
    })
}

async fn load_event_row(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    for_update: bool,
) -> sqlx::Result<Option<(MaintenanceEventRow, Vec<ResourceValue>)>> {
    let query = if for_update {
        "select id, name, description, timezone, start_at, end_at, recurrence_rule, \
                lead_in_seconds, cooldown_seconds, disruptive, notification_policy, \
                owner, source, notes, links, state, version \
         from maintenance_events where id = $1 for update"
    } else {
        "select id, name, description, timezone, start_at, end_at, recurrence_rule, \
                lead_in_seconds, cooldown_seconds, disruptive, notification_policy, \
                owner, source, notes, links, state, version \
         from maintenance_events where id = $1"
    };
    let Some(row) = sqlx::query_as::<_, MaintenanceEventRow>(query)
        .bind(event_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(None);
    };
    let resources = sqlx::query_as::<_, MaintenanceResourceRow>(
        "select event_id, role, resource_kind, resource_id, resource_key, expected_failure \
         from maintenance_resources where event_id = $1 order by id",
    )
    .bind(event_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(resource_row_to_value)
    .collect();
    Ok(Some((row, resources)))
}

fn normalized_to_request(event: &NormalizedEvent) -> MaintenanceEventRequest {
    MaintenanceEventRequest {
        name: event.name.clone(),
        description: event.description.clone(),
        timezone: event.timezone.clone(),
        start: event.start.to_string(),
        end: Some(event.end.to_string()),
        duration_seconds: None,
        recurrence_rule: event.recurrence_rule.clone(),
        lead_in_seconds: Some(event.lead_in_seconds),
        cooldown_seconds: Some(event.cooldown_seconds),
        disruptive: Some(event.disruptive),
        notification_policy: Some(event.notification_policy.clone()),
        owner: event.owner.clone(),
        source: event.source.clone(),
        notes: event.notes.clone(),
        links: Some(event.links.clone()),
        resources: event
            .resources
            .iter()
            .map(|resource| MaintenanceResourceRequest {
                role: resource.role.clone(),
                kind: resource.kind.clone(),
                id: resource.id,
                key: resource.key.clone(),
                expected_failure: resource.expected_failure,
            })
            .collect(),
        state: Some(event.state.clone()),
    }
}

fn patch_event(
    current: NormalizedEvent,
    patch: MaintenanceEventPatch,
) -> Result<NormalizedEvent, String> {
    let mut request = normalized_to_request(&current);
    if let Some(name) = patch.name {
        request.name = name;
    }
    if let Some(description) = patch.description {
        request.description = Some(description);
    }
    if let Some(timezone) = patch.timezone {
        request.timezone = timezone;
    }
    if let Some(start) = patch.start {
        request.start = start;
    }
    if let Some(end) = patch.end {
        request.end = Some(end);
        request.duration_seconds = None;
    }
    if let Some(duration) = patch.duration_seconds {
        request.duration_seconds = Some(duration);
        request.end = None;
    }
    if let Some(recurrence_rule) = patch.recurrence_rule {
        request.recurrence_rule = recurrence_rule;
    }
    if let Some(lead_in_seconds) = patch.lead_in_seconds {
        request.lead_in_seconds = Some(lead_in_seconds);
    }
    if let Some(cooldown_seconds) = patch.cooldown_seconds {
        request.cooldown_seconds = Some(cooldown_seconds);
    }
    if let Some(disruptive) = patch.disruptive {
        request.disruptive = Some(disruptive);
    }
    if let Some(notification_policy) = patch.notification_policy {
        request.notification_policy = Some(notification_policy);
    }
    if let Some(owner) = patch.owner {
        request.owner = Some(owner);
    }
    if let Some(source) = patch.source {
        request.source = Some(source);
    }
    if let Some(notes) = patch.notes {
        request.notes = Some(notes);
    }
    if let Some(links) = patch.links {
        request.links = Some(links);
    }
    if let Some(resources) = patch.resources {
        request.resources = resources;
    }
    let next_state = patch_state(&current.state, patch.state)?;
    request.state = Some(next_state);
    normalize_event(&request)
}

fn patch_state(current: &str, requested: Option<String>) -> Result<String, String> {
    let Some(requested) = requested else {
        return Ok(current.to_string());
    };
    if requested != current && !legal_transition(current, &requested) {
        return Err(format!(
            "illegal maintenance transition from {current} to {requested}"
        ));
    }
    Ok(requested)
}

async fn lock_maintenance_mutation(tx: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(MAINTENANCE_ADVISORY_LOCK_KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_normalized_event(
    tx: &mut Transaction<'_, Postgres>,
    event: &NormalizedEvent,
    actor_user_id: Uuid,
) -> sqlx::Result<Uuid> {
    let event_id: Uuid = sqlx::query_scalar(
        "insert into maintenance_events \
             (name, description, timezone, start_at, end_at, recurrence_rule, \
              lead_in_seconds, cooldown_seconds, disruptive, notification_policy, \
              owner, source, notes, links, state, created_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
         returning id",
    )
    .bind(&event.name)
    .bind(&event.description)
    .bind(&event.timezone)
    .bind(timestamp_to_db(event.start).map_err(db_error)?)
    .bind(timestamp_to_db(event.end).map_err(db_error)?)
    .bind(&event.recurrence_rule)
    .bind(event.lead_in_seconds)
    .bind(event.cooldown_seconds)
    .bind(event.disruptive)
    .bind(&event.notification_policy)
    .bind(&event.owner)
    .bind(&event.source)
    .bind(&event.notes)
    .bind(json!(event.links))
    .bind(&event.state)
    .bind(actor_user_id)
    .fetch_one(&mut **tx)
    .await?;
    persist_resources_and_occurrences(tx, event_id, event).await?;
    Ok(event_id)
}

async fn persist_resources_and_occurrences(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    event: &NormalizedEvent,
) -> sqlx::Result<()> {
    for resource in &event.resources {
        sqlx::query(
            "insert into maintenance_resources \
                (event_id, role, resource_kind, resource_id, resource_key, expected_failure) \
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(event_id)
        .bind(&resource.role)
        .bind(&resource.kind)
        .bind(resource.id)
        .bind(&resource.key)
        .bind(resource.expected_failure)
        .execute(&mut **tx)
        .await?;
    }
    let mut occurrences = expand_occurrences(event).map_err(db_error)?;
    apply_occurrence_ids(event_id, &mut occurrences);
    for occurrence in occurrences {
        sqlx::query(
            "insert into maintenance_occurrences \
                (id, event_id, occurrence_key, occurrence_index, start_at, end_at, \
                 reservation_start, reservation_end, timezone) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(occurrence.id)
        .bind(event_id)
        .bind(&occurrence.occurrence_key)
        .bind(occurrence.occurrence_index)
        .bind(timestamp_to_db(occurrence.start).map_err(db_error)?)
        .bind(timestamp_to_db(occurrence.end).map_err(db_error)?)
        .bind(timestamp_to_db(occurrence.reservation_start).map_err(db_error)?)
        .bind(timestamp_to_db(occurrence.reservation_end).map_err(db_error)?)
        .bind(&occurrence.timezone)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn load_dependency_edges(
    tx: &mut Transaction<'_, Postgres>,
) -> sqlx::Result<Vec<DependencyEdgeValue>> {
    let rows: Vec<(String, Uuid, String, Uuid)> = sqlx::query_as(
        "select provider_kind, provider_id, consumer_kind, consumer_id \
         from dependency_edges \
         where confirmation_state = 'confirmed' \
         order by provider_kind, provider_id, consumer_kind, consumer_id \
         limit $1",
    )
    .bind(i64::try_from(MAX_GRAPH_EDGES + 1).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > MAX_GRAPH_EDGES {
        return Err(db_error(
            "confirmed dependency graph exceeds maintenance bound",
        ));
    }
    Ok(rows
        .into_iter()
        .map(
            |(provider_kind, provider_id, consumer_kind, consumer_id)| DependencyEdgeValue {
                provider_kind,
                provider_id,
                consumer_kind,
                consumer_id,
            },
        )
        .collect())
}

async fn load_event_snapshots(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Option<Uuid>,
) -> sqlx::Result<Vec<EventSnapshot>> {
    let headers: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
        "select id, name, state, disruptive \
         from maintenance_events \
         where state not in ('completed', 'cancelled') \
           and ($1::uuid is null or id = $1) \
         order by id \
         limit $2",
    )
    .bind(event_id)
    .bind(MAX_EVENT_COUNT + 1)
    .fetch_all(&mut **tx)
    .await?;
    if headers.len() > usize::try_from(MAX_EVENT_COUNT).unwrap_or(0) {
        return Err(db_error("maintenance event count exceeds conflict bound"));
    }
    if headers.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = headers.iter().map(|header| header.0).collect();
    let resource_rows = sqlx::query_as::<_, MaintenanceResourceRow>(
        "select event_id, role, resource_kind, resource_id, resource_key, expected_failure \
         from maintenance_resources where event_id = any($1::uuid[]) order by event_id, id \
         limit $2",
    )
    .bind(&ids)
    .bind(MAX_RESOURCE_COUNT as i64 * MAX_EVENT_COUNT)
    .fetch_all(&mut **tx)
    .await?;
    let occurrence_rows = sqlx::query_as::<_, MaintenanceOccurrenceRow>(
        "select id, event_id, occurrence_key, occurrence_index, start_at, end_at, \
                reservation_start, reservation_end, timezone \
         from maintenance_occurrences where event_id = any($1::uuid[]) \
         order by event_id, occurrence_index, id limit $2",
    )
    .bind(&ids)
    .bind(MAX_OCCURRENCE_COUNT + 1)
    .fetch_all(&mut **tx)
    .await?;
    if occurrence_rows.len() > usize::try_from(MAX_OCCURRENCE_COUNT).unwrap_or(0) {
        return Err(db_error(
            "maintenance occurrence count exceeds conflict bound",
        ));
    }
    let mut resources_by_event: BTreeMap<Uuid, Vec<ResourceValue>> = BTreeMap::new();
    for row in resource_rows {
        let resources = resources_by_event.entry(row.event_id).or_default();
        if resources.len() >= MAX_RESOURCE_COUNT {
            return Err(db_error("maintenance resource count exceeds event bound"));
        }
        resources.push(resource_row_to_value(row));
    }
    let mut occurrences_by_event: BTreeMap<Uuid, Vec<OccurrenceValue>> = BTreeMap::new();
    for row in occurrence_rows {
        occurrences_by_event
            .entry(row.event_id)
            .or_default()
            .push(occurrence_row_to_value(row).map_err(db_error)?);
    }
    headers
        .into_iter()
        .map(|(id, name, state, disruptive)| {
            Ok(EventSnapshot {
                id,
                name,
                state,
                disruptive,
                occurrences: occurrences_by_event.remove(&id).unwrap_or_default(),
                resources: resources_by_event.remove(&id).unwrap_or_default(),
            })
        })
        .collect()
}

async fn conflicts_for_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
) -> sqlx::Result<Vec<Conflict>> {
    let snapshots = load_event_snapshots(tx, None).await?;
    let Some(target) = snapshots.iter().find(|event| event.id == event_id) else {
        return Ok(Vec::new());
    };
    let edges = load_dependency_edges(tx).await?;
    let mut conflicts = Vec::new();
    for other in &snapshots {
        if other.id == event_id {
            continue;
        }
        conflicts.extend(conflicts_between(target, other, &edges).map_err(db_error)?);
        if conflicts.len() >= MAX_CONFLICT_COUNT {
            break;
        }
    }
    conflicts.truncate(MAX_CONFLICT_COUNT);
    Ok(conflicts)
}

fn event_value_from_row(row: &MaintenanceEventRow, resources: &[ResourceValue]) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "description": row.description,
        "timezone": row.timezone,
        "start_at": row.start_at,
        "end_at": row.end_at,
        "recurrence_rule": row.recurrence_rule,
        "lead_in_seconds": row.lead_in_seconds,
        "cooldown_seconds": row.cooldown_seconds,
        "disruptive": row.disruptive,
        "notification_policy": row.notification_policy,
        "owner": row.owner,
        "source": row.source,
        "notes": row.notes,
        "links": row.links,
        "state": row.state,
        "version": row.version,
        "resources": resources.iter().map(|resource| json!({
            "role": resource.role,
            "kind": resource.kind,
            "id": resource.id,
            "key": resource.key,
            "expected_failure": resource.expected_failure,
        })).collect::<Vec<_>>(),
    })
}

async fn event_json(pool: &PgPool, event_id: Uuid) -> sqlx::Result<Option<Value>> {
    sqlx::query_as::<_, (Value,)>(
        "select row_to_json(t) from ( \
             select e.*, \
                    coalesce((select jsonb_agg(to_jsonb(r) - 'event_id' order by r.id) \
                              from maintenance_resources r where r.event_id = e.id), '[]'::jsonb) as resources, \
                    coalesce((select jsonb_agg(to_jsonb(o) - 'event_id' order by o.occurrence_index, o.id) \
                              from maintenance_occurrences o where o.event_id = e.id), '[]'::jsonb) as occurrences \
               from maintenance_events e where e.id = $1 \
         ) t",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .map(|row| row.map(|(value,)| value))
}

#[derive(Debug)]
enum MaintenanceFailure {
    Validation(String),
    Conflict(Vec<Conflict>),
    Version(Option<i32>),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for MaintenanceFailure {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

fn failure_response(error: MaintenanceFailure) -> (StatusCode, Json<Value>) {
    match error {
        MaintenanceFailure::Validation(message) => validation_error(message),
        MaintenanceFailure::Conflict(conflicts) => conflict_error(conflicts),
        MaintenanceFailure::Version(version) => version_conflict(version),
        MaintenanceFailure::Db(error) => internal_error(error),
    }
}

async fn create_event_record(
    pool: &PgPool,
    request: MaintenanceEventRequest,
    user_id: Uuid,
) -> Result<Value, MaintenanceFailure> {
    let event = normalize_event(&request).map_err(MaintenanceFailure::Validation)?;
    let mut tx = pool.begin().await?;
    lock_maintenance_mutation(&mut tx).await?;
    let event_id = insert_normalized_event(&mut tx, &event, user_id).await?;
    let conflicts = conflicts_for_event_tx(&mut tx, event_id).await?;
    if !conflicts.is_empty() {
        return Err(MaintenanceFailure::Conflict(conflicts));
    }
    let Some((row, resources)) = load_event_row(&mut tx, event_id, false).await? else {
        return Err(MaintenanceFailure::Db(db_error(
            "created maintenance event disappeared before commit",
        )));
    };
    let after = event_value_from_row(&row, &resources);
    Recorder::record_change(
        &mut tx,
        "maintenance_events",
        event_id,
        "maintenance.created",
        "info",
        None,
        Some(after.clone()),
        Some("operator"),
    )
    .await?;
    Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        "maintenance.create",
        Some("maintenance_events"),
        Some(event_id),
        "success",
        Some(json!({"state": event.state})),
    )
    .await?;
    tx.commit().await?;
    event_json(pool, event_id)
        .await?
        .ok_or_else(|| MaintenanceFailure::Db(db_error("created maintenance event not found")))
}

async fn edit_event_record(
    pool: &PgPool,
    event_id: Uuid,
    patch: MaintenanceEventPatch,
    user_id: Uuid,
) -> Result<Value, MaintenanceFailure> {
    if patch.version < 1 {
        return Err(MaintenanceFailure::Validation(
            "version must be positive".to_string(),
        ));
    }
    let mut tx = pool.begin().await?;
    lock_maintenance_mutation(&mut tx).await?;
    let Some((row, resources)) = load_event_row(&mut tx, event_id, true).await? else {
        return Err(MaintenanceFailure::Validation(
            "maintenance event not found".to_string(),
        ));
    };
    if row.version != patch.version {
        return Err(MaintenanceFailure::Version(Some(row.version)));
    }
    if matches!(
        row.state.as_str(),
        "active" | "overrunning" | "completed" | "cancelled"
    ) {
        return Err(MaintenanceFailure::Validation(
            "only draft, scheduled, or upcoming maintenance events can be edited".to_string(),
        ));
    }
    let current =
        event_row_to_normalized(&row, resources.clone()).map_err(MaintenanceFailure::Validation)?;
    let event = patch_event(current, patch).map_err(MaintenanceFailure::Validation)?;
    sqlx::query(
        "update maintenance_events set name = $1, description = $2, timezone = $3, \
             start_at = $4, end_at = $5, recurrence_rule = $6, lead_in_seconds = $7, \
             cooldown_seconds = $8, disruptive = $9, notification_policy = $10, owner = $11, \
             source = $12, notes = $13, links = $14, version = version + 1, updated_at = now() \
         where id = $15 and version = $16",
    )
    .bind(&event.name)
    .bind(&event.description)
    .bind(&event.timezone)
    .bind(timestamp_to_db(event.start).map_err(db_error)?)
    .bind(timestamp_to_db(event.end).map_err(db_error)?)
    .bind(&event.recurrence_rule)
    .bind(event.lead_in_seconds)
    .bind(event.cooldown_seconds)
    .bind(event.disruptive)
    .bind(&event.notification_policy)
    .bind(&event.owner)
    .bind(&event.source)
    .bind(&event.notes)
    .bind(json!(event.links))
    .bind(event_id)
    .bind(row.version)
    .execute(&mut *tx)
    .await?;
    sqlx::query("delete from maintenance_resources where event_id = $1")
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from maintenance_occurrences where event_id = $1")
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
    persist_resources_and_occurrences(&mut tx, event_id, &event).await?;
    let conflicts = conflicts_for_event_tx(&mut tx, event_id).await?;
    if !conflicts.is_empty() {
        return Err(MaintenanceFailure::Conflict(conflicts));
    }
    let Some((updated_row, updated_resources)) = load_event_row(&mut tx, event_id, false).await?
    else {
        return Err(MaintenanceFailure::Db(db_error(
            "edited maintenance event disappeared before commit",
        )));
    };
    let before = event_value_from_row(&row, &resources);
    let after = event_value_from_row(&updated_row, &updated_resources);
    Recorder::record_change(
        &mut tx,
        "maintenance_events",
        event_id,
        "maintenance.edited",
        "info",
        Some(before),
        Some(after),
        Some("operator"),
    )
    .await?;
    Recorder::record_audit(
        &mut tx,
        Some(user_id),
        "operator",
        "maintenance.edit",
        Some("maintenance_events"),
        Some(event_id),
        "success",
        Some(json!({"version": updated_row.version})),
    )
    .await?;
    tx.commit().await?;
    event_json(pool, event_id)
        .await?
        .ok_or_else(|| MaintenanceFailure::Db(db_error("edited maintenance event not found")))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<MaintenanceEventRequest>,
) -> (StatusCode, Json<Value>) {
    match create_event_record(&state.pool, request, user.0).await {
        Ok(value) => (StatusCode::CREATED, Json(value)),
        Err(error) => failure_response(error),
    }
}

pub async fn patch(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<MaintenanceEventPatch>,
) -> (StatusCode, Json<Value>) {
    match edit_event_record(&state.pool, event_id, request, user.0).await {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => failure_response(error),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<MaintenanceListQuery>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(query.limit);
    let cursor = decode_cursor(&query.cursor);
    let rows = sqlx::query_as::<_, (Value,)>(
        "select row_to_json(t) from ( \
             select e.*, \
                    coalesce((select jsonb_agg(to_jsonb(r) - 'event_id' order by r.id) \
                              from maintenance_resources r where r.event_id = e.id), '[]'::jsonb) as resources, \
                    coalesce((select jsonb_agg(to_jsonb(o) - 'event_id' order by o.occurrence_index, o.id) \
                              from maintenance_occurrences o where o.event_id = e.id), '[]'::jsonb) as occurrences \
               from maintenance_events e \
              where ($1::text is null or e.state = $1) \
                and ($2::timestamptz is null or (e.created_at, e.id) < ($2::timestamptz, $3)) \
              order by e.created_at desc, e.id desc limit $4 \
         ) t",
    )
    .bind(query.state)
    .bind(cursor.as_ref().map(|(created_at, _)| created_at.clone()))
    .bind(cursor.as_ref().map(|(_, id)| *id))
    .bind(limit)
    .fetch_all(&state.pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows.into_iter().map(|(value,)| value).collect::<Vec<_>>(),
        Err(error) => return internal_error(error),
    };
    let next_cursor = rows.last().and_then(|value| {
        let created_at = value.get("created_at")?.as_str()?;
        let id = value.get("id")?.as_str()?.parse::<Uuid>().ok()?;
        Some(encode_cursor(created_at, id))
    });
    (
        StatusCode::OK,
        Json(json!({"items": rows, "next_cursor": next_cursor})),
    )
}

pub async fn get(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    match event_json(&state.pool, event_id).await {
        Ok(Some(value)) => (StatusCode::OK, Json(value)),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "maintenance event not found"),
        Err(error) => internal_error(error),
    }
}

pub async fn conflicts(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
) -> (StatusCode, Json<Value>) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => return internal_error(error),
    };
    let exists = match sqlx::query_scalar::<_, bool>(
        "select exists(select 1 from maintenance_events where id = $1)",
    )
    .bind(event_id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(exists) => exists,
        Err(error) => return internal_error(error),
    };
    if !exists {
        return error_response(StatusCode::NOT_FOUND, "maintenance event not found");
    }
    match conflicts_for_event_tx(&mut tx, event_id).await {
        Ok(conflicts) => (
            StatusCode::OK,
            Json(json!({"items": conflicts, "next_cursor": Value::Null})),
        ),
        Err(error) => internal_error(error),
    }
}

#[allow(clippy::too_many_arguments)]
async fn transition_event_record(
    pool: &PgPool,
    event_id: Uuid,
    expected_version: i32,
    target_state: &str,
    actor_user_id: Option<Uuid>,
    actor_kind: &str,
    action: &str,
    category: &str,
) -> Result<Value, MaintenanceFailure> {
    if expected_version < 1 {
        return Err(MaintenanceFailure::Validation(
            "version must be positive".to_string(),
        ));
    }
    let mut tx = pool.begin().await?;
    lock_maintenance_mutation(&mut tx).await?;
    let Some((row, resources)) = load_event_row(&mut tx, event_id, true).await? else {
        return Err(MaintenanceFailure::Validation(
            "maintenance event not found".to_string(),
        ));
    };
    if row.version != expected_version {
        return Err(MaintenanceFailure::Version(Some(row.version)));
    }
    if !legal_transition(&row.state, target_state) {
        return Err(MaintenanceFailure::Validation(format!(
            "illegal maintenance transition from {} to {target_state}",
            row.state
        )));
    }
    sqlx::query(
        "update maintenance_events set state = $1, version = version + 1, updated_at = now() \
         where id = $2 and version = $3",
    )
    .bind(target_state)
    .bind(event_id)
    .bind(expected_version)
    .execute(&mut *tx)
    .await?;
    let before = event_value_from_row(&row, &resources);
    let Some((updated_row, updated_resources)) = load_event_row(&mut tx, event_id, false).await?
    else {
        return Err(MaintenanceFailure::Db(db_error(
            "maintenance event disappeared during transition",
        )));
    };
    let after = event_value_from_row(&updated_row, &updated_resources);
    Recorder::record_change(
        &mut tx,
        "maintenance_events",
        event_id,
        category,
        "info",
        Some(before),
        Some(after),
        Some(actor_kind),
    )
    .await?;
    Recorder::record_audit(
        &mut tx,
        actor_user_id,
        actor_kind,
        action,
        Some("maintenance_events"),
        Some(event_id),
        "success",
        Some(json!({"state": target_state, "version": updated_row.version})),
    )
    .await?;
    tx.commit().await?;
    event_json(pool, event_id)
        .await?
        .ok_or_else(|| MaintenanceFailure::Db(db_error("maintenance event not found")))
}

pub async fn start(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<LifecycleRequest>,
) -> (StatusCode, Json<Value>) {
    match transition_event_record(
        &state.pool,
        event_id,
        request.version,
        "active",
        Some(user.0),
        "operator",
        "maintenance.start",
        "maintenance.started",
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => failure_response(error),
    }
}

pub async fn complete(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<LifecycleRequest>,
) -> (StatusCode, Json<Value>) {
    match transition_event_record(
        &state.pool,
        event_id,
        request.version,
        "completed",
        Some(user.0),
        "operator",
        "maintenance.complete",
        "maintenance.completed",
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => failure_response(error),
    }
}

pub async fn cancel(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    Extension(user): Extension<CurrentUser>,
    Json(request): Json<LifecycleRequest>,
) -> (StatusCode, Json<Value>) {
    match transition_event_record(
        &state.pool,
        event_id,
        request.version,
        "cancelled",
        Some(user.0),
        "operator",
        "maintenance.cancel",
        "maintenance.cancelled",
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => failure_response(error),
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ActiveImpactRow {
    event_id: Uuid,
    occurrence_id: Uuid,
    name: String,
    state: String,
    resource_role: String,
    resource_kind: Option<String>,
    resource_id: Option<Uuid>,
    resource_key: Option<String>,
    expected_failure: bool,
}

fn impact_reason(
    service_id: Uuid,
    event_id: Uuid,
    resource: &ResourceValue,
    path: Option<&dependency_graph::DependencyPath>,
) -> String {
    match path {
        Some(path) => format!(
            "service {service_id} has confirmed upstream dependency {} / {} through edge path {:?}; maintenance event {event_id} reserves resource {}",
            path.provider_kind,
            path.provider_id,
            path.edge_ids,
            resource_identity(resource),
        ),
        None => format!(
            "maintenance event {event_id} reserves resource {} for service {service_id}",
            resource_identity(resource),
        ),
    }
}

pub async fn active_maintenance_for_service(
    pool: &sqlx::PgPool,
    service_id: Uuid,
) -> sqlx::Result<Vec<ActiveMaintenanceImpact>> {
    if service_id.is_nil() {
        return Ok(Vec::new());
    }
    let paths = dependency_graph::upstream_dependency_paths(pool, "services", service_id).await?;
    let mut nodes = BTreeMap::<EntityNode, Option<dependency_graph::DependencyPath>>::new();
    nodes.insert(
        EntityNode {
            kind: "services".to_string(),
            id: service_id,
        },
        None,
    );
    for path in paths {
        nodes
            .entry(EntityNode {
                kind: path.provider_kind.clone(),
                id: path.provider_id,
            })
            .or_insert(Some(path));
        if nodes.len() >= MAX_SERVICE_IMPACTS {
            break;
        }
    }
    let rows = sqlx::query_as::<_, ActiveImpactRow>(
        "select e.id as event_id, o.id as occurrence_id, e.name, e.state, \
                r.role as resource_role, r.resource_kind, r.resource_id, r.resource_key, \
                r.expected_failure \
           from maintenance_events e \
           join maintenance_occurrences o on o.event_id = e.id \
           join maintenance_resources r on r.event_id = e.id \
          where e.state in ('active', 'overrunning') \
            and o.start_at <= now() \
            and (o.reservation_end >= now() or e.state = 'overrunning') \
          order by o.start_at desc, e.id, r.id \
          limit $1",
    )
    .bind(i64::try_from(MAX_SERVICE_IMPACTS * 32 + 1).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    if rows.len() > MAX_SERVICE_IMPACTS * 32 {
        return Err(db_error("active maintenance impact count exceeds bound"));
    }
    let mut impacts = Vec::new();
    for row in rows {
        let resource = ResourceValue {
            role: row.resource_role.clone(),
            kind: row.resource_kind.clone(),
            id: row.resource_id,
            key: row.resource_key.clone(),
            expected_failure: row.expected_failure,
        };
        let Some(kind) = row.resource_kind.as_deref() else {
            continue;
        };
        let Some(resource_id) = row.resource_id else {
            continue;
        };
        let node = EntityNode {
            kind: kind.to_string(),
            id: resource_id,
        };
        let Some(path) = nodes.get(&node) else {
            continue;
        };
        impacts.push(ActiveMaintenanceImpact {
            event_id: row.event_id,
            occurrence_id: row.occurrence_id,
            name: row.name,
            state: row.state,
            resource_role: row.resource_role,
            resource_kind: row.resource_kind,
            resource_id: row.resource_id,
            resource_key: row.resource_key,
            expected_failure: row.expected_failure,
            reason: impact_reason(service_id, row.event_id, &resource, path.as_ref()),
        });
        if impacts.len() >= MAX_SERVICE_IMPACTS {
            break;
        }
    }
    Ok(impacts)
}

fn expected_service_ids(
    resources: &[ResourceValue],
    edges: &[DependencyEdgeValue],
) -> BTreeSet<Uuid> {
    let mut adjacency: BTreeMap<EntityNode, Vec<EntityNode>> = BTreeMap::new();
    for edge in edges.iter().take(MAX_GRAPH_EDGES) {
        adjacency
            .entry(EntityNode {
                kind: edge.provider_kind.clone(),
                id: edge.provider_id,
            })
            .or_default()
            .push(EntityNode {
                kind: edge.consumer_kind.clone(),
                id: edge.consumer_id,
            });
    }
    let mut services = BTreeSet::new();
    let mut queue = VecDeque::new();
    let mut visited = BTreeSet::new();
    for resource in resources
        .iter()
        .filter(|resource| resource.expected_failure)
    {
        let (Some(kind), Some(id)) = (resource.kind.as_deref(), resource.id) else {
            continue;
        };
        let node = EntityNode {
            kind: kind.to_string(),
            id,
        };
        if visited.insert(node.clone()) {
            queue.push_back((node, 0_usize));
        }
    }
    while let Some((node, depth)) = queue.pop_front() {
        if node.kind == "services" {
            services.insert(node.id);
            if services.len() >= MAX_SERVICE_IMPACTS {
                break;
            }
        }
        if depth >= MAX_GRAPH_DEPTH {
            continue;
        }
        if let Some(children) = adjacency.get(&node) {
            for child in children {
                if visited.insert(child.clone()) {
                    queue.push_back((child.clone(), depth + 1));
                }
            }
        }
    }
    services
}

async fn has_open_expected_incident(
    tx: &mut Transaction<'_, Postgres>,
    resources: &[ResourceValue],
    edges: &[DependencyEdgeValue],
) -> sqlx::Result<bool> {
    let service_ids = expected_service_ids(resources, edges);
    if service_ids.is_empty() {
        return Ok(false);
    }
    sqlx::query_scalar(
        "select exists( \
             select 1 from incidents i \
             join monitors m on m.id = i.monitor_id \
             where i.state = 'open' and m.service_id = any($1::uuid[]) \
         )",
    )
    .bind(service_ids.into_iter().collect::<Vec<_>>())
    .fetch_one(&mut **tx)
    .await
}

fn desired_reconcile_state(
    current_state: &str,
    occurrences: &[OccurrenceValue],
    now: Timestamp,
    expected_health_bad: bool,
) -> Option<&'static str> {
    if current_state == "draft" || matches!(current_state, "completed" | "cancelled") {
        return None;
    }
    let mut current_or_past: Option<&OccurrenceValue> = None;
    let mut next: Option<&OccurrenceValue> = None;
    for occurrence in occurrences {
        if occurrence.start <= now {
            current_or_past = Some(occurrence);
        } else if next.is_none() {
            next = Some(occurrence);
        }
    }
    if let Some(occurrence) = current_or_past {
        if occurrence.end > now {
            return Some("active");
        }
        if expected_health_bad {
            return if matches!(current_state, "active" | "overrunning") {
                Some("overrunning")
            } else {
                Some("active")
            };
        }
        if matches!(current_state, "active" | "overrunning") {
            return Some("completed");
        }
        return Some("active");
    }
    if next.is_some_and(|occurrence| occurrence.reservation_start <= now) {
        Some("upcoming")
    } else {
        Some("scheduled")
    }
}

pub async fn reconcile(pool: &PgPool) -> sqlx::Result<usize> {
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;
    lock_maintenance_mutation(&mut tx).await?;
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "select id from maintenance_events \
         where state in ('scheduled', 'upcoming', 'active', 'overrunning') \
         order by id limit $1",
    )
    .bind(MAX_EVENT_COUNT)
    .fetch_all(&mut *tx)
    .await?;
    let edges = load_dependency_edges(&mut tx).await?;
    let mut changed = 0_usize;
    for (event_id,) in ids {
        let Some((row, resources)) = load_event_row(&mut tx, event_id, true).await? else {
            continue;
        };
        let occurrence_rows = sqlx::query_as::<_, MaintenanceOccurrenceRow>(
            "select id, event_id, occurrence_key, occurrence_index, start_at, end_at, \
                    reservation_start, reservation_end, timezone \
             from maintenance_occurrences where event_id = $1 order by occurrence_index, id \
             limit $2",
        )
        .bind(event_id)
        .bind(MAX_OCCURRENCE_COUNT + 1)
        .fetch_all(&mut *tx)
        .await?;
        if occurrence_rows.len() > usize::try_from(MAX_OCCURRENCE_COUNT).unwrap_or(0) {
            return Err(db_error(
                "maintenance occurrence count exceeds reconcile bound",
            ));
        }
        let occurrences = occurrence_rows
            .into_iter()
            .map(occurrence_row_to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let expected_health_bad = has_open_expected_incident(&mut tx, &resources, &edges).await?;
        let Some(target_state) =
            desired_reconcile_state(&row.state, &occurrences, now, expected_health_bad)
        else {
            continue;
        };
        if target_state == row.state || !legal_transition(&row.state, target_state) {
            continue;
        }
        sqlx::query(
            "update maintenance_events set state = $1, version = version + 1, updated_at = now() \
             where id = $2 and version = $3",
        )
        .bind(target_state)
        .bind(event_id)
        .bind(row.version)
        .execute(&mut *tx)
        .await?;
        if target_state == "overrunning" {
            let occurrence = occurrences
                .iter()
                .filter(|occurrence| occurrence.start <= now && occurrence.end <= now)
                .max_by_key(|occurrence| occurrence.start)
                .ok_or_else(|| db_error("overrunning maintenance event has no ended occurrence"))?;
            notifications::enqueue_maintenance_overrun_notifications(
                &mut tx,
                event_id,
                occurrence.id,
                &row.name,
                &occurrence.end.to_string(),
            )
            .await
            .map_err(|error| db_error(error.to_string()))?;
            sqlx::query(
                "update maintenance_notification_deliveries \
                    set payload = payload || jsonb_build_object('notification_policy', $1), \
                        updated_at = now() \
                  where maintenance_event_id = $2 and maintenance_occurrence_id = $3",
            )
            .bind(&row.notification_policy)
            .bind(event_id)
            .bind(occurrence.id)
            .execute(&mut *tx)
            .await?;
        }
        let before = event_value_from_row(&row, &resources);
        let Some((updated_row, updated_resources)) =
            load_event_row(&mut tx, event_id, false).await?
        else {
            return Err(db_error("maintenance event disappeared during reconcile"));
        };
        let after = event_value_from_row(&updated_row, &updated_resources);
        let (category, severity) = if target_state == "overrunning" {
            ("maintenance.overrun", "warning")
        } else {
            ("maintenance.state_changed", "info")
        };
        Recorder::record_change(
            &mut tx,
            "maintenance_events",
            event_id,
            category,
            severity,
            Some(before),
            Some(after),
            Some("worker"),
        )
        .await?;
        Recorder::record_audit(
            &mut tx,
            None,
            "worker",
            if target_state == "overrunning" {
                "maintenance.overrun"
            } else {
                "maintenance.reconcile"
            },
            Some("maintenance_events"),
            Some(event_id),
            "success",
            Some(json!({"state": target_state, "expected_health_bad": expected_health_bad})),
        )
        .await?;
        changed += 1;
    }
    tx.commit().await?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurrence_expands_in_local_timezone_across_dst() {
        let event = test_event(
            "America/New_York",
            "2024-03-08T09:00:00",
            "2024-03-08T10:00:00",
            Some("FREQ=DAILY;COUNT=4"),
        );
        let occurrences = expand_occurrences(&event).expect("valid recurrence");
        assert_eq!(occurrences.len(), 4);
        assert_eq!(occurrences[0].start.to_string(), "2024-03-08T14:00:00Z");
        assert_eq!(occurrences[2].start.to_string(), "2024-03-10T13:00:00Z");
    }

    #[test]
    fn ambiguous_and_nonexistent_local_times_are_rejected() {
        assert!(parse_schedule_time("2024-11-03T01:30:00", "America/New_York").is_err());
        assert!(parse_schedule_time("2024-03-10T02:30:00", "America/New_York").is_err());
    }

    #[test]
    fn conflict_explanation_contains_overlap_and_later_move() {
        let first = snapshot(
            Uuid::from_u128(1),
            "UDM Pro firmware",
            false,
            Uuid::from_u128(11),
            0,
            20,
            "affected",
            "network-core",
        );
        let second = snapshot(
            Uuid::from_u128(2),
            "crypt maintenance",
            false,
            Uuid::from_u128(22),
            15,
            30,
            "required",
            "network-core",
        );
        let conflicts = conflicts_between(&first, &second, &[]).expect("conflict");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].relationship, "affected_required");
        assert!(conflicts[0].reason.contains("16:15"));
        assert_eq!(conflicts[0].suggested_move.event_id, second.id);
    }

    #[test]
    fn lifecycle_state_machine_rejects_illegal_transitions() {
        assert!(legal_transition("scheduled", "active"));
        assert!(legal_transition("upcoming", "active"));
        assert!(legal_transition("active", "overrunning"));
        assert!(legal_transition("active", "completed"));
        assert!(legal_transition("scheduled", "cancelled"));
        assert!(!legal_transition("completed", "active"));
        assert!(!legal_transition("cancelled", "scheduled"));
    }

    #[test]
    fn healthy_past_occurrence_completes_active_event() {
        let event = test_event("UTC", "2024-03-09T16:00:00Z", "2024-03-09T16:20:00Z", None);
        let mut occurrences = expand_occurrences(&event).expect("valid occurrence");
        let now = Timestamp::from_second(1_710_000_000 + 30 * 60).expect("timestamp");
        assert_eq!(
            desired_reconcile_state("active", &occurrences, now, false),
            Some("completed")
        );
        assert_eq!(
            desired_reconcile_state("active", &occurrences, now, true),
            Some("overrunning")
        );
        occurrences[0].end = now;
        assert_eq!(
            desired_reconcile_state("scheduled", &occurrences, now, false),
            Some("active")
        );
    }

    #[test]
    fn occurrence_identity_is_stable() {
        let event_id = Uuid::from_u128(99);
        let first = stable_occurrence_id(event_id, "2024-03-08T14:00:00Z");
        let second = stable_occurrence_id(event_id, "2024-03-08T14:00:00Z");
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn maintenance_db_create_edit_conflict_and_version_are_atomic() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let user_id = test_user(&pool).await;
        let key = format!("maintenance-db-{}", Uuid::new_v4());
        let start = whole_now().checked_add(Span::new().days(7)).expect("start");
        let end = start.checked_add(Span::new().minutes(20)).expect("end");

        let first = create_event_record(
            &pool,
            db_request(
                "maintenance-db-first",
                start,
                end,
                vec![db_resource("affected", &key)],
            ),
            user_id,
        )
        .await
        .expect("first event");
        let first_id = value_uuid(&first, "id");

        let second_name = format!("maintenance-db-conflict-{}", Uuid::new_v4());
        let conflict = create_event_record(
            &pool,
            db_request(
                &second_name,
                start.checked_add(Span::new().minutes(5)).expect("start"),
                end.checked_add(Span::new().minutes(5)).expect("end"),
                vec![db_resource("required", &key)],
            ),
            user_id,
        )
        .await;
        assert!(matches!(conflict, Err(MaintenanceFailure::Conflict(_))));
        let conflict_rows: i64 =
            sqlx::query_scalar("select count(*) from maintenance_events where name = $1")
                .bind(&second_name)
                .fetch_one(&pool)
                .await
                .expect("conflict rollback");
        assert_eq!(conflict_rows, 0);

        let editable_key = format!("maintenance-db-edit-{}", Uuid::new_v4());
        let editable = create_event_record(
            &pool,
            db_request(
                "maintenance-db-editable",
                start,
                end,
                vec![db_resource("required", &editable_key)],
            ),
            user_id,
        )
        .await
        .expect("editable event");
        let editable_id = value_uuid(&editable, "id");
        let edit_conflict = edit_event_record(
            &pool,
            editable_id,
            MaintenanceEventPatch {
                version: 1,
                name: None,
                description: None,
                timezone: None,
                start: None,
                end: None,
                duration_seconds: None,
                recurrence_rule: None,
                lead_in_seconds: None,
                cooldown_seconds: None,
                disruptive: None,
                notification_policy: None,
                owner: None,
                source: None,
                notes: None,
                links: None,
                resources: Some(vec![db_resource("required", &key)]),
                state: None,
            },
            user_id,
        )
        .await;
        assert!(matches!(
            edit_conflict,
            Err(MaintenanceFailure::Conflict(_))
        ));
        let version: i32 =
            sqlx::query_scalar("select version from maintenance_events where id = $1")
                .bind(editable_id)
                .fetch_one(&pool)
                .await
                .expect("editable version");
        assert_eq!(version, 1);
        let stored_key: String = sqlx::query_scalar(
            "select resource_key from maintenance_resources where event_id = $1",
        )
        .bind(editable_id)
        .fetch_one(&pool)
        .await
        .expect("editable resource");
        assert_eq!(stored_key, editable_key);

        let version_error = edit_event_record(
            &pool,
            first_id,
            MaintenanceEventPatch {
                version: 99,
                name: None,
                description: None,
                timezone: None,
                start: None,
                end: None,
                duration_seconds: None,
                recurrence_rule: None,
                lead_in_seconds: None,
                cooldown_seconds: None,
                disruptive: None,
                notification_policy: None,
                owner: None,
                source: None,
                notes: None,
                links: None,
                resources: None,
                state: None,
            },
            user_id,
        )
        .await;
        assert!(matches!(
            version_error,
            Err(MaintenanceFailure::Version(Some(1)))
        ));
    }

    #[tokio::test]
    async fn maintenance_db_reconcile_overrun_notification_and_service_seam() {
        let Some(pool) = pool_or_skip().await else {
            return;
        };
        let user_id = test_user(&pool).await;
        let device_id: Uuid = sqlx::query_scalar(
            "insert into devices (device_type, name) values ('unknown', $1) returning id",
        )
        .bind(format!("maintenance-db-device-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("device");
        let service_id: Uuid = sqlx::query_scalar(
            "insert into services (name, owner_kind, owner_id) values ($1, 'device', $2) returning id",
        )
        .bind(format!("maintenance-db-service-{}", Uuid::new_v4()))
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .expect("service");
        let endpoint_id: Uuid = sqlx::query_scalar(
            "insert into endpoints (service_id, endpoint_type, address, port) \
             values ($1, 'socket', $2::inet, 443) returning id",
        )
        .bind(service_id)
        .bind("127.0.0.1")
        .fetch_one(&pool)
        .await
        .expect("endpoint");
        let monitor_id: Uuid = sqlx::query_scalar(
            "insert into monitors (service_id, endpoint_id, monitor_type, state) \
             values ($1, $2, 'tcp', 'down') returning id",
        )
        .bind(service_id)
        .bind(endpoint_id)
        .fetch_one(&pool)
        .await
        .expect("monitor");
        sqlx::query(
            "insert into incidents (monitor_id, state, severity, failure_count, summary) \
             values ($1, 'open', 'critical', 1, 'maintenance test incident')",
        )
        .bind(monitor_id)
        .execute(&pool)
        .await
        .expect("incident");
        let channel_id: Uuid = sqlx::query_scalar(
            "insert into notification_channels (name, provider, config) \
             values ($1, 'webhook', '{\"url\":\"https://example.invalid/maintenance\"}'::jsonb) \
             returning id",
        )
        .bind(format!("maintenance-db-channel-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("notification channel");
        sqlx::query(
            "insert into notification_routes (channel_id, min_severity, event_types) \
             values ($1, 'warning', '[\"maintenance.overrun\"]'::jsonb)",
        )
        .bind(channel_id)
        .execute(&pool)
        .await
        .expect("notification route");

        let now = whole_now();
        let start = now.checked_sub(Span::new().hours(2)).expect("start");
        let end = now.checked_sub(Span::new().hours(1)).expect("end");
        let event = create_event_record(
            &pool,
            db_request(
                "maintenance-db-overrun",
                start,
                end,
                vec![MaintenanceResourceRequest {
                    role: "affected".to_string(),
                    kind: Some("services".to_string()),
                    id: Some(service_id),
                    key: None,
                    expected_failure: true,
                }],
            ),
            user_id,
        )
        .await
        .expect("overrun event");
        let event_id = value_uuid(&event, "id");

        reconcile(&pool).await.expect("active reconcile");
        let state: String =
            sqlx::query_scalar("select state from maintenance_events where id = $1")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .expect("active state");
        assert_eq!(state, "active");
        let impacts = active_maintenance_for_service(&pool, service_id)
            .await
            .expect("active impact");
        assert!(impacts.iter().any(|impact| impact.event_id == event_id));
        assert!(
            active_maintenance_for_service(&pool, Uuid::new_v4())
                .await
                .expect("unrelated service")
                .is_empty()
        );

        reconcile(&pool).await.expect("overrun reconcile");
        let state: String =
            sqlx::query_scalar("select state from maintenance_events where id = $1")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .expect("overrun state");
        assert_eq!(state, "overrunning");
        let delivery_count: i64 = sqlx::query_scalar(
            "select count(*) from maintenance_notification_deliveries \
             where maintenance_event_id = $1 and channel_id = $2",
        )
        .bind(event_id)
        .bind(channel_id)
        .fetch_one(&pool)
        .await
        .expect("overrun delivery");
        assert_eq!(delivery_count, 1);
        reconcile(&pool).await.expect("idempotent reconcile");
        let delivery_count_after: i64 = sqlx::query_scalar(
            "select count(*) from maintenance_notification_deliveries \
             where maintenance_event_id = $1 and channel_id = $2",
        )
        .bind(event_id)
        .bind(channel_id)
        .fetch_one(&pool)
        .await
        .expect("idempotent delivery");
        assert_eq!(delivery_count_after, 1);
    }

    async fn pool_or_skip() -> Option<PgPool> {
        let Ok(url) = env::var("DATABASE_URL") else {
            eprintln!("skipping: DATABASE_URL not set");
            return None;
        };
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("apply migrations");
        Some(pool)
    }

    async fn test_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, 'maintenance-test') returning id",
        )
        .bind(format!("maintenance-{}@example.invalid", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .expect("test user")
    }

    fn whole_now() -> Timestamp {
        Timestamp::from_second(Timestamp::now().as_second()).expect("whole timestamp")
    }

    fn db_resource(role: &str, key: &str) -> MaintenanceResourceRequest {
        MaintenanceResourceRequest {
            role: role.to_string(),
            kind: None,
            id: None,
            key: Some(key.to_string()),
            expected_failure: false,
        }
    }

    fn db_request(
        name: &str,
        start: Timestamp,
        end: Timestamp,
        resources: Vec<MaintenanceResourceRequest>,
    ) -> MaintenanceEventRequest {
        MaintenanceEventRequest {
            name: name.to_string(),
            description: None,
            timezone: "UTC".to_string(),
            start: start.to_string(),
            end: Some(end.to_string()),
            duration_seconds: None,
            recurrence_rule: None,
            lead_in_seconds: Some(0),
            cooldown_seconds: Some(0),
            disruptive: Some(false),
            notification_policy: Some(json!({})),
            owner: Some("maintenance-test".to_string()),
            source: Some("maintenance-test".to_string()),
            notes: None,
            links: Some(Vec::new()),
            resources,
            state: Some("scheduled".to_string()),
        }
    }

    fn value_uuid(value: &Value, field: &str) -> Uuid {
        value
            .get(field)
            .and_then(Value::as_str)
            .and_then(|value| value.parse().ok())
            .expect("UUID response field")
    }

    fn test_event(
        timezone: &str,
        start: &str,
        end: &str,
        recurrence_rule: Option<&str>,
    ) -> NormalizedEvent {
        NormalizedEvent {
            name: "test".to_string(),
            description: None,
            timezone: timezone.to_string(),
            start: parse_schedule_time(start, timezone).expect("test start"),
            end: parse_schedule_time(end, timezone).expect("test end"),
            recurrence_rule: recurrence_rule.map(str::to_string),
            lead_in_seconds: 0,
            cooldown_seconds: 0,
            disruptive: false,
            notification_policy: json!({}),
            owner: None,
            source: None,
            notes: None,
            links: Vec::new(),
            state: "scheduled".to_string(),
            resources: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn snapshot(
        id: Uuid,
        name: &str,
        disruptive: bool,
        occurrence_id: Uuid,
        start_minute: i64,
        end_minute: i64,
        role: &str,
        key: &str,
    ) -> EventSnapshot {
        let base = Timestamp::from_second(1_710_000_000).expect("timestamp");
        let start = base
            .checked_add(Span::new().minutes(start_minute))
            .expect("start");
        let end = base
            .checked_add(Span::new().minutes(end_minute))
            .expect("end");
        EventSnapshot {
            id,
            name: name.to_string(),
            state: "scheduled".to_string(),
            disruptive,
            occurrences: vec![OccurrenceValue {
                id: occurrence_id,
                occurrence_key: start.to_string(),
                occurrence_index: 0,
                start,
                end,
                reservation_start: start,
                reservation_end: end,
                timezone: "UTC".to_string(),
            }],
            resources: vec![ResourceValue {
                role: role.to_string(),
                kind: None,
                id: None,
                key: Some(key.to_string()),
                expected_failure: false,
            }],
        }
    }
}
