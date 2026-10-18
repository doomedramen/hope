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
use hope_protocol::{
    EnrollmentTokenRequest, EnrollmentTokenResponse, HISTORY_SAMPLE_SECONDS, OkResponse,
    ReportResponse, UPDATE_REQUEST_SECONDS, UpdateRequested, version_newer,
};
use sqlx::Row;
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
    if host.agent_id.is_none() || host.agent_revoked {
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
