//! Agent-authenticated log ingestion and operator-scoped search/configuration.
use crate::inventory::pagination::{decode_cursor, encode_cursor};
use crate::{auth_mw::CurrentUser, state::AppState};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error": message})))
}
fn database(error_value: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::error!(error = %error_value, "agent log database operation failed");
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Agent data is temporarily unavailable",
    )
}

pub async fn ingest(
    pool: &PgPool,
    agent: Uuid,
    batch: &protocol::LogBatch,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let active: Option<(Uuid,)> =
        sqlx::query_as("select id from agents where id = $1 and revoked_at is null for share")
            .bind(agent)
            .fetch_optional(&mut *tx)
            .await?;
    if active.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }
    for entry in &batch.entries {
        let at = OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(entry.observed_at_unix_ms) * 1_000_000,
        )
        .map_err(|_| sqlx::Error::Protocol("invalid log timestamp".into()))?;
        sqlx::query("insert into agent_log_entries(agent_id, event_id, observed_at, source, severity, message, attributes) values ($1,$2,$3,$4,$5,$6,$7) on conflict (agent_id,event_id) do nothing")
            .bind(agent).bind(&entry.event_id).bind(at).bind(&entry.source).bind(&entry.severity).bind(&entry.message).bind(&entry.attributes).execute(&mut *tx).await?;
    }
    tx.commit().await
}

#[derive(Default, Deserialize)]
pub struct LogQuery {
    pub q: Option<String>,
    pub source: Option<String>,
    pub severity: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}
pub async fn list(
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(query): Query<LogQuery>,
) -> ApiResult {
    if query.q.as_ref().is_some_and(|q| q.len() > 256)
        || query.source.as_ref().is_some_and(|s| s.len() > 256)
    {
        return Err(error(StatusCode::BAD_REQUEST, "Search is too long"));
    }
    let to = query
        .to
        .unwrap_or_else(|| OffsetDateTime::now_utc().unix_timestamp());
    let from = query.from.unwrap_or(to - 86400);
    if from > to || to.saturating_sub(from) > 90 * 86400 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Use a time range of at most 90 days",
        ));
    }
    let from = OffsetDateTime::from_unix_timestamp(from)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Invalid start time"))?;
    let to = OffsetDateTime::from_unix_timestamp(to)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "Invalid end time"))?;
    let cursor = decode_cursor(&query.cursor);
    if query.cursor.is_some() && cursor.is_none() {
        return Err(error(StatusCode::BAD_REQUEST, "Invalid cursor"));
    }
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let rows: Vec<(Value,)> = sqlx::query_as("select row_to_json(t) from (select id, event_id, observed_at, received_at, source, severity, message, attributes from agent_log_entries where agent_id=$1 and observed_at >= $2 and observed_at <= $3 and ($4::text is null or source=$4) and ($5::text is null or severity=$5) and ($6::text is null or to_tsvector('simple',message) @@ websearch_to_tsquery('simple',$6)) and ($7::timestamptz is null or (received_at,id) < ($7::timestamptz,$8::uuid)) order by received_at desc,id desc limit $9) t")
        .bind(agent).bind(from).bind(to).bind(query.source.filter(|s| !s.is_empty())).bind(query.severity.filter(|s| !s.is_empty())).bind(query.q.filter(|s| !s.trim().is_empty()))
        .bind(cursor.as_ref().map(|(time,_)| time.clone())).bind(cursor.as_ref().map(|(_,id)| *id)).bind(limit + 1).fetch_all(&state.pool).await.map_err(database)?;
    let more = rows.len() > limit as usize;
    let items: Vec<Value> = rows
        .into_iter()
        .take(limit as usize)
        .map(|(row,)| row)
        .collect();
    let next = if more {
        items.last().and_then(|item| {
            Some(encode_cursor(
                item["received_at"].as_str()?,
                Uuid::parse_str(item["id"].as_str()?).ok()?,
            ))
        })
    } else {
        None
    };
    Ok(Json(
        json!({"items": items, "next_cursor": next, "limit": limit, "from": from, "to": to}),
    ))
}

