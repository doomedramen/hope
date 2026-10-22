use crate::{
    auth::limited,
    error::{ApiError, ApiResult},
    inventory::StoredHost,
    limits::ClientKey,
    state::{State, hash, now, secret},
};
use axum::{
    Json,
    extract::{Extension, Path, State as Extract},
    http::{HeaderMap, StatusCode},
};
use hope_protocol::{
    AgentReport, AssociationUpdate, EnrollmentRequest, EnrollmentResponse, Host, Inventory,
    SCHEMA_VERSION,
};
use hope_protocol::{
    EnrollmentTokenRequest, EnrollmentTokenResponse, HISTORY_SAMPLE_SECONDS, OkResponse,
    ReportResponse, UPDATE_REQUEST_SECONDS, UpdateRequested, version_newer,
};
use sqlx::{Row, Sqlite, Transaction};
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
        command: starter_command(&state, "", "uninstall"),
        custom_ca_required: state.0.enrollment_ca.is_some(),
    }))
}

/// A word the shell reads as itself, quoted only when it has to be, so commands stay readable.
fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~:/%+=,@-".contains(&b))
    {
        value.to_string()
    } else {
        shell_quote(value)
    }
}

/// The command people run: `curl -fsS <server>/install | sh -s -- <argument>`. The served starter
/// already knows the server and custom CA; `environment` is set for the installer (`NAME=value `).
pub fn starter_command(state: &State, environment: &str, argument: &str) -> String {
    let curl_ca = state
        .0
        .enrollment_ca
        .as_ref()
        .map(|path| format!(" --cacert {}", shell_word(path)))
        .unwrap_or_default();
    let url = shell_word(&format!("{}/install", state.0.server_url));
    format!(
        "curl -fsS{curl_ca} {url} | {environment}sh -s -- {}",
        shell_word(argument)
    )
}

/// The self-contained uninstall script an SSH job sends. Console users get `starter_command`.
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
    starter_command(state, "", "upgrade")
}

/// The self-contained install script an SSH job sends. Console users get `starter_command`.
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
        command: starter_command(
            &state,
            if body.reset_agent_id.is_some() {
                "HOPE_RESET_CREDENTIAL=1 "
            } else {
                ""
            },
            &token,
        ),
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
            let mut tx = state.write_transaction().await?;
            ensure_agent_capacity(&mut tx).await?;
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
            let mut host = StoredHost::decode(&body)?;
            host.agent_revoked = false;
            host.agent_uninstalled = false;
            sqlx::query("UPDATE hosts SET body=? WHERE agent_id=?")
                .bind(serde_json::to_string(&host)?)
                .bind(&request.agent_id)
                .execute(&mut *tx)
                .await?;
            State::clear_retired_host(&mut tx, &host.id).await?;
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
    let addresses = inventory_addresses(&request.inventory);
    let macs = inventory_macs(&request.inventory);
    let hosts = state.stored_hosts().await?;
    // IP alone is insufficient: DHCP may have reassigned it since discovery.
    let candidates: Vec<_> = hosts
        .into_iter()
        .filter(|h| h.agent_id.is_none() && h.macs.iter().any(|m| macs.contains(m)))
        .collect();
    let mut host = if candidates.len() == 1 {
        candidates[0].clone()
    } else {
        StoredHost::new(Uuid::new_v4().to_string(), String::new(), "agent")
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
    let mut tx = state.write_transaction().await?;
    ensure_agent_capacity(&mut tx).await?;
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
    State::clear_retired_host(&mut tx, &host.id).await?;
    crate::identity::reconcile(&mut tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(EnrollmentResponse {
        credential,
        report_interval_seconds: 30,
    }))
}

