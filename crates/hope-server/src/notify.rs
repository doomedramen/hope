//! Notifications, delivered through ntfy (https://ntfy.sh or a self-hosted server).
//!
//! Problems are evaluated every minute into the `alerts` table whether or not notifications are
//! enabled, so turning them on never replays problems that were already open. A message is sent
//! when an alert opens and, if wanted, when it clears. New devices are announced as they are
//! first seen, except during a range's first scan (which would announce the whole network).
use crate::{
    error::{ApiError, ApiResult},
    notification_delivery,
    state::{State, now},
};
use axum::{Json, extract::State as Extract};
use hope_protocol::{Host, NotificationSettings, OkResponse};
use std::collections::HashMap;

const SETTINGS_KEY: &str = "notifications";
const TOKEN_KEY: &str = "ntfy_token";
/// An agent that has not reported for this long is offline (the console uses the same value).
pub const AGENT_STALE_SECONDS: f64 = 120.0;

pub async fn settings(state: &State) -> anyhow::Result<NotificationSettings> {
    let mut settings: NotificationSettings = match state.meta(SETTINGS_KEY).await? {
        Some(json) => serde_json::from_str(&json).unwrap_or_default(),
        None => NotificationSettings::default(),
    };
    settings.ntfy_token = None;
    settings.ntfy_token_set = state.meta(TOKEN_KEY).await?.is_some_and(|t| !t.is_empty());
    Ok(settings)
}

async fn store(state: &State, settings: &NotificationSettings) -> anyhow::Result<()> {
    let mut stored = settings.clone();
    stored.ntfy_token = None;
    stored.ntfy_token_set = false;
    state
        .set_meta(SETTINGS_KEY, &serde_json::to_string(&stored)?)
        .await
}

fn validate(input: &NotificationSettings) -> ApiResult<()> {
    let server = url::Url::parse(input.ntfy_server.trim()).map_err(|_| {
        ApiError::bad("Enter the ntfy server as a URL, for example https://ntfy.sh")
    })?;
    if !matches!(server.scheme(), "http" | "https") || server.host_str().is_none() {
        return Err(ApiError::bad(
            "The ntfy server must be an http:// or https:// URL",
        ));
    }
    let topic = input.ntfy_topic.trim();
    if topic.len() > 64
        || !topic
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad(
            "Use a topic of up to 64 letters, digits, hyphens and underscores",
        ));
    }
    if input.ntfy_enabled && topic.is_empty() {
        return Err(ApiError::bad(
            "Choose a topic before turning notifications on",
        ));
    }
    if let Some(token) = &input.ntfy_token
        && (token.len() > 256 || token.chars().any(|c| c.is_whitespace() || c.is_control()))
    {
        return Err(ApiError::bad("The access token is not valid"));
    }
    Ok(())
}

pub async fn get_settings(Extract(state): Extract<State>) -> ApiResult<Json<NotificationSettings>> {
    Ok(Json(settings(&state).await?))
}

pub async fn put_settings(
    Extract(state): Extract<State>,
    Json(input): Json<NotificationSettings>,
) -> ApiResult<Json<NotificationSettings>> {
    validate(&input)?;
    let _guard = state.0.writes.lock().await;
    let current = settings(&state).await?;
    let updated = NotificationSettings {
        ntfy_server: input.ntfy_server.trim().trim_end_matches('/').to_string(),
        ntfy_topic: input.ntfy_topic.trim().to_string(),
        last_sent_at: current.last_sent_at,
        last_error: current.last_error.clone(),
        ..input.clone()
    };
    store(&state, &updated).await?;
    if !updated.ntfy_enabled
        || notification_delivery::destination(&updated)
            != notification_delivery::destination(&current)
    {
        sqlx::query("DELETE FROM notification_outbox")
            .execute(&state.0.db)
            .await?;
    } else {
        sqlx::query("UPDATE notification_outbox SET next_attempt=?")
            .bind(now())
            .execute(&state.0.db)
            .await?;
    }
    match input.ntfy_token.as_deref() {
        Some("") => {
            sqlx::query("DELETE FROM meta WHERE key=?")
                .bind(TOKEN_KEY)
                .execute(&state.0.db)
                .await?;
        }
        Some(token) => state.set_meta(TOKEN_KEY, token).await?,
        None => {}
    }
    Ok(Json(settings(&state).await?))
}

