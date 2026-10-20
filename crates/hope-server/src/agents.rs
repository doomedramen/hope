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
    AgentReport, AssociationUpdate, ChildReportBatch, ChildReportResponse, ChildSourceKind,
    EnrollmentRequest, EnrollmentResponse, Host, Inventory, Network, SCHEMA_VERSION,
};
use hope_protocol::{
    EnrollmentTokenRequest, EnrollmentTokenResponse, HISTORY_SAMPLE_SECONDS, OkResponse,
    ReportResponse, UPDATE_REQUEST_SECONDS, UpdateRequested, version_newer,
};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub async fn uninstall_instructions(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<hope_protocol::AgentUninstallInstructions>> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agents WHERE id=?)")
        .bind(id)
        .fetch_one(&state.0.db)
        .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    Ok(Json(hope_protocol::AgentUninstallInstructions {
        command: uninstall_command(&state),
        custom_ca_required: state.0.enrollment_ca.is_some(),
    }))
}

pub fn uninstall_command(state: &State) -> String {
    let url = shell_quote(&format!("{}/agent/uninstall", state.0.server_url));
    let curl_ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|path| format!(" --cacert {}", shell_quote(path)))
        .unwrap_or_default();
    format!(
        "( umask 077; installer=$(mktemp) || exit 1; trap 'rm -f \"$installer\"' EXIT; curl -q --fail --silent --show-error --location --max-time 30 --proto '=https' --proto-redir '=https'{curl_ca} {url} -o \"$installer\" && sh \"$installer\" uninstall )"
    )
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
/// Updates an agent that is already installed, with no token: the installer finds the enrolled
/// identity, verifies the signed release and changes nothing unless the release is newer. Running
/// it as a full installation also puts the updater in place, so later updates can be one click.
pub fn upgrade_command(state: &State) -> String {
    let server = shell_quote(&state.0.server_url);
    let ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|path| format!(" HOPE_CA_CERT={}", shell_quote(path)))
        .unwrap_or_default();
    let curl_ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|path| format!(" --cacert {}", shell_quote(path)))
        .unwrap_or_default();
    format!(
        "( umask 077; installer=$(mktemp) || exit 1; trap 'rm -f \"$installer\"' EXIT; curl --fail --silent --show-error --location --max-time 30 --proto '=https' --proto-redir '=https'{curl_ca} {server}/agent/install -o \"$installer\" && HOPE_SERVER={server}{ca} HOPE_ONLY_NEWER=1 sh \"$installer\" upgrade )"
    )
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
        .filter(|address| crate::addresses::usable_ip(address).is_some())
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
            host.agent_uninstalled = false;
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
    let known: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&report.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    if known
        .as_ref()
        .is_none_or(|(expected, _)| expected != &hash(credential))
    {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    }
    if known.is_some_and(|(_, revoked)| revoked) {
        return Err(ApiError::agent_revoked());
    }
    // Keyed by the authenticated identity only: a valid credential never meets a shared ceiling.
    limited(state.0.report_limit.check(&report.agent_id, now()))?;
    state.0.report_limit.hit(&report.agent_id, now());
    let _guard = state.0.writes.lock().await;
    let exists: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&report.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    match exists {
        Some((expected, revoked)) if expected == hash(credential) => {
            if revoked {
                return Err(ApiError::agent_revoked());
            }
        }
        _ => return Err(ApiError::unauthorized()),
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
    let duplicate: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM metrics WHERE agent_id=? AND report_id=?)")
            .bind(&report.agent_id)
            .bind(&report.report_id)
            .fetch_one(&state.0.db)
            .await?;
    let mut response = ReportResponse {
        accepted: true,
        duplicate,
        report_interval_seconds: Some(state.report_interval()),
        update_to: pending_update(&host, now()),
    };
    if duplicate {
        return Ok(Json(response));
    }
    // Live figures follow every report, but history keeps one raw sample per
    // HISTORY_SAMPLE_SECONDS so a short report interval does not multiply stored rows. The
    // margin absorbs the agent's scheduling jitter.
    let spacing = HISTORY_SAMPLE_SECONDS - 2.5;
    let crowded: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>? AND collected_at<?)",
    )
    .bind(&report.agent_id)
    .bind(report.collected_at - spacing)
    .bind(report.collected_at + spacing)
    .fetch_one(&state.0.db)
    .await?;
    let mut tx = state.0.db.begin().await?;
    if !crowded {
        sqlx::query(
            "INSERT OR IGNORE INTO metrics(agent_id,report_id,collected_at,body) VALUES(?,?,?,?)",
        )
        .bind(&report.agent_id)
        .bind(&report.report_id)
        .bind(report.collected_at)
        .bind(serde_json::to_string(&report.metrics)?)
        .execute(&mut *tx)
        .await?;
    }
    host.last_report = Some(now());
    host.last_seen = now();
    if let Some(version) = agent_version(&headers) {
        host.agent_version = Some(version);
    }
    settle_update_request(&mut host, now());
    response.update_to = pending_update(&host, now());
    // A backlog delivered after an outage fills history but never replaces newer live state.
    if report.collected_at >= host.last_collected_at.unwrap_or(0.0) {
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
    tx.commit().await?;
    state.host_changed(&host);
    Ok(Json(response))
}

/// Stores a bounded, sequenced child inventory independently of the parent heartbeat. Only a
/// complete authoritative generation advances absence tracking; partial inventories cannot age
/// children toward archival.
fn same_interface_mac(
    left: &hope_protocol::ChildDeviceReport,
    right: &hope_protocol::ChildDeviceReport,
) -> bool {
    let macs = |report: &hope_protocol::ChildDeviceReport| {
        report
            .interfaces
            .iter()
            .filter_map(|interface| interface.mac.as_deref())
            .chain(
                report
                    .network_configuration
                    .iter()
                    .filter_map(|network| network.mac.as_deref()),
            )
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>()
    };
    let left = macs(left);
    macs(right).iter().any(|mac| left.contains(mac))
}

