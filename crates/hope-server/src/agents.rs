use crate::{
    error::{ApiError, ApiResult},
    state::{State, hash, now, secret},
};
use axum::{
    Json,
    extract::{Path, State as Extract},
    http::{HeaderMap, StatusCode},
};
use hope_protocol::{
    AgentReport, EnrollmentRequest, EnrollmentResponse, Host, Inventory, SCHEMA_VERSION,
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub async fn issue_token(state: &State) -> ApiResult<String> {
    let token = secret();
    sqlx::query("DELETE FROM enrollment_tokens WHERE expires_at < ?")
        .bind(now() - 86400.0)
        .execute(&state.0.db)
        .await?;
    sqlx::query("INSERT INTO enrollment_tokens(token_hash,expires_at) VALUES(?,?)")
        .bind(hash(&token))
        .bind(now() + 900.0)
        .execute(&state.0.db)
        .await?;
    Ok(token)
}
pub fn install_command(state: &State, token: &str) -> String {
    let server = shell_quote(&state.0.server_url);
    let ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|_| " HOPE_CA_CERT='/path/to/hope-ca.pem'")
        .unwrap_or("");
    let curl_ca = if state.0.enrollment_ca.is_some() {
        " --cacert '/path/to/hope-ca.pem'"
    } else {
        ""
    };
    format!(
        "( umask 077; installer=$(mktemp) || exit 1; trap 'rm -f \"$installer\"' EXIT; curl --fail --silent --show-error --location --proto '=https'{curl_ca} {server}/agent/install -o \"$installer\" && HOPE_SERVER={server} HOPE_ENROLLMENT_TOKEN={}{} sh \"$installer\" )",
        shell_quote(token),
        ca
    )
}
pub async fn token(Extract(state): Extract<State>) -> ApiResult<Json<serde_json::Value>> {
    let token = issue_token(&state).await?;
    Ok(Json(
        json!({"token":token,"expires_at":now()+900.0,"command":install_command(&state,&token),"custom_ca_required":state.0.enrollment_ca.is_some()}),
    ))
}

pub fn validate_inventory(inv: &Inventory) -> ApiResult<()> {
    if inv.hostname.is_empty()
        || inv.hostname.len() > 255
        || inv.interfaces.len() > 128
        || inv.disks.len() > 128
        || inv.gpus.len() > 32
        || inv.collectors.len() > 64
    {
        return Err(ApiError::bad("Inventory exceeds supported limits"));
    }
    if inv.interfaces.iter().any(|i| {
        i.addresses.len() > 64
            || i.addresses
                .iter()
                .any(|a| a.parse::<std::net::IpAddr>().is_err())
    }) {
        return Err(ApiError::bad("Invalid interface addresses"));
    }
    Ok(())
}
pub fn inventory_addresses(inv: &Inventory) -> Vec<String> {
    let mut addresses: Vec<_> = inv
        .interfaces
        .iter()
        .flat_map(|i| i.addresses.clone())
        .filter(|s| {
            s.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| !ip.is_loopback() && !ip.is_unspecified())
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}
pub fn inventory_macs(inv: &Inventory) -> Vec<String> {
    let mut macs: Vec<_> = inv
        .interfaces
        .iter()
        .filter_map(|i| i.mac.as_ref())
        .map(|m| m.to_lowercase())
        .filter(|s| s != "00:00:00:00:00:00")
        .collect();
    macs.sort();
    macs.dedup();
    macs
}

pub async fn enroll(
    Extract(state): Extract<State>,
    Json(request): Json<EnrollmentRequest>,
) -> ApiResult<Json<EnrollmentResponse>> {
    if request.schema_version != SCHEMA_VERSION || Uuid::parse_str(&request.agent_id).is_err() {
        return Err(ApiError::bad(
            "Unsupported schema or invalid agent identity",
        ));
    }
    validate_inventory(&request.inventory)?;
    let _guard = state.0.writes.lock().await;
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE id=?")
        .bind(&request.agent_id)
        .fetch_one(&state.0.db)
        .await?;
    if exists > 0 {
        return Err(ApiError(StatusCode::CONFLICT,"Agent identity is already enrolled; preserve its credential or explicitly reset its identity".into()));
    }
    let addresses = inventory_addresses(&request.inventory);
    let macs = inventory_macs(&request.inventory);
    let hosts = state.hosts().await?;
    // IP alone is insufficient: DHCP may have reassigned it since discovery.
    let candidates: Vec<_> = hosts
        .into_iter()
        .filter(|h| h.agent_id.is_none() && h.macs.iter().any(|m| macs.contains(m)))
        .collect();
    let mut host = if candidates.len() == 1 {
        candidates[0].clone()
    } else {
        Host {
            id: Uuid::new_v4().to_string(),
            agent_id: None,
            name: String::new(),
            addresses: vec![],
            macs: vec![],
            network_ids: vec![],
            last_seen: 0.0,
            last_report: None,
            discovery_method: "agent".into(),
            inventory: None,
            metrics: None,
            collectors: vec![],
            services: vec![],
        }
    };
    host.agent_id = Some(request.agent_id.clone());
    host.name = request.inventory.hostname.clone();
    host.addresses = addresses;
    host.macs = macs;
    host.inventory = Some(request.inventory);
    host.network_ids = crate::api::matching_networks(&state, &host.addresses).await?;
    let credential = secret();
    let mut tx = state.0.db.begin().await?;
    let changed=sqlx::query("UPDATE enrollment_tokens SET used_at=?,agent_id=? WHERE token_hash=? AND used_at IS NULL AND expires_at>?")
        .bind(now()).bind(&request.agent_id).bind(hash(&request.token)).bind(now()).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::unauthorized());
    }
    sqlx::query("INSERT INTO agents(id,credential_hash) VALUES(?,?)")
        .bind(&request.agent_id)
        .bind(hash(&credential))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO hosts(id,agent_id,name,body) VALUES(?,?,?,?) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,name=excluded.name,body=excluded.body")
        .bind(&host.id).bind(&host.agent_id).bind(&host.name).bind(serde_json::to_string(&host)?).execute(&mut *tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(EnrollmentResponse {
        credential,
        report_interval_seconds: 30,
    }))
}

