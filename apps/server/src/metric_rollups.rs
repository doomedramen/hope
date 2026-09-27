//! Fixed time buckets, updated transactionally with accepted samples.
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

pub async fn accumulate(
    tx: &mut Transaction<'_, Postgres>,
    agent: Uuid,
    at: OffsetDateTime,
    metrics: &Value,
) -> sqlx::Result<()> {
    let mut numeric = BTreeMap::new();
    crate::agent_metrics::flatten_numeric_metrics(metrics, "", &mut numeric);
    // Delivery counters and timestamps are operational metadata, not chart dimensions.
    numeric.retain(|key, _| !key.starts_with("delivery."));
    for resolution in [300_i64, 3600] {
        let bucket = OffsetDateTime::from_unix_timestamp(
            at.unix_timestamp().div_euclid(resolution) * resolution,
        )
        .unwrap();
        sqlx::query("insert into agent_metric_rollups(agent_id,resolution,bucket_start,sample_count,stats) values($1,$2,$3,0,'{}') on conflict do nothing")
            .bind(agent).bind(resolution as i32).bind(bucket).execute(&mut **tx).await?;
        let values: Value=sqlx::query_scalar("select stats from agent_metric_rollups where agent_id=$1 and resolution=$2 and bucket_start=$3 for update").bind(agent).bind(resolution as i32).bind(bucket).fetch_one(&mut **tx).await?;
        let mut values = values.as_object().cloned().unwrap_or_default();
        for (key, value) in &numeric {
            let old = values.get(key);
            let count = old.and_then(|v| v["count"].as_u64()).unwrap_or(0) + 1;
            let sum = old.and_then(|v| v["sum"].as_f64()).unwrap_or(0.0) + value;
            let minimum = old
                .and_then(|v| v["minimum"].as_f64())
                .unwrap_or(*value)
                .min(*value);
            let maximum = old
                .and_then(|v| v["maximum"].as_f64())
                .unwrap_or(*value)
                .max(*value);
            let old_at = old.and_then(|v| v["last_at"].as_i64()).unwrap_or(i64::MIN);
            let latest = if at.unix_timestamp() >= old_at {
                *value
            } else {
                old.and_then(|v| v["latest"].as_f64()).unwrap_or(*value)
            };
            values.insert(key.clone(),json!({"count":count,"sum":sum,"average":sum/count as f64,"minimum":minimum,"maximum":maximum,"latest":latest,"last_at":at.unix_timestamp().max(old_at)}));
        }
        sqlx::query("update agent_metric_rollups set sample_count=sample_count+1,stats=$4 where agent_id=$1 and resolution=$2 and bucket_start=$3")
            .bind(agent).bind(resolution as i32).bind(bucket).bind(Value::Object(values)).execute(&mut **tx).await?;
    }
    Ok(())
}
pub async fn backfill_and_retain(pool: &PgPool) -> sqlx::Result<()> {
    loop {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid,Uuid,OffsetDateTime,Value)>=sqlx::query_as("select id,agent_id,collected_at,metrics from agent_metric_samples where rolled_up_at is null order by received_at,id limit 100 for update skip locked").fetch_all(&mut *tx).await?;
        for (id, agent, at, metrics) in &rows {
            accumulate(&mut tx, *agent, *at, metrics).await?;
            sqlx::query("update agent_metric_samples set rolled_up_at=now() where id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        if rows.len() < 100 {
            break;
        }
    }
    loop {
        let result=sqlx::query("delete from agent_metric_rollups where (agent_id,resolution,bucket_start) in (select agent_id,resolution,bucket_start from agent_metric_rollups where bucket_start < now()-case when resolution=300 then interval '30 days' else interval '180 days' end limit 1000)").execute(pool).await?;
        if result.rows_affected() < 1000 {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn replay_and_late_samples_preserve_bucket_extrema_and_latest() {
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
        let at = OffsetDateTime::now_utc().unix_timestamp().div_euclid(300) * 300 - 300;
        for (offset, value) in [(30, 90.0), (15, 10.0)] {
            let parsed=crate::agent_metrics::parse_metric_sample_batch_value(json!({"message_id":Uuid::new_v4(),"protocol_version":2,"type":"metric_sample_batch","schema_version":1,"agent_id":agent,"batch_id":Uuid::new_v4(),"samples":[{"sample_id":Uuid::new_v4(),"collected_at_unix_secs":at+offset,"metrics":{"cpu":{"usage_percent":value}}}]})).unwrap();
            for _ in 0..2 {
                crate::agent_metrics::ingest_metric_sample_batch(&pool, agent, 2, &parsed)
                    .await
                    .unwrap();
            }
        }
        let (count,stats):(i64,Value)=sqlx::query_as("select sample_count,stats from agent_metric_rollups where agent_id=$1 and resolution=300").bind(agent).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(stats["cpu.usage_percent"]["average"], 50.0);
        assert_eq!(stats["cpu.usage_percent"]["minimum"], 10.0);
        assert_eq!(stats["cpu.usage_percent"]["maximum"], 90.0);
        assert_eq!(stats["cpu.usage_percent"]["latest"], 90.0);
        backfill_and_retain(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "select sample_count from agent_metric_rollups where agent_id=$1 and resolution=300",
        )
        .bind(agent)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
        sqlx::query("delete from agents where id=$1")
            .bind(agent)
            .execute(&pool)
            .await
            .unwrap();
    }
}