pub async fn test(Extract(state): Extract<State>) -> ApiResult<Json<OkResponse>> {
    let (settings, token) = delivery_settings(&state).await?;
    if settings.ntfy_topic.is_empty() {
        return Err(ApiError::bad("Save a topic first"));
    }
    send(
        &state,
        &settings,
        token.as_deref(),
        Message {
            title: "H.O.P.E test notification".into(),
            body: "Notifications from this server reach you.".into(),
            priority: 3,
            tags: "white_check_mark".into(),
            host_id: None,
        },
    )
    .await
    .map_err(|error| ApiError::bad(format!("ntfy did not accept the message: {error}")))?;
    Ok(Json(OkResponse { ok: true }))
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Message {
    title: String,
    body: String,
    /// ntfy priority, 1 (min) to 5 (urgent).
    priority: u8,
    /// Comma-separated ntfy tags (emoji short codes).
    tags: String,
    pub(crate) host_id: Option<String>,
}

/// Bind credentials to the destination they were configured for before starting network I/O.
pub(crate) async fn delivery_settings(
    state: &State,
) -> anyhow::Result<(NotificationSettings, Option<String>)> {
    let _guard = state.0.writes.lock().await;
    Ok((
        settings(state).await?,
        state
            .meta(TOKEN_KEY)
            .await?
            .filter(|token| !token.is_empty()),
    ))
}

pub(crate) async fn send(
    state: &State,
    settings: &NotificationSettings,
    token: Option<&str>,
    message: Message,
) -> anyhow::Result<()> {
    let click = message
        .host_id
        .as_ref()
        .map(|id| format!("{}/?host={id}", state.0.server_url));
    let body = serde_json::json!({
        "topic": settings.ntfy_topic,
        "title": message.title,
        "message": message.body,
        "priority": message.priority,
        "tags": message.tags.split(',').collect::<Vec<_>>(),
        "click": click,
    });
    let mut request = state
        .0
        .notifier
        .post(&settings.ntfy_server)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&body)?);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let result = async {
        let mut response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let mut bytes = Vec::new();
            while bytes.len() < 1024 {
                let Some(chunk) = response.chunk().await? else {
                    break;
                };
                bytes.extend_from_slice(&chunk[..chunk.len().min(1024 - bytes.len())]);
            }
            let text = String::from_utf8_lossy(&bytes);
            anyhow::bail!(
                "HTTP {status}{}",
                crate::fingerprint::clean(&text, 160)
                    .map(|t| format!(": {t}"))
                    .unwrap_or_default()
            );
        }
        anyhow::Ok(())
    }
    .await;
    let _guard = state.0.writes.lock().await;
    let mut record = self::settings(state).await?;
    if notification_delivery::destination(&record) != notification_delivery::destination(settings) {
        return result;
    }
    match &result {
        Ok(()) => {
            record.last_sent_at = Some(now());
            record.last_error = None;
        }
        Err(error) => record.last_error = crate::fingerprint::clean(&error.to_string(), 200),
    }
    store(state, &record).await?;
    result
}

/// Deliver one bounded batch, independently of condition evaluation.
pub async fn deliver_pending(state: &State) -> anyhow::Result<usize> {
    notification_delivery::deliver(state).await
}