pub async fn child_report(
    Extract(state): Extract<State>,
    Extension(client): Extension<ClientKey>,
    headers: HeaderMap,
    Json(batch): Json<ChildReportBatch>,
) -> ApiResult<Json<ChildReportResponse>> {
    limited(state.0.report_denied.check(&client.0, now()))?;
    let Some(credential) = headers
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    };
    let known: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&batch.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    if known
        .as_ref()
        .is_none_or(|(expected, _)| expected != &hash(credential))
    {
        state.0.report_denied.hit(&client.0, now());
        return Err(ApiError::unauthorized());
    }
    if known.is_some_and(|(_, revoked)| revoked) {
        return Err(ApiError::agent_revoked());
    }
    limited(state.0.report_limit.check(&batch.agent_id, now()))?;
    state.0.report_limit.hit(&batch.agent_id, now());
    hope_protocol::validation::child_batch(&batch).map_err(ApiError::bad)?;
    if batch.legacy_identities.len() > 500
        || batch.legacy_identities.iter().any(|item| {
            item.key.is_empty()
                || item.incarnation.is_empty()
                || item.canonical_incarnation.is_empty()
        })
    {
        return Err(ApiError::bad("Invalid legacy child identity map"));
    }
    if batch.observed_at > now() + 300.0 || batch.observed_at < now() - 86400.0 {
        return Err(ApiError::bad("Invalid child inventory timestamp"));
    }

    // Authenticate again under the shared write lock so revocation and credential replacement
    // are ordered against source writes exactly like core reports.
    let _guard = state.0.writes.lock().await;
    let current: Option<(String, bool)> =
        sqlx::query_as("SELECT credential_hash,revoked FROM agents WHERE id=?")
            .bind(&batch.agent_id)
            .fetch_optional(&state.0.db)
            .await?;
    match current {
        Some((expected, false)) if expected == hash(credential) => {}
        Some((expected, true)) if expected == hash(credential) => {
            return Err(ApiError::agent_revoked());
        }
        _ => return Err(ApiError::unauthorized()),
    }

    let source = match batch.source {
        ChildSourceKind::Docker => "docker",
        ChildSourceKind::Proxmox => "proxmox",
    };
    let mut tx = state.0.db.begin().await?;
    let shared_scope = if let Some(token) = batch.source_token.as_deref() {
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ApiError::bad("Invalid shared source token"));
        }
        let token_hash = hash(token);
        let bound: Option<String> = sqlx::query_scalar(
            "SELECT token_hash FROM shared_source_scopes WHERE source=? AND source_scope=?",
        )
        .bind(source)
        .bind(&batch.source_scope)
        .fetch_optional(&mut *tx)
        .await?;
        match bound {
            Some(expected) if expected != token_hash => {
                return Err(ApiError::bad(
                    "Shared source token does not match this cluster",
                ));
            }
            Some(_) => true,
            None => {
                sqlx::query("INSERT INTO shared_source_scopes(source,source_scope,token_hash,created_at) VALUES(?,?,?,?)")
                    .bind(source).bind(&batch.source_scope).bind(token_hash).bind(now()).execute(&mut *tx).await?;
                true
            }
        }
    } else {
        false
    };
    if shared_scope {
        sqlx::query("INSERT OR IGNORE INTO shared_source_members(source,source_scope,agent_id,joined_at) VALUES(?,?,?,?)")
            .bind(source).bind(&batch.source_scope).bind(&batch.agent_id).bind(now()).execute(&mut *tx).await?;
    }
    let replay: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT parent_agent_id,source_scope,sequence FROM child_batches WHERE batch_id=?",
    )
    .bind(&batch.batch_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((parent, scope, sequence)) = replay {
        if parent == batch.agent_id
            && scope == batch.source_scope
            && sequence == batch.sequence as i64
        {
            tx.rollback().await?;
            return Ok(Json(ChildReportResponse {
                accepted: true,
                duplicate: true,
            }));
        }
        return Err(ApiError::bad("Child inventory batch identity was reused"));
    }
    let head: Option<(i64, i64)> = sqlx::query_as(
        "SELECT sequence,inventory_generation FROM child_batch_heads WHERE parent_agent_id=? AND source=? AND source_scope=?",
    )
    .bind(&batch.agent_id)
    .bind(source)
    .bind(&batch.source_scope)
    .fetch_optional(&mut *tx)
    .await?;
    if head.is_some_and(|(sequence, generation)| {
        batch.sequence <= sequence as u64 || batch.inventory_generation < generation as u64
    }) {
        return Err(ApiError::bad(
            "Child inventory sequence or generation is out of order",
        ));
    }
    let parent_host: String = sqlx::query_scalar("SELECT id FROM hosts WHERE agent_id=?")
        .bind(&batch.agent_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if shared_scope && let Some(old_scope) = batch.legacy_source_scope.as_deref() {
        for legacy in &batch.legacy_identities {
            let kind = match legacy.kind {
                hope_protocol::ChildDeviceKind::DockerContainer => "docker_container",
                hope_protocol::ChildDeviceKind::ProxmoxVm => "proxmox_vm",
                hope_protocol::ChildDeviceKind::ProxmoxLxc => "proxmox_lxc",
            };
            let old_row: Option<(String, String, f64, String)> = sqlx::query_as("SELECT id,body,observed_at,lifecycle FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=? AND kind=?")
                    .bind(&batch.agent_id).bind(source).bind(old_scope).bind(&legacy.key).bind(&legacy.incarnation).bind(kind)
                    .fetch_optional(&mut *tx).await?;
            let Some((old_id, old_body, old_seen, old_lifecycle)) = old_row else {
                continue;
            };
            // Keep metadata-only migration evidence pending until a current observation of
            // the canonical identity proves that the interface still matches. Old local
            // maps can survive VMID reuse, so a tuple alone must never move history.
            sqlx::query("INSERT OR IGNORE INTO pending_legacy_identities(source,parent_agent_id,legacy_source_device_id,source_scope,source_key,canonical_incarnation) VALUES(?,?,?,?,?,?)")
                    .bind(source).bind(&batch.agent_id).bind(&old_id).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                    .execute(&mut *tx).await?;

            // The canonical owner may have reported before this remote reporter submitted its
            // legacy map. In that ordering no later guest batch from this reporter may arrive
            // (for example, its collector can be stopped), so reconcile against the already
            // stored shared identity now. The token-bound scope/key/incarnation selects the
            // canonical record; matching kind, fresh timestamps, and an interface MAC prove the
            // legacy row is the same current guest rather than a reused VMID.
            crate::api::reconcile_shared_identity(
                &mut tx,
                source,
                &batch.source_scope,
                &legacy.key,
                &legacy.canonical_incarnation,
            )
            .await?;
            let canonical_id: Option<String> = sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE source=? AND identity_owner='@shared' AND source_scope=? AND source_key=? AND incarnation=?")
                .bind(source).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                .fetch_optional(&mut *tx).await?;
            if old_lifecycle != "archived"
                && let Some(canonical_id) = canonical_id
                && canonical_id != old_id
            {
                let canonical: Option<(String, String, f64, String)> = sqlx::query_as(
                    "SELECT kind,body,observed_at,lifecycle FROM source_devices WHERE id=?",
                )
                .bind(&canonical_id)
                .fetch_optional(&mut *tx)
                .await?;
                if let Some((canonical_kind, canonical_body, canonical_seen, canonical_lifecycle)) =
                    canonical
                    && canonical_kind == kind
                    && canonical_lifecycle != "archived"
                    && batch.observed_at >= old_seen
                    && batch.observed_at - old_seen <= 30.0 * 86400.0
                    && batch.observed_at >= canonical_seen
                    && batch.observed_at - canonical_seen <= 30.0 * 86400.0
                    && (canonical_seen - old_seen).abs() <= 30.0 * 86400.0
                {
                    let old: hope_protocol::ChildDeviceReport = serde_json::from_str(&old_body)?;
                    let canonical: hope_protocol::ChildDeviceReport =
                        serde_json::from_str(&canonical_body)?;
                    if same_interface_mac(&old, &canonical) {
                        crate::api::merge_legacy_source_identity(
                            &mut tx,
                            &canonical_id,
                            &old_id,
                            &batch.agent_id,
                        )
                        .await?;
                        sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                            .bind(source).bind(&batch.agent_id).bind(&old_id).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                            .execute(&mut *tx).await?;
                    }
                }
            } else if old_lifecycle != "archived"
                && batch.observed_at >= old_seen
                && batch.observed_at - old_seen <= 30.0 * 86400.0
            {
                // Before any current owner report exists, two authorized cluster members may
                // submit maps for copies of the same old cluster-wide VM inventory. Join those
                // copies only when both point to the exact same token-bound canonical identity,
                // have current rows, and share an interface MAC. The pending-map membership is
                // the cluster proof; VMID/name/IP coincidence alone never reaches this path.
                let old_report: hope_protocol::ChildDeviceReport = serde_json::from_str(&old_body)?;
                let siblings = sqlx::query("SELECT p.parent_agent_id,p.legacy_source_device_id,d.body,d.observed_at,d.lifecycle,d.kind FROM pending_legacy_identities p JOIN source_devices d ON d.id=p.legacy_source_device_id AND d.parent_agent_id=p.parent_agent_id AND d.source=p.source AND d.source_key=p.source_key JOIN shared_source_members m ON m.source=p.source AND m.source_scope=p.source_scope AND m.agent_id=p.parent_agent_id JOIN agents a ON a.id=p.parent_agent_id AND a.revoked=0 WHERE p.source=? AND p.source_scope=? AND p.source_key=? AND p.canonical_incarnation=? AND p.legacy_source_device_id!=? AND p.parent_agent_id!=? ORDER BY d.observed_at DESC,d.id")
                    .bind(source).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                    .bind(&old_id).bind(&batch.agent_id).fetch_all(&mut *tx).await?;
                for sibling in siblings {
                    let sibling_parent: String = sibling.get("parent_agent_id");
                    let sibling_id: String = sibling.get("legacy_source_device_id");
                    let sibling_body: String = sibling.get("body");
                    let sibling_seen: f64 = sibling.get("observed_at");
                    let sibling_lifecycle: String = sibling.get("lifecycle");
                    let sibling_kind: String = sibling.get("kind");
                    if sibling_kind != kind
                        || sibling_lifecycle == "archived"
                        || batch.observed_at < sibling_seen
                        || batch.observed_at - sibling_seen > 30.0 * 86400.0
                        || (sibling_seen - old_seen).abs() > 30.0 * 86400.0
                    {
                        continue;
                    }
                    let sibling_report: hope_protocol::ChildDeviceReport =
                        serde_json::from_str(&sibling_body)?;
                    if !same_interface_mac(&old_report, &sibling_report) {
                        continue;
                    }
                    // Pick a stable survivor independent of which agent's metadata arrives
                    // first; old URLs for the duplicate remain resolvable through the alias.
                    let (survivor, duplicate, duplicate_parent) = if old_id <= sibling_id {
                        (&old_id, &sibling_id, &sibling_parent)
                    } else {
                        (&sibling_id, &old_id, &batch.agent_id)
                    };
                    crate::api::merge_legacy_source_identity(
                        &mut tx,
                        survivor,
                        duplicate,
                        duplicate_parent,
                    )
                    .await?;
                    sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                        .bind(source).bind(duplicate_parent).bind(duplicate).bind(&batch.source_scope).bind(&legacy.key).bind(&legacy.canonical_incarnation)
                        .execute(&mut *tx).await?;
                }
            }
        }
    }
    sqlx::query("INSERT INTO child_batches(batch_id,parent_agent_id,source,source_scope,sequence,inventory_generation,inventory_id,chunk_index,chunk_count,observed_at,complete,received_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&batch.batch_id).bind(&batch.agent_id).bind(source).bind(&batch.source_scope)
        .bind(batch.sequence as i64).bind(batch.inventory_generation as i64)
        .bind(&batch.inventory_id).bind(batch.chunk_index as i64).bind(batch.chunk_count as i64)
        .bind(batch.observed_at).bind(batch.complete).bind(now()).execute(&mut *tx).await?;

    let authoritative_complete = if batch.complete {
        if let Some(inventory_id) = &batch.inventory_id {
            let (count, min_index, max_index, min_count, max_count): (i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
                "SELECT COUNT(*),MIN(chunk_index),MAX(chunk_index),MIN(chunk_count),MAX(chunk_count) FROM child_batches WHERE parent_agent_id=? AND source=? AND source_scope=? AND inventory_id=? AND inventory_generation=?",
            ).bind(&batch.agent_id).bind(source).bind(&batch.source_scope)
                .bind(inventory_id).bind(batch.inventory_generation as i64)
                .fetch_one(&mut *tx).await?;
            count == batch.chunk_count as i64
                && min_index == Some(0)
                && max_index == Some(batch.chunk_count as i64 - 1)
                && min_count == Some(batch.chunk_count as i64)
                && max_count == Some(batch.chunk_count as i64)
        } else {
            batch.chunk_count == 1 && batch.chunk_index == 0
        }
    } else {
        false
    };

    let mut seen: HashSet<(String, String)> = batch
        .devices
        .iter()
        .map(|device| {
            (
                device.identity.key.clone(),
                device.identity.incarnation.clone(),
            )
        })
        .collect();
    let mut source_ids = std::collections::HashMap::new();
    let mut fresh_source_ids = HashSet::new();
    for device in &batch.devices {
        let identity = &device.identity;
        if shared_scope {
            crate::api::reconcile_shared_identity(
                &mut tx,
                source,
                &batch.source_scope,
                &identity.key,
                &identity.incarnation,
            )
            .await?;
        }
        // A scope is only authoritative inside its authenticated reporter until cluster
        // membership has been authorized. This still reconciles node moves reported by one
        // controller while preventing another enrolled agent from claiming the same VMID.
        let identity_owner = if shared_scope {
            "@shared"
        } else {
            batch.agent_id.as_str()
        };
        let mapped_id: Option<String> = sqlx::query_scalar("SELECT source_device_id FROM source_device_identities WHERE source=? AND identity_owner=? AND source_scope=? AND source_key=? AND incarnation=?")
            .bind(source).bind(identity_owner).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).fetch_optional(&mut *tx).await?;
        let kind = match identity.kind {
            hope_protocol::ChildDeviceKind::DockerContainer => "docker_container",
            hope_protocol::ChildDeviceKind::ProxmoxVm => "proxmox_vm",
            hope_protocol::ChildDeviceKind::ProxmoxLxc => "proxmox_lxc",
        };
        let mut pending_legacy_id = None;
        if shared_scope {
            let pending = sqlx::query("SELECT legacy_source_device_id,parent_agent_id FROM pending_legacy_identities WHERE source=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                .bind(source).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation)
                .fetch_all(&mut *tx).await?;
            for row in pending {
                let old_id: String = row.get("legacy_source_device_id");
                let old_parent: String = row.get("parent_agent_id");
                let old: Option<(String, f64)> = sqlx::query_as("SELECT body,observed_at FROM source_devices WHERE id=? AND parent_agent_id=? AND source=? AND kind=?")
                    .bind(&old_id).bind(&old_parent).bind(source).bind(kind).fetch_optional(&mut *tx).await?;
                let Some((old_body, old_seen)) = old else {
                    sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                        .bind(source).bind(&old_parent).bind(&old_id).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).execute(&mut *tx).await?;
                    continue;
                };
                let old: hope_protocol::ChildDeviceReport = serde_json::from_str(&old_body)?;
                if batch.observed_at < old_seen
                    || batch.observed_at - old_seen > 30.0 * 86400.0
                    || !same_interface_mac(device, &old)
                {
                    continue;
                }
                if let Some(canonical) = mapped_id.as_deref() {
                    if canonical != old_id {
                        crate::api::merge_legacy_source_identity(
                            &mut tx,
                            canonical,
                            &old_id,
                            &old_parent,
                        )
                        .await?;
                    }
                } else if let Some(survivor) = pending_legacy_id.as_deref() {
                    if survivor != old_id {
                        crate::api::merge_legacy_source_identity(
                            &mut tx,
                            survivor,
                            &old_id,
                            &old_parent,
                        )
                        .await?;
                    }
                } else {
                    pending_legacy_id.get_or_insert(old_id.clone());
                }
                sqlx::query("DELETE FROM pending_legacy_identities WHERE source=? AND parent_agent_id=? AND legacy_source_device_id=? AND source_scope=? AND source_key=? AND canonical_incarnation=?")
                    .bind(source).bind(&old_parent).bind(&old_id).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).execute(&mut *tx).await?;
            }
        }
        let legacy_row: Option<(String, String, f64)> = if shared_scope {
            match (batch.legacy_source_scope.as_deref(),device.legacy_incarnation.as_deref()) {
                (Some(old_scope),Some(old_incarnation)) => sqlx::query_as("SELECT id,body,observed_at FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=? AND kind=?")
                    .bind(&batch.agent_id).bind(source).bind(old_scope).bind(&identity.key).bind(old_incarnation).bind(kind)
                    .fetch_optional(&mut *tx).await?,
                _ => None,
            }
        } else {
            None
        };
        let legacy_id = if pending_legacy_id.is_some() {
            pending_legacy_id
        } else if let Some((old_id, old_body, old_seen)) = &legacy_row {
            let old: hope_protocol::ChildDeviceReport = serde_json::from_str(old_body)?;
            if batch.observed_at - *old_seen <= 30.0 * 86400.0 && same_interface_mac(device, &old) {
                Some(old_id.clone())
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(canonical), Some(old)) = (mapped_id.as_deref(), legacy_id.as_deref())
            && canonical != old
        {
            crate::api::merge_legacy_source_identity(&mut tx, canonical, old, &batch.agent_id)
                .await?;
        }
        let existing: Option<(String, f64, i64, String)> = if let Some(mapped_id) = mapped_id {
            sqlx::query_as("SELECT id,observed_at,ownership_generation,parent_agent_id FROM source_devices WHERE id=?")
                .bind(mapped_id).fetch_optional(&mut *tx).await?
        } else if let Some(old_id) = legacy_id {
            sqlx::query_as("SELECT id,observed_at,ownership_generation,parent_agent_id FROM source_devices WHERE id=?")
                .bind(old_id).fetch_optional(&mut *tx).await?
        } else {
            sqlx::query_as("SELECT id,observed_at,ownership_generation,parent_agent_id FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND source_key=? AND incarnation=?")
                .bind(&batch.agent_id).bind(source).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).fetch_optional(&mut *tx).await?
        };
        let id = existing
            .as_ref()
            .map(|(id, _, _, _)| id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let owner_generation = device.ownership_generation.unwrap_or(0);
        let is_newer =
            existing
                .as_ref()
                .is_none_or(|(_, observed_at, prior_generation, prior_parent)| {
                    owner_generation > *prior_generation as u64
                        || (owner_generation == *prior_generation as u64
                            && prior_parent == &batch.agent_id
                            && batch.observed_at >= *observed_at)
                });
        if is_newer {
            fresh_source_ids.insert(id.clone());
        }
        let body = serde_json::to_string(device)?;
        if is_newer && sqlx::query("UPDATE source_devices SET parent_agent_id=?,kind=?,lifecycle=?,name=?,observed_at=?,inventory_generation=?,ownership_generation=?,missing_complete_count=0,missing_since=NULL,missing_inventory_generation=0,body=? WHERE id=?")
            .bind(&batch.agent_id)
            .bind(kind)
            .bind(format!("{:?}", device.lifecycle).to_ascii_lowercase())
            .bind(&device.name).bind(batch.observed_at).bind(batch.inventory_generation as i64).bind(owner_generation as i64)
            .bind(&body).bind(&id).execute(&mut *tx).await?.rows_affected() == 0
        {
            sqlx::query("INSERT INTO source_devices(id,parent_agent_id,source,source_scope,source_key,incarnation,kind,lifecycle,name,observed_at,inventory_generation,ownership_generation,body) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(&id).bind(&batch.agent_id).bind(source).bind(&batch.source_scope)
                .bind(&identity.key).bind(&identity.incarnation)
                .bind(kind)
                .bind(format!("{:?}", device.lifecycle).to_ascii_lowercase())
                .bind(&device.name).bind(batch.observed_at).bind(batch.inventory_generation as i64).bind(owner_generation as i64)
                .bind(&body).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO source_device_identities(source,identity_owner,source_scope,source_key,incarnation,source_device_id) VALUES(?,?,?,?,?,?) ON CONFLICT(source,identity_owner,source_scope,source_key,incarnation) DO UPDATE SET source_device_id=excluded.source_device_id")
            .bind(source).bind(identity_owner).bind(&batch.source_scope).bind(&identity.key).bind(&identity.incarnation).bind(&id).execute(&mut *tx).await?;
        sqlx::query("INSERT OR IGNORE INTO source_observations(source_device_id,batch_id,observed_at,inventory_generation,body) VALUES(?,?,?,?,?)")
            .bind(&id).bind(&batch.batch_id).bind(batch.observed_at).bind(batch.inventory_generation as i64)
            .bind(&body).execute(&mut *tx).await?;
        sqlx::query("INSERT OR IGNORE INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES(?,?,?,30,?)")
            .bind(&id).bind(&batch.batch_id).bind(batch.observed_at)
            .bind(serde_json::to_string(&device.metrics)?).execute(&mut *tx).await?;
        source_ids.insert(identity.key.as_str(), id);
    }

    for device in &batch.devices {
        let id = &source_ids[device.identity.key.as_str()];
        if !fresh_source_ids.contains(id) {
            continue;
        }
        let parent = device
            .parent_key
            .as_deref()
            .and_then(|key| source_ids.get(key))
            .map(String::as_str)
            .unwrap_or(&parent_host);
        sqlx::query("DELETE FROM device_relationships WHERE source_device_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT OR REPLACE INTO device_relationships(from_device_id,to_device_id,kind,detail,source_device_id,observed_at) VALUES(?,?,'runs_on',NULL,?,?)")
            .bind(id).bind(parent).bind(id).bind(batch.observed_at).execute(&mut *tx).await?;
        for port in &device.published_ports {
            let Some(host_port) = port.host_port else {
                continue;
            };
            let address = port
                .host_ip
                .as_deref()
                .filter(|ip| !ip.is_empty())
                .unwrap_or("*");
            let detail = format!(
                "{}://{}:{} → container:{}",
                port.protocol, address, host_port, port.container_port
            );
            sqlx::query("INSERT OR REPLACE INTO device_relationships(from_device_id,to_device_id,kind,detail,source_device_id,observed_at) VALUES(?,?,'exposes_service',?,?,?)")
                .bind(id).bind(parent).bind(detail).bind(id).bind(batch.observed_at)
                .execute(&mut *tx).await?;
        }
    }

    let scan_hosts = sqlx::query("SELECT body FROM hosts")
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| {
            let body: String = row.get("body");
            serde_json::from_str::<Host>(&body).map_err(anyhow::Error::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let networks =
        sqlx::query("SELECT id,name,cidr,enabled,interval_minutes FROM networks WHERE enabled=1")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|row| Network {
                id: row.get("id"),
                name: row.get("name"),
                cidr: row.get("cidr"),
                enabled: row.get("enabled"),
                interval_minutes: row.get::<i64, _>("interval_minutes") as u32,
            })
            .collect::<Vec<_>>();
    let confirmed_rows = sqlx::query(
        "SELECT source_device_id,host_id FROM device_associations WHERE status='confirmed'",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut confirmed_by_source = HashMap::new();
    let mut confirmed_hosts = HashMap::new();
    for row in confirmed_rows {
        let source_id: String = row.get("source_device_id");
        let host_id: String = row.get("host_id");
        confirmed_by_source.insert(source_id.clone(), host_id.clone());
        confirmed_hosts.insert(host_id, source_id);
    }
    for device in &batch.devices {
        let id = &source_ids[device.identity.key.as_str()];
        if !fresh_source_ids.contains(id) {
            continue;
        }
        let candidates = crate::associations::candidates(device, &scan_hosts, &networks);
        let existing_confirmed = confirmed_by_source.get(id).cloned();
        let mut candidate_hosts = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            candidate_hosts.push(candidate.host_id.clone());
            let evidence = serde_json::to_string(&candidate.evidence)?;
            let evidence_hash = hash(&evidence);
            let may_confirm = candidate.strong
                && confirmed_hosts
                    .get(&candidate.host_id)
                    .is_none_or(|source_id| source_id == id)
                && existing_confirmed
                    .as_deref()
                    .is_none_or(|confirmed| confirmed == candidate.host_id);
            let status = if may_confirm {
                confirmed_hosts.insert(candidate.host_id.clone(), id.clone());
                confirmed_by_source.insert(id.clone(), candidate.host_id.clone());
                "confirmed"
            } else {
                "suggested"
            };
            sqlx::query("INSERT INTO device_associations(source_device_id,host_id,status,reason,evidence,evidence_hash,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(source_device_id,host_id) DO UPDATE SET status=CASE WHEN device_associations.status='confirmed' THEN 'confirmed' WHEN device_associations.status='rejected' AND device_associations.evidence_hash=excluded.evidence_hash THEN 'rejected' ELSE excluded.status END,reason=excluded.reason,evidence=excluded.evidence,evidence_hash=excluded.evidence_hash,updated_at=CASE WHEN device_associations.status='confirmed' OR (device_associations.status='rejected' AND device_associations.evidence_hash=excluded.evidence_hash) THEN device_associations.updated_at ELSE excluded.updated_at END")
                .bind(id)
                .bind(&candidate.host_id)
                .bind(status)
                .bind(&candidate.reason)
                .bind(evidence)
                .bind(evidence_hash)
                .bind(batch.observed_at)
                .execute(&mut *tx)
                .await?;
        }
        if candidate_hosts.is_empty() {
            sqlx::query(
                "DELETE FROM device_associations WHERE source_device_id=? AND status IN ('suggested','rejected')",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        } else {
            let placeholders = vec!["?"; candidate_hosts.len()].join(",");
            let query = format!(
                "DELETE FROM device_associations WHERE source_device_id=? AND status IN ('suggested','rejected') AND host_id NOT IN ({placeholders})"
            );
            let mut delete = sqlx::query(&query).bind(id);
            for host_id in &candidate_hosts {
                delete = delete.bind(host_id);
            }
            delete.execute(&mut *tx).await?;
        }
        let confirmed: Option<String> = sqlx::query_scalar(
            "SELECT host_id FROM device_associations WHERE source_device_id=? AND status='confirmed' LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query("UPDATE source_devices SET host_id=? WHERE id=?")
            .bind(confirmed)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    if authoritative_complete {
        if let Some(inventory_id) = &batch.inventory_id {
            let rows = sqlx::query(
                "SELECT DISTINCT d.source_key,d.incarnation FROM source_observations o JOIN child_batches b ON b.batch_id=o.batch_id JOIN source_devices d ON d.id=o.source_device_id WHERE b.parent_agent_id=? AND b.source=? AND b.source_scope=? AND b.inventory_id=? AND b.inventory_generation=?",
            )
            .bind(&batch.agent_id)
            .bind(source)
            .bind(&batch.source_scope)
            .bind(inventory_id)
            .bind(batch.inventory_generation as i64)
            .fetch_all(&mut *tx)
            .await?;
            seen = rows
                .into_iter()
                .map(|row| {
                    (
                        sqlx::Row::get(&row, "source_key"),
                        sqlx::Row::get(&row, "incarnation"),
                    )
                })
                .collect();
        }
        let scan_presence = sqlx::query("SELECT id,body FROM hosts")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .filter_map(|row| {
                let id: String = row.get("id");
                let body: String = row.get("body");
                serde_json::from_str::<Host>(&body)
                    .ok()
                    .map(|host| (id, host.last_seen))
            })
            .collect::<HashMap<_, _>>();
        let rows = sqlx::query("SELECT id,source_key,incarnation,host_id,missing_complete_count,missing_since,missing_inventory_generation FROM source_devices WHERE parent_agent_id=? AND source=? AND source_scope=? AND lifecycle!='archived'")
            .bind(&batch.agent_id).bind(source).bind(&batch.source_scope).fetch_all(&mut *tx).await?;
        for row in rows {
            let key: String = sqlx::Row::get(&row, "source_key");
            let incarnation: String = sqlx::Row::get(&row, "incarnation");
            if seen.contains(&(key, incarnation)) {
                continue;
            }
            let id: String = sqlx::Row::get(&row, "id");
            let previous: i64 = sqlx::Row::get(&row, "missing_complete_count");
            let associated_host: Option<String> = sqlx::Row::get(&row, "host_id");
            let since: Option<f64> = sqlx::Row::get(&row, "missing_since");
            let last_generation: i64 = sqlx::Row::get(&row, "missing_inventory_generation");
            if batch.inventory_generation <= last_generation as u64 {
                continue;
            }
            let missing_since = since.unwrap_or(batch.observed_at);
            let count = previous.saturating_add(1);
            let present_elsewhere = associated_host
                .as_ref()
                .and_then(|host_id| scan_presence.get(host_id))
                .is_some_and(|last_seen| batch.observed_at - last_seen < 600.0);
            let lifecycle =
                if count >= 3 && batch.observed_at - missing_since >= 300.0 && !present_elsewhere {
                    "archived"
                } else {
                    "stale"
                };
            sqlx::query("UPDATE source_devices SET lifecycle=?,missing_complete_count=?,missing_since=?,missing_inventory_generation=? WHERE id=?")
                .bind(lifecycle).bind(count).bind(missing_since).bind(batch.inventory_generation as i64).bind(id).execute(&mut *tx).await?;
        }
    }
    sqlx::query("INSERT INTO child_batch_heads(parent_agent_id,source,source_scope,sequence,inventory_generation,observed_at,complete_generation,complete_observed_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(parent_agent_id,source,source_scope) DO UPDATE SET sequence=excluded.sequence,inventory_generation=MAX(child_batch_heads.inventory_generation,excluded.inventory_generation),observed_at=excluded.observed_at,complete_generation=CASE WHEN excluded.complete_generation>0 THEN excluded.complete_generation ELSE child_batch_heads.complete_generation END,complete_observed_at=CASE WHEN excluded.complete_generation>0 THEN excluded.complete_observed_at ELSE child_batch_heads.complete_observed_at END")
        .bind(&batch.agent_id).bind(source).bind(&batch.source_scope).bind(batch.sequence as i64)
        .bind(batch.inventory_generation as i64).bind(batch.observed_at)
        .bind(if authoritative_complete { batch.inventory_generation as i64 } else { 0 })
        .bind(if authoritative_complete { Some(batch.observed_at) } else { None })
        .execute(&mut *tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(ChildReportResponse {
        accepted: true,
        duplicate: false,
    }))
}

/// Confirm, reject, or unlink a source-to-scan-device association without deleting source data.
pub async fn update_association(
    Extract(state): Extract<State>,
    Path(source_device_id): Path<String>,
    Json(update): Json<AssociationUpdate>,
) -> ApiResult<Json<Host>> {
    let _guard = state.0.writes.lock().await;
    let source: Option<String> = sqlx::query_scalar("SELECT body FROM source_devices WHERE id=?")
        .bind(&source_device_id)
        .fetch_optional(&state.0.db)
        .await?;
    let Some(source) = source else {
        return Err(ApiError::not_found());
    };
    let device: hope_protocol::ChildDeviceReport = serde_json::from_str(&source)?;
    let (host_id, action) = match update {
        AssociationUpdate::Confirm { host_id } => (host_id, "confirmed"),
        AssociationUpdate::Reject { host_id } => (host_id, "rejected"),
        AssociationUpdate::Unlink { host_id } => (host_id, "unlinked"),
    };
    let target_body: Option<String> = sqlx::query_scalar("SELECT body FROM hosts WHERE id=?")
        .bind(&host_id)
        .fetch_optional(&state.0.db)
        .await?;
    let Some(target_body) = target_body else {
        return Err(ApiError::not_found());
    };
    let target_host: Host = serde_json::from_str(&target_body)?;
    let networks = crate::api::networks(&state)
        .await?
        .into_iter()
        .filter(|network| network.enabled)
        .collect::<Vec<_>>();
    let candidate =
        crate::associations::candidates(&device, std::slice::from_ref(&target_host), &networks)
            .into_iter()
            .find(|candidate| candidate.host_id == host_id);
    let evidence = candidate
        .as_ref()
        .map(|candidate| candidate.evidence.clone())
        .unwrap_or_default();
    let evidence_json = serde_json::to_string(&evidence)?;
    let evidence_hash = hash(&evidence_json);

    let mut tx = state.0.db.begin().await?;
    if action == "unlinked" {
        sqlx::query("DELETE FROM device_associations WHERE source_device_id=? AND host_id=?")
            .bind(&source_device_id)
            .bind(&host_id)
            .execute(&mut *tx)
            .await?;
    } else {
        if action == "confirmed" {
            let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM device_associations WHERE host_id=? AND status='confirmed' AND source_device_id!=?)")
                .bind(&host_id)
                .bind(&source_device_id)
                .fetch_one(&mut *tx)
                .await?;
            if taken {
                return Err(ApiError::bad(
                    "This discovered host is already linked to another source device",
                ));
            }
            sqlx::query("DELETE FROM device_associations WHERE source_device_id=? AND status='confirmed' AND host_id!=?")
                .bind(&source_device_id)
                .bind(&host_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO device_associations(source_device_id,host_id,status,reason,evidence,evidence_hash,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(source_device_id,host_id) DO UPDATE SET status=excluded.status,reason=excluded.reason,evidence=excluded.evidence,evidence_hash=excluded.evidence_hash,updated_at=excluded.updated_at")
            .bind(&source_device_id)
            .bind(&host_id)
            .bind(action)
            .bind(if action == "confirmed" { "Manually confirmed" } else { "Rejected by administrator" })
            .bind(&evidence_json)
            .bind(&evidence_hash)
            .bind(now())
            .execute(&mut *tx)
            .await?;
    }
    let confirmed: Option<String> = sqlx::query_scalar(
        "SELECT host_id FROM device_associations WHERE source_device_id=? AND status='confirmed' LIMIT 1",
    )
    .bind(&source_device_id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("UPDATE source_devices SET host_id=? WHERE id=?")
        .bind(confirmed)
        .bind(&source_device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(
        state
            .host(&host_id)
            .await?
            .ok_or_else(ApiError::not_found)?,
    ))
}

/// The release an agent should be updating to, while an administrator's request is still fresh.
fn pending_update(host: &Host, at: f64) -> Option<String> {
    let requested = host.update_requested_at?;
    let target = host.update_target.as_ref()?;
    let outdated = host
        .agent_version
        .as_deref()
        .is_none_or(|have| version_newer(target, have));
    (outdated && at - requested < UPDATE_REQUEST_SECONDS).then(|| target.clone())
}
/// Forget a request the agent has satisfied, or that nobody acted on for a day.
fn settle_update_request(host: &mut Host, at: f64) {
    if host.update_requested_at.is_some() && pending_update(host, at).is_none() {
        host.update_requested_at = None;
        host.update_target = None;
    }
}
/// Why a host's agent cannot be asked to update to `release`, or `None` when it can. An agent
/// reports that it has an updater as a collector; one without (installed before updaters, or as a
/// user service) has to be updated with the upgrade command on the host.
fn update_blocker(host: &Host, release: &str) -> Option<&'static str> {
    if host.agent_id.is_none() || host.agent_revoked || host.agent_uninstalled {
        return Some("This host has no active agent to update");
    }
    if !host
        .agent_version
        .as_deref()
        .is_some_and(|have| version_newer(release, have))
    {
        return Some(
            "This agent already runs the release the server offers, or has not reported its version",
        );
    }
    if !host
        .collectors
        .iter()
        .any(|c| c.name == "agent_update" && c.available)
    {
        return Some(
            "This agent has no updater yet. Run the upgrade command on the host once; later updates can start from H.O.P.E",
        );
    }
    None
}
async fn offered_release(state: &State) -> ApiResult<String> {
    let directory = crate::releases::release_directory(&state.0.release_dir).await;
    crate::releases::published_version(&directory)
        .await
        .ok_or_else(|| ApiError::bad("This server publishes no signed agent release to update to"))
}
async fn mark_update_requested(state: &State, host: &mut Host, release: &str) -> ApiResult<()> {
    host.update_requested_at = Some(now());
    host.update_target = Some(release.to_string());
    sqlx::query("UPDATE hosts SET body=? WHERE id=?")
        .bind(serde_json::to_string(&*host)?)
        .bind(&host.id)
        .execute(&state.0.db)
        .await?;
    state.host_changed(host);
    Ok(())
}
/// Ask one host's agent to update to the release this server offers. The agent learns of it in
/// its next report response and passes it to its local updater, which checks the signature itself.
pub async fn request_update(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    let _guard = state.0.writes.lock().await;
    let mut host = state.host(&id).await?.ok_or_else(ApiError::not_found)?;
    let release = offered_release(&state).await?;
    if let Some(reason) = update_blocker(&host, &release) {
        return Err(ApiError::bad(reason));
    }
    mark_update_requested(&state, &mut host, &release).await?;
    Ok(Json(OkResponse { ok: true }))
}
/// Ask every agent that runs an older release than the one on offer to update.
pub async fn request_all_updates(
    Extract(state): Extract<State>,
) -> ApiResult<Json<UpdateRequested>> {
    let _guard = state.0.writes.lock().await;
    let release = offered_release(&state).await?;
    let mut requested = 0;
    for mut host in state.hosts().await? {
        if update_blocker(&host, &release).is_none() {
            mark_update_requested(&state, &mut host, &release).await?;
            requested += 1;
        }
    }
    Ok(Json(UpdateRequested { requested }))
}

pub async fn revoke(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    revoke_by_id(&state, &id).await?;
    Ok(Json(OkResponse { ok: true }))
}

pub async fn complete_uninstall(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    complete_uninstall_by_id(&state, &id).await?;
    Ok(Json(OkResponse { ok: true }))
}

/// Mark an agent installation as removed. Its identity stays attached for metric history and
/// credential replacement, while the host returns to the discovered-device fleet.
pub async fn complete_uninstall_by_id(state: &State, id: &str) -> ApiResult<()> {
    let _guard = state.0.writes.lock().await;
    let networks = crate::api::networks(state).await?;
    let mut tx = state.0.db.begin().await?;
    let updated = sqlx::query("UPDATE agents SET revoked=1 WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(ApiError::not_found());
    }
    let body: Option<String> = sqlx::query_scalar("SELECT body FROM hosts WHERE agent_id=?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(body) = body else {
        return Err(ApiError::not_found());
    };
    let mut host: Host = serde_json::from_str(&body)?;
    host.agent_revoked = true;
    host.agent_uninstalled = true;
    host.update_requested_at = None;
    host.update_target = None;
    host.last_seen = 0.0;
    host.discovery_method = "tcp".into();
    host.primary_ip = crate::addresses::primary_ip(&host.addresses, &networks);
    host.name = host.primary_ip.clone().unwrap_or_else(|| host.id.clone());
    crate::classify::refresh(&mut host, &state.gateway_set());
    sqlx::query("UPDATE hosts SET name=?,body=? WHERE agent_id=?")
        .bind(&host.name)
        .bind(serde_json::to_string(&host)?)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.changed();
    Ok(())
}

pub async fn revoke_by_id(state: &State, id: &str) -> ApiResult<()> {
    let _guard = state.0.writes.lock().await;
    let mut tx = state.0.db.begin().await?;
    sqlx::query("UPDATE agents SET revoked=1 WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for mut host in state.hosts().await? {
        if host.agent_id.as_deref() == Some(id) {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_addresses_exclude_loopback_unspecified_and_link_local() {
        let inventory = Inventory {
            hostname: "host".into(),
            os: "Linux".into(),
            architecture: "x86_64".into(),
            cpu_model: "CPU".into(),
            cpu_cores: 1,
            memory_total_bytes: 1.0,
            interfaces: vec![hope_protocol::Interface {
                name: "eth0".into(),
                mac: None,
                addresses: vec![
                    "127.0.0.1".into(),
                    "0.0.0.0".into(),
                    "169.254.0.1".into(),
                    "fe80::1".into(),
                    "203.0.113.9".into(),
                ],
                gateway: None,
            }],
            disks: vec![],
            gpus: vec![],
            collectors: vec![],
        };
        assert_eq!(inventory_addresses(&inventory), vec!["203.0.113.9"]);
    }

    fn host_at(version: &str, requested: Option<f64>) -> Host {
        let mut host = Host::new("h".into(), "h".into(), "icmp");
        host.agent_id = Some("a".into());
        host.agent_version = Some(version.into());
        host.update_requested_at = requested;
        host.update_target = requested.map(|_| "0.1.60".to_string());
        host.collectors = vec![hope_protocol::CollectorStatus {
            name: "agent_update".into(),
            available: true,
            detail: None,
        }];
        host
    }
    #[test]
    fn a_request_is_pending_only_while_fresh_and_unsatisfied() {
        let day = hope_protocol::UPDATE_REQUEST_SECONDS;
        assert_eq!(
            pending_update(&host_at("0.1.57", Some(1000.0)), 1100.0).as_deref(),
            Some("0.1.60")
        );
        assert_eq!(pending_update(&host_at("0.1.57", None), 1100.0), None);
        assert_eq!(
            pending_update(&host_at("0.1.60", Some(1000.0)), 1100.0),
            None
        );
        assert_eq!(
            pending_update(&host_at("0.1.61", Some(1000.0)), 1100.0),
            None
        );
        assert_eq!(
            pending_update(&host_at("0.1.57", Some(1000.0)), 1000.0 + day + 1.0),
            None
        );
        let mut lapsed = host_at("0.1.57", Some(1000.0));
        settle_update_request(&mut lapsed, 1000.0 + day + 1.0);
        assert_eq!(
            (lapsed.update_requested_at, lapsed.update_target),
            (None, None)
        );
        let mut live = host_at("0.1.57", Some(1000.0));
        settle_update_request(&mut live, 1100.0);
        assert!(live.update_requested_at.is_some());
    }
    #[test]
    fn only_active_agents_with_an_updater_and_an_older_version_can_be_asked() {
        assert_eq!(update_blocker(&host_at("0.1.57", None), "0.1.60"), None);
        assert!(update_blocker(&host_at("0.1.60", None), "0.1.60").is_some());
        assert!(update_blocker(&host_at("0.1.61", None), "0.1.60").is_some());
        let mut revoked = host_at("0.1.57", None);
        revoked.agent_revoked = true;
        assert!(update_blocker(&revoked, "0.1.60").is_some());
        let mut unknown = host_at("0.1.57", None);
        unknown.agent_version = None;
        assert!(update_blocker(&unknown, "0.1.60").is_some());
        let mut no_agent = host_at("0.1.57", None);
        no_agent.agent_id = None;
        assert!(update_blocker(&no_agent, "0.1.60").is_some());
        // No updater reported, or one reported unavailable: the upgrade command is the way.
        let mut bare = host_at("0.1.57", None);
        bare.collectors.clear();
        assert!(update_blocker(&bare, "0.1.60").is_some_and(|why| why.contains("upgrade command")));
        let mut unavailable = host_at("0.1.57", None);
        unavailable.collectors[0].available = false;
        assert!(update_blocker(&unavailable, "0.1.60").is_some());
    }
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
