//! Explicit, scope-checked service-collector job requests.

use std::net::Ipv4Addr;
use std::str::FromStr;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use domain::discovery::ApprovedScope;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth_mw::CurrentUser;
use crate::inventory::events::Recorder;
use crate::state::AppState;

const JOB_TYPE: &str = "discovery.service_collectors";

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": message.into() })))
}

#[derive(Debug, Deserialize)]
pub struct CreateCollectorRun {
    /// One of the fixed multicast protocols implemented by the collector
    /// adapter. Destinations are never accepted from the request body.
    pub protocol: String,
    /// The local IPv4 interface used to join the multicast group. It must be
    /// inside the network's confirmed, enabled discovery scope.
    pub interface: String,
}

fn validate_protocol(protocol: &str) -> bool {
    matches!(protocol, "mdns" | "ssdp")
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(network_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<CreateCollectorRun>,
) -> (StatusCode, Json<Value>) {
    if !validate_protocol(&request.protocol) {
        return err(StatusCode::BAD_REQUEST, "protocol must be mdns or ssdp");
    }
    let protocol = request.protocol;
    let interface = match Ipv4Addr::from_str(&request.interface) {
        Ok(interface) => interface,
        Err(_) => return err(StatusCode::BAD_REQUEST, "interface must be an IPv4 address"),
    };
    let Some(idempotency_key) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
    else {
        return err(
            StatusCode::BAD_REQUEST,
            "Idempotency-Key header is required",
        );
    };
    if idempotency_key.len() > 255 {
        return err(StatusCode::BAD_REQUEST, "Idempotency-Key exceeds 255 bytes");
    }

    let scope: Option<(String, bool, bool, Value)> = match sqlx::query_as(
        "select n.cidr::text, coalesce(ds.enabled, false), \
                (ds.confirmed_at is not null), coalesce(ds.excluded_cidrs, '[]'::jsonb) \
         from networks n \
         left join discovery_scopes ds on ds.network_id = n.id \
         where n.id = $1",
    )
    .bind(network_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(scope) => scope,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let Some((cidr, enabled, confirmed, excluded_cidrs)) = scope else {
        return err(StatusCode::NOT_FOUND, "network not found");
    };
    if !enabled || !confirmed {
        return err(
            StatusCode::CONFLICT,
            "network needs an enabled, confirmed discovery scope",
        );
    }
    let excluded_cidrs: Vec<String> = match serde_json::from_value(excluded_cidrs) {
        Ok(excluded_cidrs) => excluded_cidrs,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let scope = match ApprovedScope::parse(&cidr, &excluded_cidrs) {
        Ok(scope) => scope,
        Err(error) => return err(StatusCode::CONFLICT, error.to_string()),
    };
    if !scope.cidr().contains(&interface)
        || scope
            .exclusions()
            .iter()
            .any(|excluded| excluded.contains(&interface))
    {
        return err(
            StatusCode::BAD_REQUEST,
            "collector interface is outside the confirmed discovery scope",
        );
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    let job_id = match jobs::enqueue_in(
        &mut transaction,
        JOB_TYPE,
        idempotency_key,
        json!({
            "network_id": network_id,
            "protocol": &protocol,
            "interface": interface.to_string(),
        }),
    )
    .await
    {
        Ok(job_id) => job_id,
        Err(error) => return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    if let Err(error) = Recorder::record_audit(
        &mut transaction,
        Some(user.0),
        "operator",
        "service_collector.create",
        Some("networks"),
        Some(network_id),
        "success",
        Some(json!({
            "job_id": job_id,
            "protocol": &protocol,
            "interface": interface.to_string(),
        })),
    )
    .await
    {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    if let Err(error) = transaction.commit().await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "job_id": job_id,
            "network_id": network_id,
            "protocol": &protocol,
            "interface": interface.to_string(),
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fixed_protocols_are_accepted() {
        assert!(validate_protocol("mdns"));
        assert!(validate_protocol("ssdp"));
        assert!(!validate_protocol("http"));
        assert!(!validate_protocol("239.255.255.250"));
    }
}
