use crate::{
    auth::limited,
    error::{ApiError, ApiResult},
    limits::ClientKey,
    state::{State, hash, now, secret},
};
use axum::{
    Json,
    extract::{Extension, Path, State as Extract},
    http::{HeaderMap, StatusCode},
};
use hope_protocol::{
    AgentReport, EnrollmentRequest, EnrollmentResponse, Host, Inventory, SCHEMA_VERSION,
};
use hope_protocol::{EnrollmentTokenRequest, EnrollmentTokenResponse, OkResponse, ReportResponse};
use sqlx::Row;
use uuid::Uuid;

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub async fn issue_token(state: &State) -> ApiResult<String> {
    issue_token_for_reset(state, None).await
}
pub async fn issue_token_for_reset(
    state: &State,
    reset_agent_id: Option<&str>,
) -> ApiResult<String> {
    if let Some(id) = reset_agent_id {
        let revoked: Option<bool> = sqlx::query_scalar("SELECT revoked FROM agents WHERE id=?")
            .bind(id)
            .fetch_optional(&state.0.db)
            .await?;
        if revoked != Some(true) {
            return Err(ApiError::bad(
                "Credential replacement requires an existing revoked agent",
            ));
        }
    }
    let token = secret();
    sqlx::query("DELETE FROM enrollment_tokens WHERE expires_at < ?")
        .bind(now() - 86400.0)
        .execute(&state.0.db)
        .await?;
    sqlx::query(
        "INSERT INTO enrollment_tokens(token_hash,expires_at,reset_agent_id) VALUES(?,?,?)",
    )
    .bind(hash(&token))
    .bind(now() + 900.0)
    .bind(reset_agent_id)
    .execute(&state.0.db)
    .await?;
    Ok(token)
}
pub fn install_command(state: &State, token: &str) -> String {
    install_command_with_reset(state, token, false)
}
pub fn install_command_with_reset(state: &State, token: &str, reset: bool) -> String {
    install_command_for(state, token, reset, false)
}
/// `persistent` adds `HOPE_REQUIRE_PERSISTENT_SERVICE=1` (SSH jobs: nobody is logged in afterwards,
/// so a user service that stops at logout must be refused before anything is installed).
pub fn install_command_for(state: &State, token: &str, reset: bool, persistent: bool) -> String {
    let server = shell_quote(&state.0.server_url);
    let ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|path| format!(" HOPE_CA_CERT={}", shell_quote(path)))
        .unwrap_or_default();
    let reset = if reset {
        " HOPE_RESET_CREDENTIAL=1"
    } else {
        ""
    };
    let persistent = if persistent {
        " HOPE_REQUIRE_PERSISTENT_SERVICE=1"
    } else {
        ""
    };
    let curl_ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|path| format!(" --cacert {}", shell_quote(path)))
        .unwrap_or_default();
    format!(
        "( umask 077; installer=$(mktemp) || exit 1; trap 'rm -f \"$installer\"' EXIT; curl --fail --silent --show-error --location --max-time 30 --proto '=https' --proto-redir '=https'{curl_ca} {server}/agent/install -o \"$installer\" && HOPE_SERVER={server} HOPE_ENROLLMENT_TOKEN={}{}{reset}{persistent} sh \"$installer\" )",
        shell_quote(token),
        ca
    )
}
pub async fn token(
    Extract(state): Extract<State>,
    Json(body): Json<EnrollmentTokenRequest>,
) -> ApiResult<Json<EnrollmentTokenResponse>> {
    let token = issue_token_for_reset(&state, body.reset_agent_id.as_deref()).await?;
    Ok(Json(EnrollmentTokenResponse {
        command: install_command_with_reset(&state, &token, body.reset_agent_id.is_some()),
        token,
        expires_at: now() + 900.0,
        custom_ca_required: state.0.enrollment_ca.is_some(),
    }))
}