async fn ensure_agent_capacity(tx: &mut Transaction<'_, Sqlite>) -> ApiResult<()> {
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE revoked=0")
        .fetch_one(&mut **tx)
        .await?;
    if active >= 250 {
        return Err(ApiError::bad(
            "This installation supports at most 250 active agent identities",
        ));
    }
    Ok(())
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
        || report
            .sequence
            .is_some_and(|sequence| sequence == 0 || sequence > hope_protocol::MAX_REPORT_SEQUENCE)
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
    let mut tx = state.write_transaction().await?;
    let row = sqlx::query("SELECT body FROM hosts WHERE agent_id=?")
        .bind(&report.agent_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut host = StoredHost::decode(row.get("body"))?;
    let head: Option<(i64, String)> =
        sqlx::query_as("SELECT sequence,report_id FROM agent_report_heads WHERE agent_id=?")
            .bind(&report.agent_id)
            .fetch_optional(&mut *tx)
            .await?;
    let live_applied = match (report.sequence, &head) {
        (Some(sequence), Some((prior, _))) => sequence > *prior as u64,
        (Some(_), None) => true,
        // Once an identity supports ordering, old queued reports and older binaries cannot
        // replace its live measurements. Their valid history remains ingestible.
        (None, Some(_)) => false,
        (None, None) => report.collected_at >= host.last_collected_at.unwrap_or(0.0),
    };
    let duplicate = head.as_ref().is_some_and(|(_, id)| id == &report.report_id)
        || sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM metrics WHERE agent_id=? AND report_id=?)",
        )
        .bind(&report.agent_id)
        .bind(&report.report_id)
        .fetch_one(&mut *tx)
        .await?;
    let mut response = ReportResponse {
        accepted: true,
        duplicate,
        live_applied: Some(
            (live_applied && (!duplicate || head.is_none()))
                || head.as_ref().is_some_and(|(_, id)| id == &report.report_id),
        ),
        report_sequence: report
            .sequence
            .filter(|_| live_applied && !duplicate)
            .or_else(|| head.as_ref().map(|(sequence, _)| *sequence as u64)),
        report_interval_seconds: Some(state.report_interval()),
        update_to: pending_update(&host, now()),
        update_release_id: pending_update(&host, now()).and(host.update_target_release_id.clone()),
    };
    if duplicate {
        return Ok(Json(response));
    }
    State::clear_retired_host(&mut tx, &host.id).await?;
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
    .fetch_one(&mut *tx)
    .await?;
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
    let identity_changed =
        (live_applied && report.inventory.is_some()) || host.last_seen < now() - 7.0 * 86400.0;
    host.last_report = Some(now());
    host.last_seen = now();
    if let Some(version) = agent_version(&headers) {
        host.agent_version = Some(version);
    }
    // A backlog delivered after an outage fills history but never replaces newer live state.
    if live_applied {
        host.last_collected_at = Some(report.collected_at);
        host.metrics_received_at = Some(now());
        if let Some(sequence) = report.sequence {
            sqlx::query("INSERT INTO agent_report_heads(agent_id,sequence,report_id) VALUES(?,?,?) ON CONFLICT(agent_id) DO UPDATE SET sequence=excluded.sequence,report_id=excluded.report_id")
                .bind(&report.agent_id).bind(sequence as i64).bind(&report.report_id)
                .execute(&mut *tx).await?;
        }
        if let Some(inv) = report.inventory {
            host.name = inv.hostname.clone();
            host.addresses = inventory_addresses(&inv);
            host.macs = inventory_macs(&inv);
            host.inventory = Some(inv);
        }
        host.metrics = Some(report.metrics);
        host.collectors = report.collectors;
    }
    let mut rollout_success = false;
    let mut rollout_failure = None;
    // A failed manual request ends here rather than being retried hourly for the rest of its day.
    if live_applied
        && host.update_request_source.as_deref() == Some("manual")
        && let Some(release_id) = host.update_target_release_id.clone()
        && reported_failure(&mut host, &release_id)
    {
        host.update_requested_at = None;
        host.update_target = None;
        host.update_target_release_id = None;
        host.update_request_source = None;
    }
    if live_applied
        && host.update_request_source.as_deref() == Some("automatic")
        && let (Some(target), Some(release_id)) = (
            host.update_target.as_deref(),
            host.update_target_release_id.clone(),
        )
    {
        if host
            .update_requested_at
            .is_some_and(|requested| now() - requested >= UPDATE_REQUEST_SECONDS)
        {
            rollout_failure = Some(release_id.clone());
            host.auto_update_failed_release_id = Some(release_id);
            host.update_requested_at = None;
            host.update_target = None;
            host.update_target_release_id = None;
            host.update_request_source = None;
        } else if host
            .agent_version
            .as_deref()
            .is_some_and(|have| !version_newer(target, have))
        {
            rollout_success = true;
            host.auto_update_failed_release_id = None;
            host.update_requested_at = None;
            host.update_target = None;
            host.update_target_release_id = None;
            host.update_request_source = None;
        } else if reported_failure(&mut host, &release_id) {
            rollout_failure = Some(release_id.clone());
            host.auto_update_failed_release_id = Some(release_id);
            host.update_requested_at = None;
            host.update_target = None;
            host.update_target_release_id = None;
            host.update_request_source = None;
        }
    }
    settle_update_request(&mut host, now());
    response.update_to = pending_update(&host, now());
    response.update_release_id = response
        .update_to
        .as_ref()
        .and(host.update_target_release_id.clone());
    host.network_ids = crate::api::matching_networks(&state, &host.addresses).await?;
    crate::classify::refresh(&mut host, &state.gateway_set());
    sqlx::query("UPDATE hosts SET name=?,body=? WHERE agent_id=?")
        .bind(&host.name)
        .bind(serde_json::to_string(&host)?)
        .bind(&report.agent_id)
        .execute(&mut *tx)
        .await?;
    let association_changed = if identity_changed {
        crate::identity::reconcile(&mut tx).await?
    } else {
        false
    };
    if rollout_success {
        let canary = meta_in_transaction(&mut tx, "auto_update_canary_agent")
            .await?
            .unwrap_or_default();
        if canary == report.agent_id {
            set_meta_in_transaction(&mut tx, "auto_update_canary_passed", "true").await?;
            set_meta_in_transaction(&mut tx, "auto_update_last_queued_at", &now().to_string())
                .await?;
        }
    }
    if let Some(release_id) = rollout_failure {
        set_meta_in_transaction(&mut tx, "auto_update_blocked_release_id", &release_id).await?;
    }
    tx.commit().await?;
    if let Err(error) = schedule_auto_update(&state, now()).await {
        eprintln!("Automatic agent update scheduling failed: {}", error.1);
    }
    if let Ok(hosts) = state.stored_hosts().await
        && let Some(updated_host) = hosts
            .into_iter()
            .find(|host| host.agent_id.as_deref() == Some(&report.agent_id))
    {
        response.update_to = pending_update(&updated_host, now());
        response.update_release_id = response
            .update_to
            .as_ref()
            .and(updated_host.update_target_release_id.clone());
    }
    state.publish_host(&host.id).await;
    if association_changed {
        state.changed();
    }
    Ok(Json(response))
}