pub async fn report(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Json(report): Json<AgentReport>,
) -> ApiResult<Json<serde_json::Value>> {
    let credential = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agents WHERE id=? AND credential_hash=? AND revoked=0",
    )
    .bind(&report.agent_id)
    .bind(hash(credential))
    .fetch_one(&state.0.db)
    .await?;
    if exists != 1 {
        return Err(ApiError::unauthorized());
    }
    if report.schema_version != SCHEMA_VERSION
        || Uuid::parse_str(&report.report_id).is_err()
        || !report.collected_at.is_finite()
        || report.collected_at > now() + 300.0
        || report.collected_at < now() - 7.0 * 86400.0
    {
        return Err(ApiError::bad(
            "Invalid report version, identity, or timestamp",
        ));
    }
    if report.metrics.disks.len() > 128
        || report.metrics.gpus.len() > 32
        || report.collectors.len() > 64
    {
        return Err(ApiError::bad("Too many metric dimensions"));
    }
    if let Some(inv) = &report.inventory {
        validate_inventory(inv)?;
    }
    let _guard = state.0.writes.lock().await;
    let row = sqlx::query("SELECT body FROM hosts WHERE agent_id=?")
        .bind(&report.agent_id)
        .fetch_optional(&state.0.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut host: Host = serde_json::from_str(row.get("body"))?;
    let previous: f64 = sqlx::query_scalar::<_, Option<f64>>(
        "SELECT MAX(collected_at) FROM metrics WHERE agent_id=?",
    )
    .bind(&report.agent_id)
    .fetch_one(&state.0.db)
    .await?
    .unwrap_or(0.0);
    let mut tx = state.0.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO metrics(agent_id,report_id,collected_at,body) VALUES(?,?,?,?)",
    )
    .bind(&report.agent_id)
    .bind(&report.report_id)
    .bind(report.collected_at)
    .bind(serde_json::to_string(&report.metrics)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted > 0 {
        host.last_report = Some(now());
        host.last_seen = now();
        if report.collected_at >= previous {
            if let Some(inv) = report.inventory {
                host.name = inv.hostname.clone();
                host.addresses = inventory_addresses(&inv);
                host.macs = inventory_macs(&inv);
                host.inventory = Some(inv);
            }
            host.metrics = Some(report.metrics);
            host.collectors = report.collectors;
        }
        sqlx::query("UPDATE hosts SET name=?,body=? WHERE agent_id=?")
            .bind(&host.name)
            .bind(serde_json::to_string(&host)?)
            .bind(&report.agent_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if inserted > 0 {
        host.network_ids = crate::api::matching_networks(&state, &host.addresses).await?;
        state.save_host(&host).await?;
        state.changed();
    }
    Ok(Json(json!({"accepted":true,"duplicate":inserted==0})))
}

pub async fn revoke(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let _guard = state.0.writes.lock().await;
    sqlx::query("UPDATE agents SET revoked=1 WHERE id=?")
        .bind(&id)
        .execute(&state.0.db)
        .await?;
    state.changed();
    Ok(Json(json!({"ok":true})))
}