pub fn validate_inventory(inv: &Inventory) -> ApiResult<()> {
    crate::validation::inventory(inv)
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

/// Failed enrollments count against the client; successful ones do not, and there is no global
/// ceiling that would let an attacker block a legitimate agent.
/// The agent's version from its `User-Agent` (`hope-agent/0.1.57`), when well formed.
pub fn agent_version(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("user-agent")?.to_str().ok()?;
    let version = value.strip_prefix("hope-agent/")?;
    (!version.is_empty()
        && version.len() <= 32
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')))
    .then(|| version.to_string())
}

pub async fn enroll(
    Extract(state): Extract<State>,
    Extension(client): Extension<ClientKey>,
    headers: HeaderMap,
    Json(request): Json<EnrollmentRequest>,
) -> ApiResult<Json<EnrollmentResponse>> {
    limited(state.0.enroll_limit.check(&client.0, now()))?;
    let result = enroll_inner(&state, request, agent_version(&headers)).await;
    if result.is_err() {
        state.0.enroll_limit.hit(&client.0, now());
    }
    result
}
async fn enroll_inner(
    state: &State,
    request: EnrollmentRequest,
    version: Option<String>,
) -> ApiResult<Json<EnrollmentResponse>> {
    let state = state.clone();
    if request.schema_version != SCHEMA_VERSION
        || Uuid::parse_str(&request.agent_id).is_err()
        || request.credential.len() != 64
        || !request.credential.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ApiError::bad(
            "Unsupported schema or invalid agent identity",
        ));
    }
    validate_inventory(&request.inventory)?;
    let _guard = state.0.writes.lock().await;
    let existing: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&request.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    if let Some((credential_hash, revoked)) = existing {
        if revoked {
            // Only an administrator-issued, identity-bound token can replace a revoked secret.
            let mut tx = state.0.db.begin().await?;
            let consumed = sqlx::query("UPDATE enrollment_tokens SET used_at=?,agent_id=? WHERE token_hash=? AND expires_at>? AND used_at IS NULL AND reset_agent_id=?")
                .bind(now()).bind(&request.agent_id).bind(hash(&request.token)).bind(now()).bind(&request.agent_id).execute(&mut *tx).await?.rows_affected();
            if consumed != 1 || credential_hash == hash(&request.credential) {
                return Err(ApiError::unauthorized());
            }
            sqlx::query("UPDATE agents SET revoked=0,credential_hash=? WHERE id=?")
                .bind(hash(&request.credential))
                .bind(&request.agent_id)
                .execute(&mut *tx)
                .await?;
            let body: String = sqlx::query_scalar("SELECT body FROM hosts WHERE agent_id=?")
                .bind(&request.agent_id)
                .fetch_one(&mut *tx)
                .await?;
            let mut host: Host = serde_json::from_str(&body)?;
            host.agent_revoked = false;
            sqlx::query("UPDATE hosts SET body=? WHERE agent_id=?")
                .bind(serde_json::to_string(&host)?)
                .bind(&request.agent_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            state.changed();
            return Ok(Json(EnrollmentResponse {
                credential: request.credential,
                report_interval_seconds: 30,
            }));
        }
        if credential_hash != hash(&request.credential) {
            return Err(ApiError(StatusCode::CONFLICT,"Agent identity is already enrolled or revoked; preserve its credential or explicitly reset its identity".into()));
        }
        let recovered = sqlx::query("UPDATE enrollment_tokens SET used_at=?,agent_id=? WHERE token_hash=? AND expires_at>? AND (used_at IS NULL OR agent_id=?) AND (reset_agent_id IS NULL OR reset_agent_id=?)")
            .bind(now()).bind(&request.agent_id).bind(hash(&request.token)).bind(now()).bind(&request.agent_id).bind(&request.agent_id).execute(&state.0.db).await?.rows_affected();
        if recovered != 1 {
            return Err(ApiError::unauthorized());
        }
        return Ok(Json(EnrollmentResponse {
            credential: request.credential,
            report_interval_seconds: 30,
        }));
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
        .fetch_one(&state.0.db)
        .await?;
    if count >= 250 {
        return Err(ApiError::bad(
            "This installation supports at most 250 agent identities",
        ));
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
        Host::new(Uuid::new_v4().to_string(), String::new(), "agent")
    };
    host.agent_id = Some(request.agent_id.clone());
    host.name = request.inventory.hostname.clone();
    host.addresses = addresses;
    host.macs = macs;
    host.inventory = Some(request.inventory);
    host.agent_version = version;
    host.network_ids = crate::api::matching_networks(&state, &host.addresses).await?;
    crate::classify::refresh(&mut host, &state.gateway_set());
    let credential = request.credential;
    let mut tx = state.0.db.begin().await?;
    let changed=sqlx::query("UPDATE enrollment_tokens SET used_at=?,agent_id=? WHERE token_hash=? AND used_at IS NULL AND expires_at>? AND reset_agent_id IS NULL")
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
    Extension(client): Extension<ClientKey>,
    headers: HeaderMap,
    Json(report): Json<AgentReport>,
) -> ApiResult<Json<ReportResponse>> {
    limited(state.0.report_denied.check(&client.0, now()))?;
    let Some(credential) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    else {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    };
    // Authenticate before queueing on the global write lock so unauthenticated callers cannot
    // hold up legitimate writers. The check is repeated under the lock to order against revocation.
    let known: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agents WHERE id=? AND credential_hash=? AND revoked=0",
    )
    .bind(&report.agent_id)
    .bind(hash(credential))
    .fetch_one(&state.0.db)
    .await?;
    if known != 1 {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    }
    // Keyed by the authenticated identity only: a valid credential never meets a shared ceiling.
    limited(state.0.report_limit.check(&report.agent_id, now()))?;
    state.0.report_limit.hit(&report.agent_id, now());
    let _guard = state.0.writes.lock().await;
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
        || report.collected_at < now() - 86400.0
    {
        return Err(ApiError::bad(
            "Invalid report version, identity, or timestamp",
        ));
    }
    crate::validation::metrics(&report.metrics)?;
    crate::validation::collectors(&report.collectors)?;
    if let Some(inv) = &report.inventory {
        validate_inventory(inv)?;
    }
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
        if let Some(version) = agent_version(&headers) {
            host.agent_version = Some(version);
        }
        if report.collected_at >= previous {
            host.last_collected_at = Some(report.collected_at);
            if let Some(inv) = report.inventory {
                host.name = inv.hostname.clone();
                host.addresses = inventory_addresses(&inv);
                host.macs = inventory_macs(&inv);
                host.inventory = Some(inv);
            }
            host.metrics = Some(report.metrics);
            host.collectors = report.collectors;
        }
        host.network_ids = crate::api::matching_networks(&state, &host.addresses).await?;
        crate::classify::refresh(&mut host, &state.gateway_set());
        sqlx::query("UPDATE hosts SET name=?,body=? WHERE agent_id=?")
            .bind(&host.name)
            .bind(serde_json::to_string(&host)?)
            .bind(&report.agent_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if inserted > 0 {
        state.changed();
    }
    Ok(Json(ReportResponse {
        accepted: true,
        duplicate: inserted == 0,
    }))
}

pub async fn revoke(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    let _guard = state.0.writes.lock().await;
    let mut tx = state.0.db.begin().await?;
    sqlx::query("UPDATE agents SET revoked=1 WHERE id=?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    for mut host in state.hosts().await? {
        if host.agent_id.as_ref() == Some(&id) {
            host.agent_revoked = true;
            sqlx::query("UPDATE hosts SET body=? WHERE id=?")
                .bind(serde_json::to_string(&host)?)
                .bind(&host.id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    state.changed();
    Ok(Json(OkResponse { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_versions_come_only_from_well_formed_user_agents() {
        let headers = |value: &str| {
            let mut map = HeaderMap::new();
            map.insert("user-agent", value.parse().expect("header"));
            map
        };
        assert_eq!(
            agent_version(&headers("hope-agent/0.1.57")).as_deref(),
            Some("0.1.57")
        );
        assert_eq!(
            agent_version(&headers("hope-agent/0.1.0-rc.1")).as_deref(),
            Some("0.1.0-rc.1")
        );
        assert_eq!(agent_version(&headers("curl/8.0")), None);
        assert_eq!(agent_version(&headers("hope-agent/")), None);
        assert_eq!(agent_version(&headers("hope-agent/1.0 <script>")), None);
        assert_eq!(agent_version(&HeaderMap::new()), None);
    }
}