/// Confirm, reject, or unlink a source-to-scan-device association without deleting source data.
pub async fn update_association(
    Extract(state): Extract<State>,
    Path(source_device_id): Path<String>,
    Json(update): Json<AssociationUpdate>,
) -> ApiResult<Json<Host>> {
    Ok(Json(
        crate::identity::change(&state, &source_device_id, update).await?,
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
    if !host.inventory.as_ref().is_some_and(|inventory| {
        is_linux_os(&inventory.os) && inventory.architecture.eq_ignore_ascii_case("x86_64")
    }) {
        return Some("Agent releases currently target Linux x86_64 hosts only");
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
            "This agent has no working updater. Run the upgrade command on the host once; later updates can start from H.O.P.E",
        );
    }
    None
}

fn is_linux_os(os: &str) -> bool {
    os.to_ascii_lowercase().contains("linux")
}

fn reports_update_failure_for(detail: &str, release_id: &str) -> bool {
    detail.starts_with("The last update failed")
        && detail.ends_with(&format!("for release {release_id}"))
}

fn failure_reported(host: &Host, release_id: &str) -> bool {
    host.collectors.iter().any(|collector| {
        collector.name == "agent_update"
            && collector
                .detail
                .as_deref()
                .is_some_and(|detail| reports_update_failure_for(detail, release_id))
    })
}

/// Whether the latest report says the requested release failed, ignoring a failure that was
/// already on display when the request was made until the agent has reported something else.
fn reported_failure(host: &mut Host, release_id: &str) -> bool {
    let failed = failure_reported(host, release_id);
    if host.update_failure_predates_request {
        host.update_failure_predates_request = failed;
        return false;
    }
    failed
}

fn auto_eligible(host: &Host, release: &str, release_id: &str) -> bool {
    !host.auto_update_excluded
        && host.auto_update_failed_release_id.as_deref() != Some(release_id)
        && host.inventory.as_ref().is_some_and(|inventory| {
            is_linux_os(&inventory.os) && inventory.architecture.eq_ignore_ascii_case("x86_64")
        })
        && update_blocker(host, release).is_none()
}

/// Schedule at most one automatic update at a time. A single canary must report success before
/// the fleet advances; subsequent requests are separated by fifteen minutes.
async fn schedule_auto_update(state: &State, at: f64) -> ApiResult<()> {
    let Some((release_id, release)) =
        crate::releases::current_linux_x86_64(&state.0.release_dir).await
    else {
        return Ok(());
    };
    let mut tx = state.write_transaction().await?;
    if meta_in_transaction(&mut tx, "agent_auto_updates_enabled")
        .await?
        .is_some_and(|enabled| enabled != "true")
    {
        return Ok(());
    }
    if meta_in_transaction(&mut tx, "auto_update_blocked_release_id")
        .await?
        .as_deref()
        == Some(&release_id)
    {
        return Ok(());
    }
    if meta_in_transaction(&mut tx, "auto_update_release_id")
        .await?
        .as_deref()
        != Some(&release_id)
    {
        set_meta_in_transaction(&mut tx, "auto_update_release_id", &release_id).await?;
        set_meta_in_transaction(&mut tx, "auto_update_canary_agent", "").await?;
        set_meta_in_transaction(&mut tx, "auto_update_canary_passed", "false").await?;
        set_meta_in_transaction(&mut tx, "auto_update_blocked_release_id", "").await?;
        set_meta_in_transaction(&mut tx, "auto_update_last_queued_at", "0").await?;
    }
    let mut hosts = stored_hosts_in_transaction(&mut tx).await?;
    if let Some(index) = hosts.iter().position(|host| {
        host.update_request_source.as_deref() == Some("automatic")
            && host
                .update_requested_at
                .is_some_and(|requested| at - requested >= UPDATE_REQUEST_SECONDS)
    }) {
        let expired = &mut hosts[index];
        let expired_release = expired
            .update_target_release_id
            .clone()
            .unwrap_or_else(|| release_id.clone());
        expired.auto_update_failed_release_id = Some(expired_release.clone());
        expired.update_requested_at = None;
        expired.update_target = None;
        expired.update_target_release_id = None;
        expired.update_request_source = None;
        sqlx::query("UPDATE hosts SET body=? WHERE id=?")
            .bind(serde_json::to_string(&expired)?)
            .bind(&expired.id)
            .execute(&mut *tx)
            .await?;
        if expired_release == release_id {
            set_meta_in_transaction(&mut tx, "auto_update_blocked_release_id", &release_id).await?;
            let expired_id = expired.id.clone();
            tx.commit().await?;
            state.publish_host(&expired_id).await;
            return Ok(());
        }
    }
    let canary_passed = meta_in_transaction(&mut tx, "auto_update_canary_passed")
        .await?
        .as_deref()
        == Some("true");
    if !canary_passed {
        let mut canary = meta_in_transaction(&mut tx, "auto_update_canary_agent")
            .await?
            .unwrap_or_default();

        // The request and canary marker used to be written separately. If the server stopped
        // between those writes, recover the already-persisted request instead of selecting a
        // second host and updating both at once.
        if let Some(in_flight) = hosts.iter().find(|host| {
            host.update_request_source.as_deref() == Some("automatic")
                && pending_update(host, at).is_some()
        }) {
            if canary.is_empty()
                && in_flight.update_target_release_id.as_deref() == Some(&release_id)
            {
                canary = in_flight
                    .agent_id
                    .clone()
                    .unwrap_or_else(|| in_flight.id.clone());
                set_meta_in_transaction(&mut tx, "auto_update_canary_agent", &canary).await?;
            }
            tx.commit().await?;
            return Ok(());
        }

        if !canary.is_empty()
            && let Some(host) = hosts
                .iter()
                .find(|host| host.agent_id.as_deref().unwrap_or(&host.id) == canary)
        {
            if host
                .agent_version
                .as_deref()
                .is_some_and(|have| !version_newer(&release, have))
            {
                // Repair older partial state where the agent reported success before the
                // rollout marker was persisted.
                set_meta_in_transaction(&mut tx, "auto_update_canary_passed", "true").await?;
                set_meta_in_transaction(&mut tx, "auto_update_last_queued_at", &at.to_string())
                    .await?;
                tx.commit().await?;
                return Ok(());
            }
            if !auto_eligible(host, &release, &release_id) {
                canary.clear();
                set_meta_in_transaction(&mut tx, "auto_update_canary_agent", "").await?;
            }
        }

        hosts.sort_by(|a, b| a.id.cmp(&b.id));
        let candidate = if canary.is_empty() {
            hosts.iter().find(|host| {
                auto_eligible(host, &release, &release_id) && pending_update(host, at).is_none()
            })
        } else {
            hosts.iter().find(|host| {
                host.agent_id.as_deref().unwrap_or(&host.id) == canary
                    && auto_eligible(host, &release, &release_id)
                    && pending_update(host, at).is_none()
            })
        };
        if let Some(mut host) = candidate.cloned() {
            set_update_request(&mut host, &release_id, &release, "automatic", at);
            sqlx::query("UPDATE hosts SET body=? WHERE id=?")
                .bind(serde_json::to_string(&host)?)
                .bind(&host.id)
                .execute(&mut *tx)
                .await?;
            let agent_id = host.agent_id.clone().unwrap_or_else(|| host.id.clone());
            set_meta_in_transaction(&mut tx, "auto_update_canary_agent", &agent_id).await?;
            set_meta_in_transaction(&mut tx, "auto_update_last_queued_at", &at.to_string()).await?;
            let host_id = host.id.clone();
            tx.commit().await?;
            state.publish_host(&host_id).await;
            return Ok(());
        }
        tx.commit().await?;
        return Ok(());
    }
    let last: f64 = meta_in_transaction(&mut tx, "auto_update_last_queued_at")
        .await?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    if at - last < 900.0 {
        tx.commit().await?;
        return Ok(());
    }
    if hosts.iter().any(|host| {
        host.update_request_source.as_deref() == Some("automatic")
            && pending_update(host, at).is_some()
    }) {
        tx.commit().await?;
        return Ok(());
    }
    hosts.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(mut host) = hosts.into_iter().find(|host| {
        auto_eligible(host, &release, &release_id) && pending_update(host, at).is_none()
    }) {
        set_update_request(&mut host, &release_id, &release, "automatic", at);
        sqlx::query("UPDATE hosts SET body=? WHERE id=?")
            .bind(serde_json::to_string(&host)?)
            .bind(&host.id)
            .execute(&mut *tx)
            .await?;
        set_meta_in_transaction(&mut tx, "auto_update_last_queued_at", &at.to_string()).await?;
        let host_id = host.id.clone();
        tx.commit().await?;
        state.publish_host(&host_id).await;
    } else {
        tx.commit().await?;
    }
    Ok(())
}

async fn meta_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    key: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key=?")
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?)
}