pub async fn effective_config(
    pool: &PgPool,
    agent: Uuid,
) -> Result<protocol::CollectionConfig, sqlx::Error> {
    let row: Option<(i64, Value)> =
        sqlx::query_as("select revision, config from agent_collection_settings where agent_id=$1")
            .bind(agent)
            .fetch_optional(pool)
            .await?;
    let Some((revision, config)) = row else {
        return Ok(Default::default());
    };
    let mut settings: protocol::CollectionConfig =
        serde_json::from_value(config).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    settings.revision = revision;
    Ok(settings)
}
pub async fn get_settings(State(state): State<AppState>, Path(agent): Path<Uuid>) -> ApiResult {
    let settings = effective_config(&state.pool, agent)
        .await
        .map_err(database)?;
    let row: Option<(Option<i64>, i32)> = sqlx::query_as("select applied_revision,log_retention_days from agent_collection_settings where agent_id=$1").bind(agent).fetch_optional(&state.pool).await.map_err(database)?;
    let supports_controls: bool = sqlx::query_scalar("select coalesce((select capabilities ? 'log_source_controls' from agents where id=$1), false)")
        .bind(agent).fetch_one(&state.pool).await.map_err(database)?;
    Ok(Json(
        json!({"supports_source_controls":supports_controls,"config": settings, "applied_revision": row.as_ref().and_then(|r| r.0), "log_retention_days": row.map_or(7, |r| r.1)}),
    ))
}
#[derive(Deserialize)]
pub struct SettingsRequest {
    pub config: protocol::CollectionConfig,
    pub log_retention_days: i32,
}
pub async fn put_settings(
    State(state): State<AppState>,
    Path(agent): Path<Uuid>,
    Extension(CurrentUser(actor)): Extension<CurrentUser>,
    Json(request): Json<SettingsRequest>,
) -> ApiResult {
    request.config.validate().map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "Invalid source names, redactions, or configuration bounds",
        )
    })?;
    if !(1..=90).contains(&request.log_retention_days) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Retention must be between 1 and 90 days",
        ));
    }
    if !request.config.source_policies.is_empty() {
        let supported: bool = sqlx::query_scalar("select coalesce((select capabilities ? 'log_source_controls' from agents where id=$1),false)")
            .bind(agent).fetch_one(&state.pool).await.map_err(database)?;
        if !supported {
            return Err(error(
                StatusCode::CONFLICT,
                "Update the agent before setting source budgets or severity controls",
            ));
        }
    }
    let mut tx = state.pool.begin().await.map_err(database)?;
    let active: bool = sqlx::query_scalar(
        "select exists(select 1 from agents where id=$1 and revoked_at is null)",
    )
    .bind(agent)
    .fetch_one(&mut *tx)
    .await
    .map_err(database)?;
    if !active {
        return Err(error(StatusCode::NOT_FOUND, "Active agent not found"));
    }
    sqlx::query(
        "insert into agent_collection_settings(agent_id) values ($1) on conflict do nothing",
    )
    .bind(agent)
    .execute(&mut *tx)
    .await
    .map_err(database)?;
    let revision: Option<i64> = sqlx::query_scalar("update agent_collection_settings set config=$2, revision=revision+1, log_retention_days=$3, updated_at=now(), updated_by=$4 where agent_id=$1 and revision=$5 returning revision")
        .bind(agent).bind(serde_json::to_value(&request.config).unwrap()).bind(request.log_retention_days).bind(actor).bind(request.config.revision).fetch_optional(&mut *tx).await.map_err(database)?;
    let Some(revision) = revision else {
        return Err(error(
            StatusCode::CONFLICT,
            "Settings changed; reload before saving",
        ));
    };
    // Audit source names/counts only. Redaction values can themselves be secrets.
    sqlx::query("insert into audit_events(actor_user_id,actor_kind,action,target_kind,target_id,result,detail) values ($1,'operator','agent.collection.updated','agents',$2,'success',$3)")
        .bind(actor).bind(agent).bind(json!({"revision":revision,"journal_units":request.config.journal_units,"docker_containers":request.config.docker_containers,"redaction_count":request.config.redact.len(),"source_policies":request.config.source_policies})).execute(&mut *tx).await.map_err(database)?;
    tx.commit().await.map_err(database)?;
    Ok(Json(json!({"revision": revision})))
}
pub async fn acknowledge_config(
    pool: &PgPool,
    agent: Uuid,
    ack: &protocol::CollectionConfigAck,
) -> Result<(), sqlx::Error> {
    if ack.accepted {
        sqlx::query("update agent_collection_settings set applied_revision=$2, applied_at=now() where agent_id=$1 and revision=$2 and (coalesce(config->'source_policies','{}')='{}'::jsonb or exists(select 1 from agents where id=$1 and capabilities ? 'log_source_controls'))").bind(agent).bind(ack.revision).execute(pool).await?;
    }
    Ok(())
}
pub async fn retention(pool: &PgPool) -> Result<(), sqlx::Error> {
    loop {
        let result = sqlx::query("delete from agent_log_entries where id in (select l.id from agent_log_entries l left join agent_collection_settings s on s.agent_id=l.agent_id where l.received_at < now() - make_interval(days => case when l.source='agent' then 30 else coalesce(s.log_retention_days,7) end) order by l.received_at limit 1000)").execute(pool).await?;
        if result.rows_affected() < 1000 {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn log_replay_search_pagination_and_revocation() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let agent = Uuid::new_v4();
        sqlx::query("insert into agents(id,cert_fingerprint,cert_serial) values($1,$2,$2)")
            .bind(agent)
            .bind(agent.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let batch = protocol::LogBatch {
            agent_id: agent,
            batch_id: Uuid::new_v4(),
            entries: (0..3)
                .map(|i| protocol::LogEntry {
                    event_id: format!("source-{i}"),
                    observed_at_unix_ms: OffsetDateTime::now_utc().unix_timestamp() * 1000,
                    source: "docker:app".into(),
                    severity: "unknown".into(),
                    message: "same repeated message".into(),
                    attributes: json!({"stream":"stdout"}),
                })
                .collect(),
        };
        ingest(&pool, agent, &batch).await.unwrap();
        ingest(&pool, agent, &batch).await.unwrap();
        let page = list(
            State(AppState { pool: pool.clone() }),
            Path(agent),
            Query(LogQuery {
                q: Some("repeated".into()),
                limit: Some(2),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page["items"].as_array().unwrap().len(), 2);
        let next = page["next_cursor"].as_str().unwrap().to_owned();
        let page = list(
            State(AppState { pool: pool.clone() }),
            Path(agent),
            Query(LogQuery {
                q: Some("repeated".into()),
                cursor: Some(next),
                limit: Some(2),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert!(page["next_cursor"].is_null());
        crate::agents::revoke(&pool, agent).await.unwrap();
        assert!(matches!(
            ingest(&pool, agent, &batch).await,
            Err(sqlx::Error::RowNotFound)
        ));
        sqlx::query("delete from agents where id=$1")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn collection_configuration_uses_optimistic_revision_and_safe_audit() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let agent = Uuid::new_v4();
        sqlx::query("insert into agents(id,cert_fingerprint,cert_serial) values($1,$2,$2)")
            .bind(agent)
            .bind(agent.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let user: Uuid = sqlx::query_scalar(
            "insert into users(email,password_hash) values($1,'test') returning id",
        )
        .bind(format!("{agent}@example.test"))
        .fetch_one(&pool)
        .await
        .unwrap();
        let settings = || SettingsRequest {
            config: protocol::CollectionConfig {
                journal_units: vec!["app.service".into()],
                redact: vec!["private-pattern".into()],
                ..Default::default()
            },
            log_retention_days: 7,
        };
        let state = AppState { pool: pool.clone() };
        assert_eq!(
            put_settings(
                State(state.clone()),
                Path(agent),
                Extension(CurrentUser(user)),
                Json(settings())
            )
            .await
            .unwrap()
            .0["revision"],
            1
        );
        assert_eq!(
            put_settings(
                State(state.clone()),
                Path(agent),
                Extension(CurrentUser(user)),
                Json(settings())
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        acknowledge_config(
            &pool,
            agent,
            &protocol::CollectionConfigAck {
                revision: 1,
                accepted: true,
            },
        )
        .await
        .unwrap();
        let value = get_settings(State(state.clone()), Path(agent))
            .await
            .unwrap()
            .0;
        assert_eq!(value["applied_revision"], 1);
        let bounded = || {
            let mut request = settings();
            request.config.revision = 1;
            request.config.source_policies.insert(
                "journal:app.service".into(),
                protocol::LogSourcePolicy::default(),
            );
            request
        };
        assert_eq!(
            put_settings(
                State(state.clone()),
                Path(agent),
                Extension(CurrentUser(user)),
                Json(bounded())
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        sqlx::query(
            "update agents set capabilities='[\"log_source_controls\"]'::jsonb where id=$1",
        )
        .bind(agent)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            put_settings(
                State(state.clone()),
                Path(agent),
                Extension(CurrentUser(user)),
                Json(bounded())
            )
            .await
            .unwrap()
            .0["revision"],
            2
        );
        // A rollback to an older agent must not falsely acknowledge ignored controls.
        sqlx::query("update agents set capabilities='[]'::jsonb where id=$1")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
        acknowledge_config(
            &pool,
            agent,
            &protocol::CollectionConfigAck {
                revision: 2,
                accepted: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            get_settings(State(state), Path(agent)).await.unwrap().0["applied_revision"],
            1
        );
        let audits: Vec<Value> =
            sqlx::query_scalar("select detail from audit_events where target_id=$1")
                .bind(agent)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            !serde_json::to_string(&audits)
                .unwrap()
                .contains("private-pattern")
        );
        sqlx::query("delete from agents where id=$1")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
    }
}
