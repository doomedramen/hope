use crate::{
    error::{ApiError, ApiResult},
    state::{State, now},
};
use axum::{
    Json,
    extract::{Path, Query, State as Extract},
};
use hope_protocol::{MetricPoint, Metrics};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::BTreeMap;

pub use hope_protocol::MetricWindow as Window;
#[derive(Clone, Default, Serialize, Deserialize)]
struct Statistic {
    sum: f64,
    count: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Statistics(BTreeMap<String, Statistic>);
impl Statistics {
    fn sample(metrics: &Metrics) -> Self {
        let mut result = Self::default();
        macro_rules! scalar { ($($field:ident),*) => { $(result.add(stringify!($field), metrics.$field);)* }; }
        scalar!(
            cpu_percent,
            memory_used_bytes,
            memory_total_bytes,
            memory_cached_bytes,
            memory_zfs_arc_bytes,
            swap_used_bytes,
            swap_total_bytes,
            network_rx_bytes_per_second,
            network_tx_bytes_per_second,
            disk_read_bytes_per_second,
            disk_write_bytes_per_second
        );
        if let Some(load) = metrics.load_average {
            result.add("load:one", Some(load.one));
            result.add("load:five", Some(load.five));
            result.add("load:fifteen", Some(load.fifteen));
        }
        for interface in &metrics.network_interfaces {
            let key = format!("network:{}", serde_json::json!(interface.name));
            result.add(&format!("{key}:rx"), interface.rx_bytes_per_second);
            result.add(&format!("{key}:tx"), interface.tx_bytes_per_second);
        }
        for sensor in &metrics.temperatures {
            result.add(
                &format!("temperature:{}", serde_json::json!(sensor.sensor)),
                Some(sensor.celsius),
            );
        }
        for device in &metrics.disk_io {
            let key = format!("device:{}", serde_json::json!(device.device));
            result.add(&format!("{key}:read"), device.read_bytes_per_second);
            result.add(&format!("{key}:write"), device.write_bytes_per_second);
            result.add(&format!("{key}:busy"), device.busy_percent);
        }
        for pool in &metrics.pools {
            let key = format!("pool:{}", serde_json::json!(pool.name));
            result.add(&format!("{key}:size"), Some(pool.size_bytes));
            result.add(&format!("{key}:allocated"), Some(pool.allocated_bytes));
            result.add(&format!("{key}:read"), pool.read_bytes_per_second);
            result.add(&format!("{key}:write"), pool.write_bytes_per_second);
        }
        for monitor in &metrics.monitors {
            let key = format!("monitor:{}", serde_json::json!(monitor.target));
            result.add(&format!("{key}:latency"), monitor.latency_ms);
            result.add(
                &format!("{key}:up"),
                Some(if monitor.up { 1.0 } else { 0.0 }),
            );
        }
        for gpu in &metrics.gpus {
            if gpu.id.is_none() {
                continue;
            }
            let key = gpu_key(gpu.id.as_deref(), &gpu.vendor, &gpu.name);
            result.add(&format!("{key}:utilization"), gpu.utilization_percent);
            result.add(&format!("{key}:used"), gpu.memory_used_bytes);
            result.add(&format!("{key}:total"), gpu.memory_total_bytes);
            result.add(&format!("{key}:temperature"), gpu.temperature_celsius);
        }
        for disk in &metrics.disks {
            let key = disk_key(&disk.name, &disk.mount, &disk.filesystem);
            result.add(&format!("{key}:available"), Some(disk.available_bytes));
            result.add(&format!("{key}:total"), Some(disk.total_bytes));
        }
        result
    }
    fn add(&mut self, key: &str, value: Option<f64>) {
        if let Some(value) = value.filter(|v| v.is_finite()) {
            let entry = self.0.entry(key.into()).or_default();
            entry.sum += value;
            entry.count += 1;
        }
    }
    fn merge(&mut self, other: &Self) {
        for (key, value) in &other.0 {
            let entry = self.0.entry(key.clone()).or_default();
            entry.sum += value.sum;
            entry.count += value.count;
        }
    }
    fn mean(&self, key: &str) -> Option<f64> {
        self.0
            .get(key)
            .filter(|s| s.count > 0)
            .map(|s| s.sum / s.count as f64)
    }
    fn apply(&self, mut latest: Metrics) -> Metrics {
        macro_rules! scalar { ($($field:ident),*) => { $(latest.$field = self.mean(stringify!($field));)* }; }
        scalar!(
            cpu_percent,
            memory_used_bytes,
            memory_total_bytes,
            memory_cached_bytes,
            memory_zfs_arc_bytes,
            swap_used_bytes,
            swap_total_bytes,
            network_rx_bytes_per_second,
            network_tx_bytes_per_second,
            disk_read_bytes_per_second,
            disk_write_bytes_per_second
        );
        latest.load_average = match (
            self.mean("load:one"),
            self.mean("load:five"),
            self.mean("load:fifteen"),
        ) {
            (Some(one), Some(five), Some(fifteen)) => {
                Some(hope_protocol::LoadAverage { one, five, fifteen })
            }
            _ => None,
        };
        for interface in &mut latest.network_interfaces {
            let key = format!("network:{}", serde_json::json!(interface.name));
            interface.rx_bytes_per_second = self.mean(&format!("{key}:rx"));
            interface.tx_bytes_per_second = self.mean(&format!("{key}:tx"));
        }
        for sensor in &mut latest.temperatures {
            if let Some(value) =
                self.mean(&format!("temperature:{}", serde_json::json!(sensor.sensor)))
            {
                sensor.celsius = value;
            }
        }
        for device in &mut latest.disk_io {
            let key = format!("device:{}", serde_json::json!(device.device));
            device.read_bytes_per_second = self.mean(&format!("{key}:read"));
            device.write_bytes_per_second = self.mean(&format!("{key}:write"));
            device.busy_percent = self.mean(&format!("{key}:busy"));
        }
        for pool in &mut latest.pools {
            let key = format!("pool:{}", serde_json::json!(pool.name));
            if let Some(value) = self.mean(&format!("{key}:size")) {
                pool.size_bytes = value;
            }
            if let Some(value) = self.mean(&format!("{key}:allocated")) {
                pool.allocated_bytes = value;
            }
            pool.read_bytes_per_second = self.mean(&format!("{key}:read"));
            pool.write_bytes_per_second = self.mean(&format!("{key}:write"));
        }
        for monitor in &mut latest.monitors {
            let key = format!("monitor:{}", serde_json::json!(monitor.target));
            monitor.latency_ms = self.mean(&format!("{key}:latency"));
            // An aggregate is "up" only when every sample in it was up.
            if let Some(up) = self.mean(&format!("{key}:up")) {
                monitor.up = up >= 1.0;
            }
        }
        for gpu in &mut latest.gpus {
            if gpu.id.is_none() {
                continue;
            }
            let key = gpu_key(gpu.id.as_deref(), &gpu.vendor, &gpu.name);
            gpu.utilization_percent = self.mean(&format!("{key}:utilization"));
            gpu.memory_used_bytes = self.mean(&format!("{key}:used"));
            gpu.memory_total_bytes = self.mean(&format!("{key}:total"));
            gpu.temperature_celsius = self.mean(&format!("{key}:temperature"));
        }
        for disk in &mut latest.disks {
            let key = disk_key(&disk.name, &disk.mount, &disk.filesystem);
            if let Some(value) = self.mean(&format!("{key}:available")) {
                disk.available_bytes = value;
            }
            if let Some(value) = self.mean(&format!("{key}:total")) {
                disk.total_bytes = value;
            }
        }
        latest
    }
}
fn gpu_key(id: Option<&str>, vendor: &str, name: &str) -> String {
    id.map(|id| format!("gpu-id:{id}"))
        .unwrap_or_else(|| format!("gpu:{}", serde_json::json!([vendor, name])))
}
fn disk_key(name: &str, mount: &str, filesystem: &str) -> String {
    format!("disk:{}", serde_json::json!([name, mount, filesystem]))
}

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
    let since = now() - hours as f64 * 3600.0;
    let rows = sqlx::query("SELECT collected_at,resolution,body,statistics FROM metrics WHERE agent_id=? AND (collected_at>=? OR (resolution>30 AND collected_at+resolution>?)) ORDER BY collected_at DESC LIMIT 10000")
        .bind(agent).bind(since).bind(since).fetch_all(&state.0.db).await?;
    let mut points = vec![];
    for row in rows.into_iter().rev() {
        let point = MetricPoint {
            collected_at: row.get("collected_at"),
            resolution_seconds: row.get::<i64, _>("resolution") as u32,
            metrics: serde_json::from_str(row.get("body"))?,
        };
        let statistics: Option<String> = row.get("statistics");
        let statistics = statistics
            .map(|s| serde_json::from_str::<Statistics>(&s))
            .transpose()?;
        let statistics = statistics.or_else(|| {
            (point.resolution_seconds == 30).then(|| Statistics::sample(&point.metrics))
        });
        points.push((point, statistics));
    }
    let seconds = if hours > 24 {
        900
    } else if hours > 6 {
        120
    } else {
        0
    };
    // Legacy rollups have no recoverable per-metric counts. Preserve their values rather
    // than inventing weights; they expire naturally after seven days.
    if seconds == 0 || points.iter().any(|(_, statistics)| statistics.is_none()) {
        return Ok(Json(points.into_iter().map(|(point, _)| point).collect()));
    }
    Ok(Json(weighted_bucket(points, seconds)))
}
fn weighted_bucket(
    points: Vec<(MetricPoint, Option<Statistics>)>,
    seconds: u32,
) -> Vec<MetricPoint> {
    let mut groups: BTreeMap<i64, (Metrics, Statistics)> = BTreeMap::new();
    for (point, statistics) in points {
        let entry = groups
            .entry((point.collected_at / seconds as f64).floor() as i64)
            .or_default();
        entry.0 = point.metrics;
        if let Some(statistics) = statistics {
            entry.1.merge(&statistics);
        }
    }
    groups
        .into_iter()
        .map(|(time, (latest, statistics))| MetricPoint {
            collected_at: (time * seconds as i64) as f64,
            resolution_seconds: seconds,
            metrics: statistics.apply(latest),
        })
        .collect()
}
pub fn aggregate(values: &[Metrics]) -> Metrics {
    let mut statistics = Statistics::default();
    for value in values {
        statistics.merge(&Statistics::sample(value));
    }
    statistics.apply(values.last().cloned().unwrap_or_default())
}
pub async fn retain(state: &State) -> anyhow::Result<()> {
    retain_at(state, now()).await
}
async fn retain_at(state: &State, reference: f64) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let cutoff = ((reference - 86400.0) / 300.0).floor() * 300.0;
    let groups = sqlx::query("SELECT raw.agent_id,CAST(raw.collected_at/300 AS INTEGER) AS bucket FROM metrics AS raw WHERE raw.resolution=30 AND raw.collected_at<? AND NOT EXISTS(SELECT 1 FROM metrics AS legacy WHERE legacy.agent_id=raw.agent_id AND legacy.report_id='rollup-' || CAST(raw.collected_at/300 AS INTEGER) AND legacy.statistics IS NULL) GROUP BY raw.agent_id,bucket LIMIT 128")
        .bind(cutoff).fetch_all(&state.0.db).await?;
    for group in groups {
        if start.elapsed() > std::time::Duration::from_secs(2) {
            break;
        }
        // Release the write lock and commit after each bucket so live reports can proceed.
        let _guard = state.0.writes.lock().await;
        let agent: String = group.get("agent_id");
        let bucket: i64 = group.get("bucket");
        let from = (bucket * 300) as f64;
        let mut tx = state.0.db.begin().await?;
        let existing =
            sqlx::query("SELECT body,statistics FROM metrics WHERE agent_id=? AND report_id=?")
                .bind(&agent)
                .bind(format!("rollup-{bucket}"))
                .fetch_optional(&mut *tx)
                .await?;
        let mut statistics = Statistics::default();
        let mut latest = Metrics::default();
        if let Some(existing) = existing {
            let stored: Option<String> = existing.get("statistics");
            let Some(stored) = stored else {
                // Exceptional pre-upgrade late samples remain intact until expiry. A legacy
                // aggregate's unknown weights cannot be reconstructed without fabricating data.
                tx.commit().await?;
                continue;
            };
            statistics = serde_json::from_str(&stored)?;
            latest = serde_json::from_str(existing.get("body"))?;
        }
        let rows = sqlx::query("SELECT body FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>=? AND collected_at<? ORDER BY collected_at")
            .bind(&agent).bind(from).bind(from+300.0).fetch_all(&mut *tx).await?;
        for row in rows {
            latest = serde_json::from_str(row.get("body"))?;
            statistics.merge(&Statistics::sample(&latest));
        }
        sqlx::query("INSERT OR REPLACE INTO metrics(agent_id,report_id,collected_at,resolution,body,statistics) VALUES(?,?,?,300,?,?)")
            .bind(&agent).bind(format!("rollup-{bucket}")).bind(from).bind(serde_json::to_string(&statistics.apply(latest))?).bind(serde_json::to_string(&statistics)?).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>=? AND collected_at<?")
            .bind(&agent).bind(from).bind(from+300.0).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    let _guard = state.0.writes.lock().await;
    let mut tx = state.0.db.begin().await?;
    sqlx::query("DELETE FROM metrics WHERE rowid IN (SELECT rowid FROM metrics WHERE collected_at<? AND (resolution=30 OR collected_at+resolution<=?) LIMIT 5000)")
        .bind(reference - 7.0 * 86400.0)
        .bind(reference - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM scans WHERE rowid IN (SELECT rowid FROM scans WHERE finished_at<? LIMIT 500)",
    )
    .bind(reference - 7.0 * 86400.0)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn retention_boundaries_and_restart_preserve_partial_buckets() {
        let directory = tempfile::tempdir().expect("temporary");
        let path = directory.path().join("history.db");
        let capabilities = hope_protocol::Capabilities {
            platform: "test".into(),
            discovery_mode: "limited".into(),
            arp: false,
            icmp: false,
            limitations: vec![],
        };
        let state = State::open(
            path.to_str().expect("path"),
            "http://localhost:8080".into(),
            "releases".into(),
            capabilities.clone(),
        )
        .await
        .expect("state");
        let reference = 1_700_000_123.0;
        let raw_cutoff = ((reference - 86400.0) / 300.0_f64).floor() * 300.0;
        let expiry = reference - 7.0 * 86400.0;
        for (id, time) in [
            ("before-day", raw_cutoff - 1.0),
            ("at-day", raw_cutoff),
            ("after-day", reference - 86400.0 + 1.0),
            ("expired", expiry - 301.0),
            ("partial-seven-day", expiry + 1.0),
        ] {
            sqlx::query(
                "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent',?,?,?)",
            )
            .bind(id)
            .bind(time)
            .bind(
                serde_json::to_string(&Metrics {
                    cpu_percent: Some(42.0),
                    ..Default::default()
                })
                .expect("json"),
            )
            .execute(&state.0.db)
            .await
            .expect("sample");
        }
        retain_at(&state, reference).await.expect("retention");
        let raw: Vec<String> = sqlx::query_scalar(
            "SELECT report_id FROM metrics WHERE resolution=30 ORDER BY report_id",
        )
        .fetch_all(&state.0.db)
        .await
        .expect("raw");
        assert_eq!(raw, vec!["after-day", "at-day"]);
        let partial = (expiry / 300.0).floor() * 300.0;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM metrics WHERE resolution=300 AND collected_at=?",
        )
        .bind(partial)
        .fetch_one(&state.0.db)
        .await
        .expect("partial bucket");
        assert_eq!(
            count, 1,
            "Keep a bucket while any part of it is within seven days"
        );
        state.0.db.close().await;
        let reopened = State::open(
            path.to_str().expect("path"),
            "http://localhost:8080".into(),
            "releases".into(),
            capabilities,
        )
        .await
        .expect("restart");
        retain_at(&reopened, reference)
            .await
            .expect("repeat after restart");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics")
            .fetch_one(&reopened.0.db)
            .await
            .expect("count");
        assert_eq!(count, 4);
        retain_at(&reopened, partial + 300.0 + 7.0 * 86400.0)
            .await
            .expect("expired full bucket");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metrics WHERE collected_at=?")
            .bind(partial)
            .fetch_one(&reopened.0.db)
            .await
            .expect("count");
        assert_eq!(count, 0);
    }
    #[test]
    fn identical_gpu_models_keep_separate_adapter_averages() {
        let gpu = |id: &str, utilization| hope_protocol::Gpu {
            id: Some(id.into()),
            name: "Same model".into(),
            vendor: "NVIDIA".into(),
            utilization_percent: Some(utilization),
            memory_used_bytes: None,
            memory_total_bytes: None,
            temperature_celsius: None,
        };
        let samples = [
            Metrics {
                gpus: vec![gpu("first", 10.0), gpu("second", 90.0)],
                ..Default::default()
            },
            Metrics {
                gpus: vec![gpu("second", 50.0), gpu("first", 30.0)],
                ..Default::default()
            },
        ];
        let result = aggregate(&samples);
        assert_eq!(
            result
                .gpus
                .iter()
                .find(|g| g.id.as_deref() == Some("first"))
                .expect("first")
                .utilization_percent,
            Some(20.0)
        );
        assert_eq!(
            result
                .gpus
                .iter()
                .find(|g| g.id.as_deref() == Some("second"))
                .expect("second")
                .utilization_percent,
            Some(70.0)
        );
    }
    #[test]
    fn extended_series_average_per_key_and_monitors_need_every_sample_up() {
        use hope_protocol::{DiskIo, LoadAverage, MonitorResult, Temperature};
        let sample = |load, celsius, read: Option<f64>, up| Metrics {
            load_average: Some(LoadAverage {
                one: load,
                five: load,
                fifteen: load,
            }),
            temperatures: vec![Temperature {
                sensor: "coretemp:temp1".into(),
                label: "Package".into(),
                celsius,
            }],
            disk_io: vec![DiskIo {
                device: "nvme0n1".into(),
                read_bytes_per_second: read,
                write_bytes_per_second: None,
                busy_percent: None,
                root: true,
            }],
            monitors: vec![MonitorResult {
                target: "tcp://10.0.0.1:22".into(),
                kind: "tcp".into(),
                up,
                latency_ms: Some(10.0),
                status_code: None,
                detail: None,
            }],
            ..Default::default()
        };
        let result = aggregate(&[
            sample(1.0, 40.0, Some(100.0), true),
            sample(3.0, 60.0, None, false),
        ]);
        assert_eq!(result.load_average.map(|l| l.one), Some(2.0));
        assert_eq!(result.temperatures[0].celsius, 50.0);
        assert_eq!(result.disk_io[0].read_bytes_per_second, Some(100.0));
        assert_eq!(result.disk_io[0].write_bytes_per_second, None);
        assert!(!result.monitors[0].up);
        assert_eq!(
            aggregate(&[Metrics::default()]).load_average,
            None,
            "no load samples means no load average"
        );
    }
    #[test]
    fn mixed_resolutions_use_per_metric_counts_and_preserve_nulls() {
        let samples = [
            Metrics {
                cpu_percent: Some(10.0),
                ..Default::default()
            },
            Metrics {
                cpu_percent: Some(30.0),
                ..Default::default()
            },
            Metrics::default(),
        ];
        let mut stats = Statistics::default();
        for sample in &samples {
            stats.merge(&Statistics::sample(sample));
        }
        let points = vec![
            (
                MetricPoint {
                    collected_at: 300.0,
                    resolution_seconds: 300,
                    metrics: aggregate(&samples),
                },
                Some(stats),
            ),
            (
                MetricPoint {
                    collected_at: 610.0,
                    resolution_seconds: 30,
                    metrics: Metrics {
                        cpu_percent: Some(80.0),
                        ..Default::default()
                    },
                },
                Some(Statistics::sample(&Metrics {
                    cpu_percent: Some(80.0),
                    ..Default::default()
                })),
            ),
        ];
        let result = weighted_bucket(points, 900);
        assert_eq!(result[0].metrics.cpu_percent, Some(40.0));
        assert!(result[0].metrics.memory_used_bytes.is_none());
    }
}