async fn set_meta_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    key: &str,
    value: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        .bind(key)
        .bind(value)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn stored_hosts_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
) -> ApiResult<Vec<StoredHost>> {
    let bodies: Vec<String> =
        sqlx::query_scalar("SELECT body FROM hosts ORDER BY name COLLATE NOCASE")
            .fetch_all(&mut **tx)
            .await?;
    bodies
        .iter()
        .map(|body| StoredHost::decode(body).map_err(ApiError::from))
        .collect()
}
async fn offered_release(state: &State) -> ApiResult<(String, String)> {
    crate::releases::current_linux_x86_64(&state.0.release_dir)
        .await
        .ok_or_else(|| {
            ApiError::bad(
                "This server publishes no signed agent release for Linux x86_64 to update to",
            )
        })
}
async fn mark_update_requested(
    state: &State,
    host: &mut StoredHost,
    release_id: &str,
    release: &str,
    source: &str,
) -> ApiResult<()> {
    set_update_request(host, release_id, release, source, now());
    sqlx::query("UPDATE hosts SET body=? WHERE id=?")
        .bind(serde_json::to_string(&*host)?)
        .bind(&host.id)
        .execute(&state.0.db)
        .await?;
    state.publish_host(&host.id).await;
    Ok(())
}
fn set_update_request(
    host: &mut StoredHost,
    release_id: &str,
    release: &str,
    source: &str,
    at: f64,
) {
    host.update_requested_at = Some(at);
    host.update_target = Some(release.to_string());
    host.update_target_release_id = Some(release_id.to_string());
    host.update_request_source = Some(source.to_string());
    host.update_failure_predates_request = failure_reported(host, release_id);
}
/// Ask one host's agent to update to the release this server offers. The agent learns of it in
/// its next report response and passes it to its local updater, which checks the signature itself.
pub async fn request_update(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    let _guard = state.0.writes.lock().await;
    let mut host = state
        .stored_host(&id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (release_id, release) = offered_release(&state).await?;
    if let Some(reason) = update_blocker(&host, &release) {
        return Err(ApiError::bad(reason));
    }
    mark_update_requested(&state, &mut host, &release_id, &release, "manual").await?;
    Ok(Json(OkResponse { ok: true }))
}
/// Ask every agent that runs an older release than the one on offer to update.
pub async fn request_all_updates(
    Extract(state): Extract<State>,
) -> ApiResult<Json<UpdateRequested>> {
    let _guard = state.0.writes.lock().await;
    let (release_id, release) = offered_release(&state).await?;
    let mut requested = 0;
    for mut host in state.stored_hosts().await? {
        if update_blocker(&host, &release).is_none() {
            mark_update_requested(&state, &mut host, &release_id, &release, "manual").await?;
            requested += 1;
        }
    }
    Ok(Json(UpdateRequested { requested }))
}

/// Exclude or restore one host in automatic Linux x86_64 release rollouts.
pub async fn set_auto_update_exclusion(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Json(body): Json<hope_protocol::AgentAutoUpdateExclusion>,
) -> ApiResult<Json<hope_protocol::AgentAutoUpdateExclusion>> {
    let _guard = state.0.writes.lock().await;
    let Some(mut host) = state.stored_host(&id).await? else {
        return Err(ApiError::not_found());
    };
    host.auto_update_excluded = body.excluded;
    if body.excluded && host.update_request_source.as_deref() == Some("automatic") {
        host.update_requested_at = None;
        host.update_target = None;
        host.update_target_release_id = None;
        host.update_request_source = None;
    }
    if body.excluded
        && state.meta("auto_update_canary_agent").await?.as_deref()
            == Some(host.agent_id.as_deref().unwrap_or(&host.id))
    {
        state.set_meta("auto_update_canary_agent", "").await?;
    }
    sqlx::query("UPDATE hosts SET body=? WHERE id=?")
        .bind(serde_json::to_string(&host)?)
        .bind(&host.id)
        .execute(&state.0.db)
        .await?;
    state.publish_host(&host.id).await;
    Ok(Json(body))
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
    let mut tx = state.write_transaction().await?;
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
    let mut host = StoredHost::decode(&body)?;
    host.agent_revoked = true;
    host.agent_uninstalled = true;
    host.update_requested_at = None;
    host.update_target = None;
    host.update_target_release_id = None;
    host.update_request_source = None;
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
    let mut tx = state.write_transaction().await?;
    sqlx::query("UPDATE agents SET revoked=1 WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for mut host in state.stored_hosts().await? {
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
            os: "Debian GNU/Linux 13".into(),
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
        host.inventory = Some(Inventory {
            hostname: "h".into(),
            os: "Linux".into(),
            architecture: "x86_64".into(),
            cpu_model: "cpu".into(),
            cpu_cores: 1,
            memory_total_bytes: 1.0,
            interfaces: vec![],
            disks: vec![],
            gpus: vec![],
            collectors: vec![],
        });
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
    fn automatic_updates_require_linux_x86_64_and_respect_opt_out_and_failure() {
        let mut host = host_at("0.1.57", None);
        assert!(auto_eligible(&host, "0.1.82", "0.1.82-a1b2c3d4e5f6"));
        host.auto_update_excluded = true;
        assert!(!auto_eligible(&host, "0.1.82", "0.1.82-a1b2c3d4e5f6"));
        host.auto_update_excluded = false;
        host.auto_update_failed_release_id = Some("0.1.82-a1b2c3d4e5f6".into());
        assert!(!auto_eligible(&host, "0.1.82", "0.1.82-a1b2c3d4e5f6"));
        host.auto_update_failed_release_id = None;
        host.inventory.as_mut().unwrap().architecture = "aarch64".into();
        assert!(update_blocker(&host, "0.1.82").is_some());
    }

    #[test]
    fn an_old_update_failure_does_not_block_a_different_immutable_release() {
        assert!(reports_update_failure_for(
            "The last update failed (MANIFEST_INVALID) for release 0.1.82-a1b2c3d4e5f6",
            "0.1.82-a1b2c3d4e5f6"
        ));
        assert!(!reports_update_failure_for(
            "The last update failed (MANIFEST_INVALID) for release 0.1.82-a1b2c3d4e5f6",
            "0.1.83-f1e2d3c4b5a6"
        ));
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
