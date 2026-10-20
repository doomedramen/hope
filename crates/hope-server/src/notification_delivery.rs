//! Bounded, durable, at-least-once delivery. No network request holds the database write lock.
use crate::{
    notify::{self, Message},
    state::{State, hash, now},
};
use futures::{StreamExt, stream};
use hope_protocol::NotificationSettings;
use sqlx::{Row, Sqlite, Transaction};

pub(crate) const MAX_PENDING: i64 = 4096;
const CONCURRENCY: i64 = 4;

pub(crate) fn destination(settings: &NotificationSettings) -> String {
    hash(&format!(
        "{}\n{}",
        settings.ntfy_server, settings.ntfy_topic
    ))
}
pub(crate) async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    settings: &NotificationSettings,
    key: &str,
    kind: &str,
    recovery: bool,
    message: &Message,
) -> anyhow::Result<bool> {
    Ok(sqlx::query("INSERT INTO notification_outbox(alert_key,host_id,kind,recovery,destination,body,next_attempt,created_at) SELECT ?,?,?,?,?,?,?,? WHERE (SELECT COUNT(*) FROM notification_outbox)<?")
        .bind(key).bind(&message.host_id).bind(kind).bind(recovery).bind(destination(settings))
        .bind(serde_json::to_string(message)?).bind(now()).bind(now()).bind(MAX_PENDING)
        .execute(&mut **tx).await?.rows_affected()==1)
}
fn wanted(settings: &NotificationSettings, kind: &str, recovery: bool) -> bool {
    settings.ntfy_enabled
        && !settings.ntfy_topic.is_empty()
        && (!recovery || settings.notify_recovery)
        && match kind {
            "offline" => settings.notify_offline,
            "resource" => settings.notify_resources,
            "new_device" => settings.notify_new_devices,
            _ => true,
        }
}

pub(crate) async fn deliver(state: &State) -> anyhow::Result<usize> {
    // There is one background worker. This also serializes explicit fixture invocations.
    let _delivery = state.0.notification_delivery.lock().await;
    let (settings, token) = notify::delivery_settings(state).await?;
    let rows=sqlx::query("SELECT id,alert_key,kind,recovery,destination,body,attempts FROM notification_outbox current WHERE next_attempt<=? AND NOT EXISTS(SELECT 1 FROM notification_outbox older WHERE older.alert_key=current.alert_key AND older.id<current.id) ORDER BY id LIMIT ?")
        .bind(now()).bind(CONCURRENCY).fetch_all(&state.0.db).await?;
    let outcomes = stream::iter(rows)
        .map(|row| {
            let settings = &settings;
            let token = token.as_deref();
            async move {
                let id: i64 = row.get("id");
                let attempts: i64 = row.get("attempts");
                let kind: String = row.get("kind");
                let recovery: bool = row.get("recovery");
                let destination: String = row.get("destination");
                if destination != self::destination(settings) || !wanted(settings, &kind, recovery)
                {
                    return Ok((id, attempts, true));
                }
                let message: Message = serde_json::from_str(row.get("body"))?;
                let accepted = notify::send(state, settings, token, message).await.is_ok();
                anyhow::Ok((id, attempts, accepted))
            }
        })
        .buffer_unordered(CONCURRENCY as usize)
        .collect::<Vec<_>>()
        .await;
    let _guard = state.0.writes.lock().await;
    let mut completed = 0;
    for outcome in outcomes {
        let (id, attempts, accepted) = outcome?;
        if accepted {
            sqlx::query("DELETE FROM notification_outbox WHERE id=?")
                .bind(id)
                .execute(&state.0.db)
                .await?;
            completed += 1;
        } else {
            let delay = (5u64.saturating_mul(2u64.saturating_pow(attempts.min(8) as u32))).min(900);
            sqlx::query("UPDATE notification_outbox SET attempts=MIN(attempts+1,1000000),next_attempt=? WHERE id=?")
                .bind(now()+delay as f64).bind(id).execute(&state.0.db).await?;
        }
    }
    Ok(completed)
}