/// Announce a device seen for the first time, unless its range is still on its first scan.
pub async fn new_device(state: &State, host: &Host, network_id: &str) {
    let Ok(settings) = settings(state).await else {
        return;
    };
    if !settings.ntfy_enabled || !settings.notify_new_devices {
        return;
    }
    let scanned_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM scans WHERE network_id=? AND status='completed'")
            .bind(network_id)
            .fetch_one(&state.0.db)
            .await
            .unwrap_or(0);
    if scanned_before == 0 {
        return;
    }
    let address = host.addresses.first().cloned().unwrap_or_default();
    let detail = [host.vendor.clone(), host.macs.first().cloned()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    let message = Message {
        title: format!("New device on your network: {}", host.name),
        body: if detail.is_empty() {
            format!("{address} joined a monitored range.")
        } else {
            format!("{address} ({detail}) joined a monitored range.")
        },
        priority: 3,
        tags: "new".into(),
        host_id: Some(host.id.clone()),
    };
    let _guard = state.0.writes.lock().await;
    let queued = async {
        let mut tx = state.write_transaction().await?;
        let queued = notification_delivery::enqueue(
            &mut tx,
            &settings,
            &format!("{}:new_device", host.id),
            "new_device",
            false,
            &message,
        )
        .await?;
        tx.commit().await?;
        anyhow::Ok(queued)
    }
    .await;
    match queued {
        Ok(true) => {}
        Ok(false) => tracing::warn!("notification outbox is full; new-device announcement omitted"),
        Err(error) => tracing::warn!(%error, "new-device notification could not be queued"),
    }
}

/// A problem worth a notification: (alert key, category, severity tag, message).
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub key: String,
    pub offline: bool,
    pub critical: bool,
    pub message: String,
}

/// Problems on one host, mirroring the console's attention list (CPU and memory spikes excluded:
/// they are too noisy to push to a phone). `last_scan` maps a network to its latest completed
/// scan, used to tell "not seen" from "not looked for".
pub fn problems(host: &Host, now: f64, last_scan: &HashMap<String, (f64, u32)>) -> Vec<Problem> {
    let mut out = vec![];
    let mut push = |kind: &str, offline: bool, critical: bool, message: String| {
        out.push(Problem {
            key: format!("{}:{kind}", host.id),
            offline,
            critical,
            message,
        });
    };
    if host.agent_id.is_some() && !host.agent_revoked {
        let silent = host
            .last_report
            .is_none_or(|t| now - t > AGENT_STALE_SECONDS);
        if silent {
            push(
                "agent",
                true,
                true,
                format!("The agent on {} stopped reporting", host.name),
            );
        }
        if let Some(metrics) = host.metrics.as_ref().filter(|_| !silent) {
            for disk in &metrics.disks {
                if disk.total_bytes > 0.0 && !disk.is_guest_storage() {
                    let used = 100.0 * (1.0 - disk.available_bytes / disk.total_bytes);
                    if used >= 90.0 {
                        push(
                            &format!("disk:{}", disk.mount),
                            false,
                            used >= 97.0,
                            format!("{} on {} is {used:.0}% full", disk.mount, host.name),
                        );
                    }
                }
            }
            for pool in &metrics.pools {
                let used = if pool.size_bytes > 0.0 {
                    100.0 * pool.allocated_bytes / pool.size_bytes
                } else {
                    0.0
                };
                if !pool.health.eq_ignore_ascii_case("ONLINE") {
                    push(
                        &format!("pool:{}", pool.name),
                        false,
                        true,
                        format!("Pool {} on {} is {}", pool.name, host.name, pool.health),
                    );
                } else if used >= 90.0 {
                    push(
                        &format!("pool:{}", pool.name),
                        false,
                        used >= 97.0,
                        format!(
                            "Pool {} on {} is {used:.0}% allocated",
                            pool.name, host.name
                        ),
                    );
                }
            }
            if let Some(hot) = metrics
                .temperatures
                .iter()
                .max_by(|a, b| a.celsius.total_cmp(&b.celsius))
                && hot.celsius >= 85.0
            {
                push(
                    "sensor",
                    false,
                    hot.celsius >= 95.0,
                    format!("{} on {} is at {:.0}°C", hot.label, host.name, hot.celsius),
                );
            }
            let failed: Vec<_> = metrics
                .services
                .iter()
                .filter(|u| u.state == "failed")
                .collect();
            let failed_count = metrics
                .service_summary
                .map_or(0, |summary| summary.failed as usize)
                .max(failed.len());
            if let [unit] = failed.as_slice()
                && failed_count == 1
            {
                push(
                    "service",
                    false,
                    false,
                    format!("{} failed on {}", unit.name, host.name),
                );
            } else if failed_count > 0 {
                push(
                    "service",
                    false,
                    false,
                    format!(
                        "{} service {} failed on {}",
                        failed_count,
                        if failed_count == 1 { "unit" } else { "units" },
                        host.name
                    ),
                );
            }
            for monitor in metrics.monitors.iter().filter(|m| !m.up) {
                push(
                    &format!("monitor:{}", monitor.target),
                    false,
                    false,
                    format!("{} cannot reach {}", host.name, monitor.target),
                );
            }
        }
    } else if (host.agent_id.is_none() || host.agent_uninstalled)
        && host
            .device
            .as_ref()
            .is_some_and(|d| crate::classify::is_infrastructure(&d.kind))
    {
        // Infrastructure is offline when a scan of one of its ranges finished after the host was
        // last seen and at least two scan intervals (and 15 minutes) have passed.
        let missed = host
            .network_ids
            .iter()
            .filter_map(|id| last_scan.get(id))
            .any(|(finished, interval)| {
                *finished > host.last_seen
                    && now - host.last_seen > (2 * interval * 60).max(900) as f64
            });
        if missed {
            let label = host
                .device
                .as_ref()
                .map(|d| d.label.clone())
                .unwrap_or_default();
            push(
                "offline",
                true,
                true,
                format!("{} ({label}) stopped answering", host.name),
            );
        }
    }
    out
}

