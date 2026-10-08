use crate::{
    error::{ApiError, ApiResult},
    state::{State, now},
};
use axum::{
    Json,
    extract::{Path, Query, State as Extract},
};
use hope_protocol::{MetricPoint, Metrics};
use sqlx::Row;
use std::collections::BTreeMap;

pub use hope_protocol::MetricWindow as Window;
pub async fn history(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Query(window): Query<Window>,
) -> ApiResult<Json<Vec<MetricPoint>>> {
    let host = state.host(&id).await?.ok_or_else(ApiError::not_found)?;
    let Some(agent) = host.agent_id else {
        return Ok(Json(vec![]));
    };
    let hours = window.hours.unwrap_or(24).clamp(1, 168);
    let rows=sqlx::query("SELECT collected_at,resolution,body FROM metrics WHERE agent_id=? AND collected_at>=? ORDER BY collected_at LIMIT 10000")
        .bind(agent).bind(now()-hours as f64*3600.0).fetch_all(&state.0.db).await?;
    let points: Vec<MetricPoint> = rows
        .into_iter()
        .map(|r| {
            Ok(MetricPoint {
                collected_at: r.get("collected_at"),
                resolution_seconds: r.get::<i64, _>("resolution") as u32,
                metrics: serde_json::from_str(r.get("body"))?,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Json(if hours > 24 {
        bucket(points, 900)
    } else if hours > 6 {
        bucket(points, 120)
    } else {
        points
    }))
}

fn average(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let values: Vec<_> = values.flatten().filter(|v| v.is_finite()).collect();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}
pub fn aggregate(values: &[Metrics]) -> Metrics {
    let mut result = values.last().cloned().unwrap_or_default();
    result.cpu_percent = average(values.iter().map(|v| v.cpu_percent));
    result.memory_used_bytes = average(values.iter().map(|v| v.memory_used_bytes));
    result.memory_total_bytes = average(values.iter().map(|v| v.memory_total_bytes));
    result.network_rx_bytes_per_second =
        average(values.iter().map(|v| v.network_rx_bytes_per_second));
    result.network_tx_bytes_per_second =
        average(values.iter().map(|v| v.network_tx_bytes_per_second));
    result.disk_read_bytes_per_second =
        average(values.iter().map(|v| v.disk_read_bytes_per_second));
    result.disk_write_bytes_per_second =
        average(values.iter().map(|v| v.disk_write_bytes_per_second));
    for gpu in &mut result.gpus {
        let matches: Vec<_> = values
            .iter()
            .flat_map(|v| &v.gpus)
            .filter(|g| g.name == gpu.name && g.vendor == gpu.vendor)
            .collect();
        gpu.utilization_percent = average(matches.iter().map(|g| g.utilization_percent));
        gpu.memory_used_bytes = average(matches.iter().map(|g| g.memory_used_bytes));
        gpu.temperature_celsius = average(matches.iter().map(|g| g.temperature_celsius));
    }
    result
}
pub fn bucket(points: Vec<MetricPoint>, seconds: u32) -> Vec<MetricPoint> {
    let mut groups: BTreeMap<i64, Vec<Metrics>> = BTreeMap::new();
    for point in points {
        groups
            .entry(point.collected_at as i64 / seconds as i64)
            .or_default()
            .push(point.metrics);
    }
    groups
        .into_iter()
        .map(|(t, values)| MetricPoint {
            collected_at: (t * seconds as i64) as f64,
            resolution_seconds: seconds,
            metrics: aggregate(&values),
        })
        .collect()
}

pub async fn retain(state: &State) -> anyhow::Result<()> {
    let _guard = state.0.writes.lock().await;
    let cutoff = ((now() - 86400.0) / 300.0).floor() * 300.0;
    // Work in bounded batches of complete five-minute groups. Never retain unbounded raw history.
    let groups=sqlx::query("SELECT agent_id,CAST(collected_at/300 AS INTEGER) AS bucket FROM metrics WHERE resolution=30 AND collected_at<? GROUP BY agent_id,bucket LIMIT 500")
        .bind(cutoff).fetch_all(&state.0.db).await?;
    let mut tx = state.0.db.begin().await?;
    for group in groups {
        let agent: String = group.get("agent_id");
        let bucket: i64 = group.get("bucket");
        let from = (bucket * 300) as f64;
        let rows=sqlx::query("SELECT body FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>=? AND collected_at<?")
            .bind(&agent).bind(from).bind(from+300.0).fetch_all(&mut *tx).await?;
        let values: Vec<Metrics> = rows
            .into_iter()
            .map(|r| serde_json::from_str(r.get("body")))
            .collect::<Result<_, _>>()?;
        sqlx::query("INSERT OR REPLACE INTO metrics(agent_id,report_id,collected_at,resolution,body) VALUES(?,?,?,300,?)")
            .bind(&agent).bind(format!("rollup-{bucket}")).bind(from).bind(serde_json::to_string(&aggregate(&values))?).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>=? AND collected_at<?").bind(&agent).bind(from).bind(from+300.0).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM metrics WHERE collected_at<?")
        .bind(now() - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM scans WHERE finished_at<?")
        .bind(now() - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