/// Unknown telemetry is not evidence that an existing resource problem recovered.
fn resource_observed(host: &Host, key: &str, at: f64) -> bool {
    if host.agent_revoked
        || host
            .last_report
            .is_none_or(|last| at - last > AGENT_STALE_SECONDS)
    {
        return false;
    }
    let Some(metrics) = &host.metrics else {
        return false;
    };
    let Some(kind) = key.strip_prefix(&format!("{}:", host.id)) else {
        return false;
    };
    let (collector, legacy_evidence) = if kind.starts_with("disk:") {
        ("storage", !metrics.disks.is_empty())
    } else if kind.starts_with("pool:") {
        ("storage_pools", !metrics.pools.is_empty())
    } else if kind == "service" {
        ("services", metrics.service_summary.is_some())
    } else if kind == "sensor" {
        ("temperatures", !metrics.temperatures.is_empty())
    } else if let Some(target) = kind.strip_prefix("monitor:") {
        return metrics
            .monitors
            .iter()
            .any(|monitor| monitor.target == target);
    } else {
        return false;
    };
    host.collectors
        .iter()
        .find(|status| status.name == collector)
        .map_or(legacy_evidence, |status| status.available)
}

/// Open and close alerts, notifying as configured.
pub async fn evaluate(state: &State) -> anyhow::Result<()> {
    let hosts = state.hosts().await?;
    let mut last_scan: HashMap<String, (f64, u32)> = HashMap::new();
    for network in crate::api::networks(state)
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?
    {
        if !network.enabled {
            continue;
        }
        let finished: Option<f64> = sqlx::query_scalar(
            "SELECT MAX(finished_at) FROM scans WHERE network_id=? AND status='completed'",
        )
        .bind(&network.id)
        .fetch_one(&state.0.db)
        .await?;
        if let Some(finished) = finished {
            last_scan.insert(network.id, (finished, network.interval_minutes));
        }
    }
    let now = now();
    let mut current: HashMap<String, (Problem, String)> = hosts
        .iter()
        .flat_map(|h| {
            problems(h, now, &last_scan)
                .into_iter()
                .filter(|p| !(p.offline && h.uptime.enabled && h.uptime.device))
                .map(|p| (p.key.clone(), (p, h.id.clone())))
        })
        .collect();
    for (problem, host_id) in crate::uptime::problems(state, &hosts, now, &last_scan).await? {
        current.insert(problem.key.clone(), (problem, host_id));
    }
    let _guard = state.0.writes.lock().await;
    let settings = settings(state).await?;
    let mut tx = state.write_transaction().await?;
    let open: HashMap<String, (String, String)> =
        sqlx::query_as::<_, (String, String, String)>("SELECT key, message, kind FROM alerts")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|(key, message, kind)| (key, (message, kind)))
            .collect();
    let enabled = settings.ntfy_enabled && !settings.ntfy_topic.is_empty();
    for (key, (problem, host_id)) in &current {
        if open.contains_key(key) {
            continue;
        }
        let wanted = if key.contains(":uptime:") {
            true
        } else if problem.offline {
            settings.notify_offline
        } else {
            settings.notify_resources
        };
        if enabled && wanted {
            let message = Message {
                title: problem.message.clone(),
                body: if problem.offline {
                    "Open H.O.P.E for its last report and history.".into()
                } else {
                    "Open H.O.P.E for details.".into()
                },
                priority: if problem.critical { 4 } else { 3 },
                tags: if problem.offline {
                    "red_circle"
                } else {
                    "warning"
                }
                .into(),
                host_id: Some(host_id.clone()),
            };
            let kind = if key.contains(":uptime:") {
                "uptime"
            } else if problem.offline {
                "offline"
            } else {
                "resource"
            };
            if !notification_delivery::enqueue(&mut tx, &settings, key, kind, false, &message)
                .await?
            {
                tracing::warn!("notification outbox is full; postponing alert transition");
                continue;
            }
        }
        sqlx::query(
            "INSERT OR IGNORE INTO alerts(key,host_id,kind,message,opened_at) VALUES(?,?,?,?,?)",
        )
        .bind(key)
        .bind(host_id)
        .bind(if key.contains(":uptime:") {
            "uptime"
        } else if problem.offline {
            "offline"
        } else {
            "resource"
        })
        .bind(&problem.message)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for (key, (message, kind)) in &open {
        if current.contains_key(key) {
            continue;
        }
        if kind == "resource"
            && let Some(host) = hosts
                .iter()
                .find(|host| key.starts_with(&format!("{}:", host.id)))
            && !resource_observed(host, key, now)
        {
            continue;
        }
        let wanted = if kind == "uptime" {
            hosts
                .iter()
                .any(|h| h.uptime.enabled && key.starts_with(&format!("{}:uptime:", h.id)))
        } else if kind == "offline" {
            settings.notify_offline
                && !hosts.iter().any(|h| {
                    h.uptime.enabled && h.uptime.device && key.starts_with(&format!("{}:", h.id))
                })
        } else {
            settings.notify_resources
        };
        if enabled && wanted && settings.notify_recovery {
            let host_id = key.split(':').next().map(str::to_string);
            let message = Message {
                title: format!("Resolved: {message}"),
                body: "This problem has cleared.".into(),
                priority: 2,
                tags: "white_check_mark".into(),
                host_id,
            };
            if !notification_delivery::enqueue(&mut tx, &settings, key, kind, true, &message)
                .await?
            {
                tracing::warn!("notification outbox is full; postponing recovery transition");
                continue;
            }
        }
        sqlx::query("DELETE FROM alerts WHERE key=?")
            .bind(key)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Independent, cancellation-aware tasks: a slow integration cannot delay discovery or evaluation.
pub fn spawn(state: State) {
    for delivery in [false, true] {
        let state = state.clone();
        let task = state.track();
        tokio::spawn(async move {
            let _task = task;
            if !delivery {
                tokio::select! {
                    _=state.0.shutdown.cancelled()=>return,
                    _=tokio::time::sleep(std::time::Duration::from_secs(20))=>{}
                }
            }
            let mut timer = tokio::time::interval(std::time::Duration::from_secs(if delivery {
                1
            } else {
                60
            }));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let work = async {
                    timer.tick().await;
                    if delivery {
                        deliver_pending(&state).await.map(|_| ())
                    } else {
                        evaluate(&state).await
                    }
                };
                tokio::select! {
                    _=state.0.shutdown.cancelled()=>break,
                    result=work=>if let Err(error)=result {tracing::warn!(%error,delivery,"notification background task failed");}
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hope_protocol::DeviceGuess;

    #[test]
    fn service_notifications_keep_the_full_failure_count_when_details_are_bounded() {
        let mut host = Host::new("a".into(), "fixture".into(), "agent");
        host.agent_id = Some("agent".into());
        host.last_report = Some(10_000.0);
        host.metrics = Some(hope_protocol::Metrics {
            services: vec![hope_protocol::ServiceUnit {
                name: "one.service".into(),
                state: "failed".into(),
                sub_state: "failed".into(),
                description: "fixture".into(),
                watched: true,
            }],
            service_summary: Some(hope_protocol::ServiceSummary {
                active: 1,
                failed: 200,
                total: 201,
            }),
            ..Default::default()
        });
        assert_eq!(
            problems(&host, 10_000.0, &HashMap::new())[0].message,
            "200 service units failed on fixture"
        );
        host.metrics.as_mut().unwrap().service_summary = None;
        assert_eq!(
            problems(&host, 10_000.0, &HashMap::new())[0].message,
            "one.service failed on fixture"
        );
        host.metrics.as_mut().unwrap().services.clear();
        host.metrics.as_mut().unwrap().service_summary = Some(hope_protocol::ServiceSummary {
            active: 1,
            failed: 1,
            total: 2,
        });
        assert_eq!(
            problems(&host, 10_000.0, &HashMap::new())[0].message,
            "1 service unit failed on fixture"
        );
    }

    #[test]
    fn agents_and_infrastructure_raise_offline_problems_only_when_looked_for() {
        let now = 10_000.0;
        let mut agent = Host::new("a".into(), "pve1".into(), "agent");
        agent.agent_id = Some("id".into());
        agent.last_report = Some(now - 30.0);
        assert!(problems(&agent, now, &HashMap::new()).is_empty());
        agent.last_report = Some(now - 600.0);
        let found = problems(&agent, now, &HashMap::new());
        assert_eq!(found.len(), 1);
        assert!(found[0].offline && found[0].message.contains("pve1"));

        let mut router = Host::new("r".into(), "gateway".into(), "icmp");
        router.network_ids = vec!["lan".into()];
        router.device = Some(DeviceGuess {
            kind: "router".into(),
            label: "Router".into(),
            confidence: 90,
            reasons: vec![],
            manual: false,
        });
        router.last_seen = now - 3600.0;
        let scanned_after = HashMap::from([("lan".to_string(), (now - 60.0, 5))]);
        let scanned_before = HashMap::from([("lan".to_string(), (now - 7200.0, 5))]);
        assert_eq!(problems(&router, now, &scanned_after).len(), 1);
        assert!(
            problems(&router, now, &scanned_before).is_empty(),
            "no scan since it was seen"
        );
        let mut phone = router.clone();
        phone.device.as_mut().unwrap().kind = "phone".into();
        assert!(
            problems(&phone, now, &scanned_after).is_empty(),
            "phones come and go"
        );
    }

    #[test]
    fn settings_validation() {
        let mut s = NotificationSettings::default();
        assert!(validate(&s).is_ok());
        s.ntfy_enabled = true;
        assert!(validate(&s).is_err(), "topic required");
        s.ntfy_topic = "hope-alerts_1".into();
        assert!(validate(&s).is_ok());
        s.ntfy_topic = "bad topic".into();
        assert!(validate(&s).is_err());
        s.ntfy_topic = "ok".into();
        s.ntfy_server = "ftp://x".into();
        assert!(validate(&s).is_err());
        s.ntfy_server = "https://ntfy.example.com".into();
        s.ntfy_token = Some("tk_abc def".into());
        assert!(validate(&s).is_err());
    }
}
