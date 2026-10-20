use crate::{
    error::{ApiError, ApiResult},
    state::{State, now},
};
use axum::{
    Json,
    extract::{Path, Query, State as Extract},
};
use hope_protocol::{
    ChildMetrics, CpuMeasurement, MemoryMeasurement, MetricPoint, Metrics, SourceMetricPoint,
};
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
    fn observe_containers(&mut self, metrics: &Metrics, at: f64) {
        for container in &metrics.containers {
            self.0
                .entry(container_observed_key(&container.id))
                .or_insert(Statistic { sum: at, count: 1 });
        }
    }
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
        for container in &metrics.containers {
            let key = format!("container:{}", serde_json::json!(container.id));
            macro_rules! gauge {
                ($($field:ident),*) => { $(result.add(&format!("{key}:{}", stringify!($field)), container.$field);)* };
            }
            gauge!(
                cpu_percent,
                memory_used_bytes,
                memory_limit_bytes,
                memory_percent
            );
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
            if key.starts_with("container:") && key.ends_with(":observed_at") {
                entry.sum = entry.sum.max(value.sum);
                entry.count = 1;
                continue;
            }
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
    fn apply_container_gauges(&self, metrics: &mut Metrics) {
        for container in &mut metrics.containers {
            let key = format!("container:{}", serde_json::json!(container.id));
            macro_rules! gauge {
                ($($field:ident),*) => { $(container.$field = self.mean(&format!("{key}:{}", stringify!($field)));)* };
            }
            gauge!(
                cpu_percent,
                memory_used_bytes,
                memory_limit_bytes,
                memory_percent
            );
        }
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
        // Cumulative counters and state remain the latest observed metadata.
        self.apply_container_gauges(&mut latest);
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

#[derive(Clone, Default, Serialize, Deserialize)]
struct SourceMetricStatistics {
    values: BTreeMap<String, (f64, u64)>,
    cpu_measurement: Option<CpuMeasurement>,
    cpu_mixed: bool,
    memory_measurement: Option<MemoryMeasurement>,
    memory_mixed: bool,
    latest_at: f64,
    latest_rx_bytes: Option<f64>,
    latest_tx_bytes: Option<f64>,
    latest_disk_read_bytes: Option<f64>,
    latest_disk_write_bytes: Option<f64>,
    latest_uptime_seconds: Option<f64>,
}
impl SourceMetricStatistics {
    fn merge(&mut self, other: Self) {
        for (key, (sum, count)) in other.values {
            let entry = self.values.entry(key).or_default();
            entry.0 += sum;
            entry.1 += count;
        }
        self.cpu_mixed |= other.cpu_mixed
            || matches!((self.cpu_measurement, other.cpu_measurement), (Some(a), Some(b)) if a != b);
        self.memory_mixed |= other.memory_mixed
            || matches!((self.memory_measurement, other.memory_measurement), (Some(a), Some(b)) if a != b);
        self.cpu_measurement = self.cpu_measurement.or(other.cpu_measurement);
        self.memory_measurement = self.memory_measurement.or(other.memory_measurement);
        if other.latest_at > self.latest_at {
            self.latest_at = other.latest_at;
            self.latest_rx_bytes = other.latest_rx_bytes;
            self.latest_tx_bytes = other.latest_tx_bytes;
            self.latest_disk_read_bytes = other.latest_disk_read_bytes;
            self.latest_disk_write_bytes = other.latest_disk_write_bytes;
            self.latest_uptime_seconds = other.latest_uptime_seconds;
        }
    }

    fn observe(&mut self, metrics: &ChildMetrics, at: f64) {
        macro_rules! sample {
            ($field:ident) => {
                if let Some(value) = metrics.$field {
                    let entry = self.values.entry(stringify!($field).into()).or_default();
                    entry.0 += value;
                    entry.1 += 1;
                }
            };
        }
        sample!(cpu_percent);
        sample!(cpu_capacity_cores);
        sample!(memory_used_bytes);
        sample!(memory_total_bytes);
        sample!(disk_used_bytes);
        sample!(disk_total_bytes);
        sample!(disk_read_bytes_per_second);
        sample!(disk_write_bytes_per_second);
        sample!(network_rx_bytes_per_second);
        sample!(network_tx_bytes_per_second);
        if let Some(measurement) = metrics.cpu_measurement {
            if self
                .cpu_measurement
                .is_some_and(|previous| previous != measurement)
            {
                self.cpu_mixed = true;
            } else {
                self.cpu_measurement = Some(measurement);
            }
        }
        if let Some(measurement) = metrics.memory_measurement {
            if self
                .memory_measurement
                .is_some_and(|previous| previous != measurement)
            {
                self.memory_mixed = true;
            } else {
                self.memory_measurement = Some(measurement);
            }
        }
        if at >= self.latest_at {
            self.latest_at = at;
            self.latest_rx_bytes = metrics.network_rx_bytes;
            self.latest_tx_bytes = metrics.network_tx_bytes;
            self.latest_disk_read_bytes = metrics.disk_read_bytes;
            self.latest_disk_write_bytes = metrics.disk_write_bytes;
            self.latest_uptime_seconds = metrics.uptime_seconds;
        }
    }

    fn metrics(&self) -> ChildMetrics {
        let average = |field: &str| {
            self.values
                .get(field)
                .filter(|(_, count)| *count > 0)
                .map(|(sum, count)| sum / *count as f64)
        };
        ChildMetrics {
            cpu_percent: average("cpu_percent"),
            cpu_capacity_cores: average("cpu_capacity_cores"),
            cpu_measurement: (!self.cpu_mixed).then_some(self.cpu_measurement).flatten(),
            memory_used_bytes: average("memory_used_bytes"),
            memory_total_bytes: average("memory_total_bytes"),
            memory_measurement: (!self.memory_mixed)
                .then_some(self.memory_measurement)
                .flatten(),
            disk_used_bytes: average("disk_used_bytes"),
            disk_total_bytes: average("disk_total_bytes"),
            network_rx_bytes_per_second: average("network_rx_bytes_per_second"),
            network_tx_bytes_per_second: average("network_tx_bytes_per_second"),
            disk_read_bytes: self.latest_disk_read_bytes,
            disk_write_bytes: self.latest_disk_write_bytes,
            disk_read_bytes_per_second: average("disk_read_bytes_per_second"),
            disk_write_bytes_per_second: average("disk_write_bytes_per_second"),
            uptime_seconds: self.latest_uptime_seconds,
            network_rx_bytes: self.latest_rx_bytes,
            network_tx_bytes: self.latest_tx_bytes,
        }
    }
}

/// Consolidate duplicate identities only after the caller has established they describe the
/// same guest incarnation. Merge retained sample counts, not averages of unequal-size buckets.
pub(crate) async fn merge_source_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    survivor: &str,
    duplicate: &str,
) -> anyhow::Result<()> {
    if survivor == duplicate {
        anyhow::bail!("cannot consolidate source history into itself");
    }
    // Raw report IDs are batch IDs and retain provenance through child_batches. Legacy
    // aggregates without statistics stay separate so their unknown weights are not invented.
    sqlx::query("INSERT OR IGNORE INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) SELECT ?,CASE WHEN resolution=30 THEN report_id ELSE 'merged-' || ? || ':' || report_id END,observed_at,resolution,body FROM source_metrics m WHERE source_device_id=? AND (resolution=30 OR NOT EXISTS(SELECT 1 FROM source_metric_rollups r WHERE r.source_device_id=m.source_device_id AND r.bucket=CAST(m.observed_at/300 AS INTEGER)))")
        .bind(survivor).bind(duplicate).bind(duplicate).execute(&mut **tx).await?;
    let rows =
        sqlx::query("SELECT bucket,statistics FROM source_metric_rollups WHERE source_device_id=?")
            .bind(duplicate)
            .fetch_all(&mut **tx)
            .await?;
    for row in rows {
        let bucket: i64 = row.get("bucket");
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT statistics FROM source_metric_rollups WHERE source_device_id=? AND bucket=?",
        )
        .bind(survivor)
        .bind(bucket)
        .fetch_optional(&mut **tx)
        .await?;
        if previous.is_none() {
            // Unknown legacy weights cannot be combined with the new statistics.
            sqlx::query("UPDATE source_metrics SET report_id='legacy-' || report_id WHERE source_device_id=? AND report_id=? AND resolution=300")
                .bind(survivor).bind(format!("source-rollup-{bucket}"))
                .execute(&mut **tx).await?;
        }
        let mut statistics = previous
            .as_deref()
            .map(serde_json::from_str::<SourceMetricStatistics>)
            .transpose()?
            .unwrap_or_default();
        statistics.merge(serde_json::from_str(row.get("statistics"))?);
        sqlx::query("INSERT INTO source_metric_rollups(source_device_id,bucket,statistics) VALUES(?,?,?) ON CONFLICT(source_device_id,bucket) DO UPDATE SET statistics=excluded.statistics")
            .bind(survivor).bind(bucket).bind(serde_json::to_string(&statistics)?).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES(?,?,?,300,?) ON CONFLICT(source_device_id,report_id) DO UPDATE SET body=excluded.body")
            .bind(survivor).bind(format!("source-rollup-{bucket}")).bind(bucket as f64 * 300.0)
            .bind(serde_json::to_string(&statistics.metrics())?).execute(&mut **tx).await?;
    }
    sqlx::query("DELETE FROM source_metrics WHERE source_device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM source_metric_rollups WHERE source_device_id=?")
        .bind(duplicate)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
fn gpu_key(id: Option<&str>, vendor: &str, name: &str) -> String {
    id.map(|id| format!("gpu-id:{id}"))
        .unwrap_or_else(|| format!("gpu:{}", serde_json::json!([vendor, name])))
}
fn disk_key(name: &str, mount: &str, filesystem: &str) -> String {
    format!("disk:{}", serde_json::json!([name, mount, filesystem]))
}

fn container_observed_key(id: &str) -> String {
    format!("container:{}:observed_at", serde_json::json!(id))
}
/// Keep containers observed earlier in a bucket when its last sample omits
/// them. Prefer each container's newest metadata and bound churn to the list limit.
fn keep_observed_containers(
    latest: &mut Metrics,
    previous: &Metrics,
    latest_statistics: &Statistics,
    previous_statistics: &Statistics,
) {
    let previous_by_id: std::collections::HashMap<_, _> = previous
        .containers
        .iter()
        .map(|container| (container.id.as_str(), container))
        .collect();
    for container in &mut latest.containers {
        let key = container_observed_key(&container.id);
        if previous_statistics.mean(&key) > latest_statistics.mean(&key)
            && let Some(previous) = previous_by_id.get(container.id.as_str())
        {
            *container = (*previous).clone();
        }
    }
    let mut seen: std::collections::HashSet<_> =
        latest.containers.iter().map(|c| c.id.clone()).collect();
    for container in &previous.containers {
        if latest.containers.len() == 128 {
            break;
        }
        if seen.insert(container.id.clone()) {
            latest.containers.push(container.clone());
        }
    }
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
    let rows = sqlx::query("SELECT collected_at,resolution,body,statistics,latest_collected_at FROM metrics WHERE agent_id=? AND (collected_at>=? OR (resolution>30 AND collected_at+resolution>?)) ORDER BY collected_at DESC LIMIT 10000")
        .bind(agent).bind(since).bind(since).fetch_all(&state.0.db).await?;
    let mut points = vec![];
    for row in rows.into_iter().rev() {
        let mut point = MetricPoint {
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
        if point.resolution_seconds > 30 {
            // Pre-upgrade rollups copied the final container sample without
            // gauge sums/counts. They cannot truthfully be displayed as averages.
            statistics
                .as_ref()
                .unwrap_or(&Statistics::default())
                .apply_container_gauges(&mut point.metrics);
        }
        let latest_at = row.get::<Option<f64>, _>("latest_collected_at").unwrap_or(
            point.collected_at
                + if point.resolution_seconds > 30 {
                    point.resolution_seconds as f64
                } else {
                    0.0
                },
        );
        points.push((point, statistics, latest_at));
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
    if seconds == 0 || points.iter().any(|(_, statistics, _)| statistics.is_none()) {
        return Ok(Json(
            points.into_iter().map(|(point, _, _)| point).collect(),
        ));
    }
    Ok(Json(weighted_bucket(points, seconds)))
}

/// Return samples collected by the parent source, preserving its measurement semantics.
pub async fn source_history(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Query(window): Query<Window>,
) -> ApiResult<Json<Vec<SourceMetricPoint>>> {
    let host = state
        .hosts()
        .await?
        .into_iter()
        .find(|host| host.id == id || host.source_device_id.as_deref() == Some(id.as_str()))
        .or(state.host(&id).await?)
        .ok_or_else(ApiError::not_found)?;
    let mut source_ids = match host.source_device_id {
        Some(id) => vec![id],
        None => sqlx::query_scalar::<_, String>(
            "SELECT d.id FROM source_devices d WHERE d.host_id=? AND NOT EXISTS(SELECT 1 FROM source_device_aliases a WHERE a.alias_id=d.id) ORDER BY d.lifecycle='archived',d.observed_at DESC",
        ).bind(&host.id).fetch_all(&state.0.db).await?,
    };
    if source_ids.is_empty() {
        return Ok(Json(vec![]));
    }
    let aliases: Vec<String> = {
        let placeholders = vec!["?"; source_ids.len()].join(",");
        let query = format!(
            "SELECT alias_id FROM source_device_aliases WHERE survivor_id IN ({placeholders})"
        );
        let mut query = sqlx::query_scalar::<_, String>(&query);
        for id in &source_ids {
            query = query.bind(id);
        }
        query.fetch_all(&state.0.db).await?
    };
    source_ids.extend(aliases);
    let hours = window.hours.unwrap_or(24).clamp(1, 168);
    let since = now() - hours as f64 * 3600.0;
    let placeholders = vec!["?"; source_ids.len()].join(",");
    let query = format!(
        "SELECT source_device_id,observed_at,resolution,body FROM source_metrics WHERE source_device_id IN ({placeholders}) AND (observed_at>=? OR (resolution>30 AND observed_at+resolution>?)) ORDER BY observed_at DESC LIMIT 10000"
    );
    let mut query = sqlx::query(&query);
    for id in &source_ids {
        query = query.bind(id);
    }
    let rows = query.bind(since).bind(since).fetch_all(&state.0.db).await?;
    let mut points = rows
        .into_iter()
        .map(|row| {
            Ok(SourceMetricPoint {
                source_device_id: Some(row.get("source_device_id")),
                collected_at: row.get("observed_at"),
                resolution_seconds: row.get::<i64, _>("resolution") as u32,
                metrics: serde_json::from_str(row.get("body"))?,
            })
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    points.reverse();
    Ok(Json(points))
}
fn weighted_bucket(
    points: Vec<(MetricPoint, Option<Statistics>, f64)>,
    seconds: u32,
) -> Vec<MetricPoint> {
    let mut groups: BTreeMap<i64, (Metrics, Statistics, f64)> = BTreeMap::new();
    for (point, statistics, latest_at) in points {
        let entry = groups
            .entry((point.collected_at / seconds as f64).floor() as i64)
            .or_default();
        let mut latest = point.metrics;
        let mut statistics = statistics.unwrap_or_default();
        statistics.observe_containers(&latest, latest_at);
        if latest_at >= entry.2 {
            keep_observed_containers(&mut latest, &entry.0, &statistics, &entry.1);
            entry.0 = latest;
            entry.2 = latest_at;
        } else {
            keep_observed_containers(&mut entry.0, &latest, &entry.1, &statistics);
        }
        entry.1.merge(&statistics);
    }
    groups
        .into_iter()
        .map(|(time, (latest, statistics, _))| MetricPoint {
            collected_at: (time * seconds as i64) as f64,
            resolution_seconds: seconds,
            metrics: statistics.apply(latest),
        })
        .collect()
}
pub fn aggregate(values: &[Metrics]) -> Metrics {
    let mut statistics = Statistics::default();
    let mut latest = Metrics::default();
    for (index, value) in values.iter().enumerate() {
        let mut sample_statistics = Statistics::sample(value);
        sample_statistics.observe_containers(value, index as f64);
        let mut next = value.clone();
        keep_observed_containers(&mut next, &latest, &sample_statistics, &statistics);
        statistics.merge(&sample_statistics);
        latest = next;
    }
    statistics.apply(latest)
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
            sqlx::query("SELECT body,statistics,latest_collected_at FROM metrics WHERE agent_id=? AND report_id=?")
                .bind(&agent)
                .bind(format!("rollup-{bucket}"))
                .fetch_optional(&mut *tx)
                .await?;
        let mut statistics = Statistics::default();
        let mut latest = Metrics::default();
        let mut latest_at = f64::NEG_INFINITY;
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
            // Older rollups did not retain the metadata timestamp. Preserve
            // their known counters rather than letting a late sample rewind them.
            latest_at = existing
                .get::<Option<f64>, _>("latest_collected_at")
                .unwrap_or(from + 300.0);
            statistics.observe_containers(&latest, latest_at);
        }
        let rows = sqlx::query("SELECT body,collected_at FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>=? AND collected_at<? ORDER BY collected_at")
            .bind(&agent).bind(from).bind(from+300.0).fetch_all(&mut *tx).await?;
        for row in rows {
            let mut sample = serde_json::from_str(row.get("body"))?;
            let at: f64 = row.get("collected_at");
            let mut sample_statistics = Statistics::sample(&sample);
            sample_statistics.observe_containers(&sample, at);
            if at >= latest_at {
                keep_observed_containers(&mut sample, &latest, &sample_statistics, &statistics);
                latest = sample;
                latest_at = at;
            } else {
                keep_observed_containers(&mut latest, &sample, &statistics, &sample_statistics);
            }
            statistics.merge(&sample_statistics);
        }
        sqlx::query("INSERT OR REPLACE INTO metrics(agent_id,report_id,collected_at,resolution,body,statistics,latest_collected_at) VALUES(?,?,?,300,?,?,?)")
            .bind(&agent).bind(format!("rollup-{bucket}")).bind(from).bind(serde_json::to_string(&statistics.apply(latest))?).bind(serde_json::to_string(&statistics)?).bind(latest_at).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM metrics WHERE agent_id=? AND resolution=30 AND collected_at>=? AND collected_at<?")
            .bind(&agent).bind(from).bind(from+300.0).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    retain_source_metrics(state, cutoff, start).await?;
    let _guard = state.0.writes.lock().await;
    let mut tx = state.0.db.begin().await?;
    sqlx::query("DELETE FROM metrics WHERE rowid IN (SELECT rowid FROM metrics WHERE collected_at<? AND (resolution=30 OR collected_at+resolution<=?) LIMIT 5000)")
        .bind(reference - 7.0 * 86400.0)
        .bind(reference - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM source_metrics WHERE rowid IN (SELECT rowid FROM source_metrics WHERE observed_at<? AND (resolution=30 OR observed_at+resolution<=?) LIMIT 5000)")
        .bind(reference - 7.0 * 86400.0)
        .bind(reference - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM source_observations WHERE rowid IN (SELECT rowid FROM source_observations WHERE observed_at<? LIMIT 5000)")
        .bind(reference - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM child_batches WHERE rowid IN (SELECT rowid FROM child_batches WHERE observed_at<? LIMIT 5000)")
        .bind(reference - 7.0 * 86400.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM source_metric_rollups WHERE rowid IN (SELECT rowid FROM source_metric_rollups WHERE bucket<? LIMIT 5000)")
        .bind(((reference - 7.0 * 86400.0) / 300.0).floor() as i64)
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

async fn retain_source_metrics(
    state: &State,
    cutoff: f64,
    started: std::time::Instant,
) -> anyhow::Result<()> {
    let groups = sqlx::query("SELECT source_device_id,CAST(observed_at/300 AS INTEGER) AS bucket FROM source_metrics WHERE resolution=30 AND observed_at<? GROUP BY source_device_id,bucket LIMIT 128")
        .bind(cutoff).fetch_all(&state.0.db).await?;
    for group in groups {
        if started.elapsed() > std::time::Duration::from_secs(2) {
            break;
        }
        let _guard = state.0.writes.lock().await;
        let source_id: String = group.get("source_device_id");
        let bucket: i64 = group.get("bucket");
        let from = bucket as f64 * 300.0;
        let mut tx = state.0.db.begin().await?;
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT statistics FROM source_metric_rollups WHERE source_device_id=? AND bucket=?",
        )
        .bind(&source_id)
        .bind(bucket)
        .fetch_optional(&mut *tx)
        .await?;
        let mut statistics = existing
            .as_deref()
            .map(serde_json::from_str::<SourceMetricStatistics>)
            .transpose()?
            .unwrap_or_default();
        let rows = sqlx::query("SELECT body,observed_at FROM source_metrics WHERE source_device_id=? AND resolution=30 AND observed_at>=? AND observed_at<? ORDER BY observed_at")
            .bind(&source_id).bind(from).bind(from + 300.0).fetch_all(&mut *tx).await?;
        for row in rows {
            let metrics: ChildMetrics = serde_json::from_str(row.get("body"))?;
            statistics.observe(&metrics, row.get("observed_at"));
        }
        sqlx::query("INSERT OR REPLACE INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES(?,?,?,300,?)")
            .bind(&source_id)
            .bind(format!("source-rollup-{bucket}"))
            .bind(from)
            .bind(serde_json::to_string(&statistics.metrics())?)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT OR REPLACE INTO source_metric_rollups(source_device_id,bucket,statistics) VALUES(?,?,?)")
            .bind(&source_id)
            .bind(bucket)
            .bind(serde_json::to_string(&statistics)?)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM source_metrics WHERE source_device_id=? AND resolution=30 AND observed_at>=? AND observed_at<?")
            .bind(&source_id).bind(from).bind(from + 300.0).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_rollups_preserve_counter_resets_and_latest_uptime_with_delayed_samples() {
        let sample = |uptime, counter, rate| ChildMetrics {
            uptime_seconds: Some(uptime),
            disk_read_bytes: Some(counter),
            disk_write_bytes: Some(counter * 2.0),
            disk_read_bytes_per_second: rate,
            disk_write_bytes_per_second: rate.map(|value| value * 2.0),
            ..Default::default()
        };
        let mut statistics = SourceMetricStatistics::default();
        statistics.observe(&sample(10_000.0, 50_000.0, Some(100.0)), 100.0);
        statistics.observe(&sample(10.0, 10.0, None), 130.0);
        // The aggregate is persisted between retention passes. A late pre-reboot sample
        // must contribute its rates without resurrecting the old boot or larger counters.
        let mut statistics: SourceMetricStatistics =
            serde_json::from_str(&serde_json::to_string(&statistics).unwrap()).unwrap();
        statistics.observe(&sample(10_010.0, 51_000.0, Some(200.0)), 110.0);
        let metrics = statistics.metrics();
        assert_eq!(metrics.uptime_seconds, Some(10.0));
        assert_eq!(metrics.disk_read_bytes, Some(10.0));
        assert_eq!(metrics.disk_write_bytes, Some(20.0));
        assert_eq!(metrics.disk_read_bytes_per_second, Some(150.0));
        assert_eq!(metrics.disk_write_bytes_per_second, Some(300.0));
    }

    #[test]
    fn source_rollups_read_pre_disk_telemetry_statistics() {
        let statistics: SourceMetricStatistics = serde_json::from_value(serde_json::json!({
            "values": {}, "cpu_measurement": null, "cpu_mixed": false,
            "memory_measurement": null, "memory_mixed": false,
            "latest_at": 100.0, "latest_rx_bytes": 10.0, "latest_tx_bytes": 20.0
        }))
        .unwrap();
        assert_eq!(statistics.metrics().network_rx_bytes, Some(10.0));
        assert_eq!(statistics.metrics().disk_read_bytes, None);
        assert_eq!(statistics.metrics().uptime_seconds, None);
    }

    #[test]
    fn consolidated_source_rollups_weight_samples_and_preserve_latest_reset() {
        let sample = |cpu, uptime| ChildMetrics {
            cpu_percent: Some(cpu),
            cpu_measurement: Some(CpuMeasurement::HypervisorGuest),
            uptime_seconds: Some(uptime),
            disk_read_bytes: Some(uptime * 10.0),
            ..Default::default()
        };
        let mut first = SourceMetricStatistics::default();
        first.observe(&sample(10.0, 1000.0), 100.0);
        let mut second = SourceMetricStatistics::default();
        second.observe(&sample(30.0, 1010.0), 110.0);
        second.observe(&sample(50.0, 10.0), 130.0);
        first.merge(second);
        assert_eq!(first.metrics().cpu_percent, Some(30.0));
        assert_eq!(first.metrics().uptime_seconds, Some(10.0));
        assert_eq!(first.metrics().disk_read_bytes, Some(100.0));
    }

    #[test]
    fn uptime_preserves_the_latest_boot_instead_of_averaging_across_a_reboot() {
        let sample = |seconds, id: &str| Metrics {
            system_uptime: Some(hope_protocol::SystemUptime {
                seconds,
                boot_time: None,
                boot_id: Some(id.into()),
                scope: hope_protocol::UptimeScope::System,
            }),
            ..Default::default()
        };
        let before = sample(100_000.0, "old boot");
        let after = sample(10.0, "new boot");
        let aggregated = aggregate(&[before, after]);
        let uptime = aggregated.system_uptime.as_ref().expect("latest boot");
        assert_eq!(uptime.seconds, 10.0);
        assert_eq!(uptime.boot_id.as_deref(), Some("new boot"));
        assert!(
            aggregate(&[aggregated, Metrics::default()])
                .system_uptime
                .is_none()
        );
    }
    #[test]
    fn container_gauges_average_by_id_with_missing_samples_and_counter_totals_preserved() {
        let container = |id: &str, cpu, received| {
            serde_json::from_value(serde_json::json!({
                "id":id,"name":"same name","image":"fixture:latest",
                "state":"running","status":"Up","ports":"",
                "cpu_percent":cpu,"memory_used_bytes":cpu,
                "network_rx_bytes":received
            }))
            .unwrap()
        };
        let samples = [
            Metrics {
                containers: vec![
                    container("a", Some(20.0), 100.0),
                    container("b", Some(90.0), 5.0),
                ],
                ..Default::default()
            },
            Metrics {
                containers: vec![
                    container("b", None, 10.0),
                    container("a", Some(80.0), 400.0),
                ],
                ..Default::default()
            },
            Metrics::default(),
        ];
        let result = aggregate(&samples);
        let a = result.containers.iter().find(|c| c.id == "a").unwrap();
        let b = result.containers.iter().find(|c| c.id == "b").unwrap();
        assert_eq!(a.cpu_percent, Some(50.0));
        assert_eq!(a.memory_used_bytes, Some(50.0));
        assert_eq!(a.memory_percent, None);
        assert_eq!(a.network_rx_bytes, Some(400.0));
        assert_eq!(b.cpu_percent, Some(90.0));
        assert_eq!(b.network_rx_bytes, Some(10.0));

        let mut old_statistics = Statistics::default();
        for sample in &samples {
            old_statistics.merge(&Statistics::sample(sample));
        }
        let next = Metrics {
            containers: vec![container("a", Some(100.0), 500.0)],
            ..Default::default()
        };
        let combined = weighted_bucket(
            vec![
                (
                    MetricPoint {
                        collected_at: 300.0,
                        resolution_seconds: 300,
                        metrics: result,
                    },
                    Some(old_statistics),
                    540.0,
                ),
                (
                    MetricPoint {
                        collected_at: 610.0,
                        resolution_seconds: 30,
                        metrics: next.clone(),
                    },
                    Some(Statistics::sample(&next)),
                    610.0,
                ),
            ],
            900,
        );
        let a = combined[0]
            .metrics
            .containers
            .iter()
            .find(|c| c.id == "a")
            .unwrap();
        assert_eq!(a.cpu_percent, Some(200.0 / 3.0));
        assert_eq!(a.network_rx_bytes, Some(500.0));
        assert_eq!(combined[0].metrics.containers.len(), 2);
    }

    #[test]
    fn container_churn_keeps_the_newest_metadata_with_a_bounded_bucket() {
        let sample = |index| Metrics {
            containers: vec![
                serde_json::from_value(serde_json::json!({
                    "id":format!("container-{index}"),"name":"fixture","image":"fixture:latest",
                    "state":"running","status":"Up","ports":""
                }))
                .unwrap(),
            ],
            ..Default::default()
        };
        let result = aggregate(&(0..150).map(sample).collect::<Vec<_>>());
        assert_eq!(result.containers.len(), 128);
        assert_eq!(result.containers[0].id, "container-149");
        assert!(hope_protocol::validation::metrics(&result).is_ok());
    }

    #[tokio::test]
    async fn late_reports_do_not_rewind_retained_or_displayed_container_counters() {
        let directory = tempfile::tempdir().unwrap();
        let state = State::open(
            directory.path().join("history.db").to_str().unwrap(),
            "http://localhost:8080".into(),
            "releases".into(),
            hope_protocol::Capabilities {
                platform: "test".into(),
                discovery_mode: "limited".into(),
                arp: false,
                icmp: false,
                limitations: vec![],
            },
        )
        .await
        .unwrap();
        let reference = 1_700_000_200.0;
        let from = ((reference - 90_000.0) / 300.0_f64).floor() * 300.0;
        let sample = |cpu, received, status| {
            Metrics {containers:vec![serde_json::from_value(serde_json::json!({
            "id":"stable-id","name":"fixture","image":"fixture:latest","state":"running","status":status,"ports":"",
            "cpu_percent":cpu,"network_rx_bytes":received
        })).unwrap()], ..Default::default()}
        };
        let newer = sample(80.0, 500.0, "newer");
        sqlx::query(
            "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent','newer',?,?)",
        )
        .bind(from + 240.0)
        .bind(serde_json::to_string(&newer).unwrap())
        .execute(&state.0.db)
        .await
        .unwrap();
        retain_at(&state, reference).await.unwrap();
        let older = sample(20.0, 100.0, "older");
        sqlx::query(
            "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent','late',?,?)",
        )
        .bind(from + 30.0)
        .bind(serde_json::to_string(&older).unwrap())
        .execute(&state.0.db)
        .await
        .unwrap();
        let display = weighted_bucket(
            vec![
                (
                    MetricPoint {
                        collected_at: from,
                        resolution_seconds: 300,
                        metrics: newer.clone(),
                    },
                    Some(Statistics::sample(&newer)),
                    from + 240.0,
                ),
                (
                    MetricPoint {
                        collected_at: from + 30.0,
                        resolution_seconds: 30,
                        metrics: older.clone(),
                    },
                    Some(Statistics::sample(&older)),
                    from + 30.0,
                ),
            ],
            900,
        );
        assert_eq!(
            display[0].metrics.containers[0].network_rx_bytes,
            Some(500.0)
        );
        assert_eq!(display[0].metrics.containers[0].status, "newer");
        retain_at(&state, reference).await.unwrap();
        let (body, latest_at): (String, f64) = sqlx::query_as("SELECT body,latest_collected_at FROM metrics WHERE agent_id='agent' AND resolution=300")
            .fetch_one(&state.0.db).await.unwrap();
        let result: Metrics = serde_json::from_str(&body).unwrap();
        assert_eq!(result.containers[0].cpu_percent, Some(50.0));
        assert_eq!(result.containers[0].network_rx_bytes, Some(500.0));
        assert_eq!(result.containers[0].status, "newer");
        assert_eq!(latest_at, from + 240.0);

        // A late observation can be newer for an individual container even
        // when another container supplied the bucket's newest host report.
        let mut other = sample(10.0, 50.0, "other");
        other.containers[0].id = "other-id".into();
        sqlx::query(
            "INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent','other',?,?)",
        )
        .bind(from + 270.0)
        .bind(serde_json::to_string(&other).unwrap())
        .execute(&state.0.db)
        .await
        .unwrap();
        retain_at(&state, reference).await.unwrap();
        let late_newer = sample(60.0, 700.0, "newer for this container");
        sqlx::query("INSERT INTO metrics(agent_id,report_id,collected_at,body) VALUES('agent','late-newer',?,?)")
            .bind(from+250.0).bind(serde_json::to_string(&late_newer).unwrap()).execute(&state.0.db).await.unwrap();
        retain_at(&state, reference).await.unwrap();
        let body: String = sqlx::query_scalar(
            "SELECT body FROM metrics WHERE agent_id='agent' AND resolution=300",
        )
        .fetch_one(&state.0.db)
        .await
        .unwrap();
        let result: Metrics = serde_json::from_str(&body).unwrap();
        let container = result
            .containers
            .iter()
            .find(|container| container.id == "stable-id")
            .unwrap();
        assert_eq!(container.network_rx_bytes, Some(700.0));
        assert_eq!(container.status, "newer for this container");
    }

    #[tokio::test]
    async fn source_history_rolls_up_after_a_day_and_prunes_after_seven_days() {
        let directory = tempfile::tempdir().unwrap();
        let state = State::open(
            directory.path().join("source-history.db").to_str().unwrap(),
            "http://localhost:8080".into(),
            "releases".into(),
            hope_protocol::Capabilities {
                platform: "test".into(),
                discovery_mode: "limited".into(),
                arp: false,
                icmp: false,
                limitations: vec![],
            },
        )
        .await
        .unwrap();
        let reference = now();
        let cutoff = ((reference - 86400.0) / 300.0).floor() * 300.0;
        let from = cutoff - 300.0;
        macro_rules! insert_sample {
            ($report:expr, $at:expr, $cpu:expr, $counter:expr) => {
                sqlx::query("INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES('source','sample-' || ?,?,30,?)")
                    .bind($report)
                    .bind($at)
                    .bind(serde_json::to_string(&ChildMetrics {
                        cpu_percent: Some($cpu),
                        cpu_measurement: Some(CpuMeasurement::DockerEngine),
                        memory_used_bytes: Some($cpu * 10.0),
                        memory_measurement: Some(MemoryMeasurement::DockerWorkingSet),
                        network_rx_bytes: Some($counter),
                        ..Default::default()
                    }).unwrap())
                    .execute(&state.0.db)
                    .await
                    .unwrap();
            };
        }
        insert_sample!("a", from + 10.0, 10.0, 10.0);
        insert_sample!("b", from + 40.0, 30.0, 30.0);
        retain_at(&state, reference).await.unwrap();
        let first: (i64, String) = sqlx::query_as("SELECT resolution,body FROM source_metrics WHERE source_device_id='source' AND report_id=?")
            .bind(format!("source-rollup-{}", (from / 300.0) as i64))
            .fetch_one(&state.0.db)
            .await
            .unwrap();
        let first_metrics: ChildMetrics = serde_json::from_str(&first.1).unwrap();
        assert_eq!(first.0, 300);
        assert_eq!(first_metrics.cpu_percent, Some(20.0));
        assert_eq!(first_metrics.network_rx_bytes, Some(30.0));

        // A delayed report from the same old bucket is merged using the retained statistics.
        insert_sample!("c", from + 70.0, 50.0, 50.0);
        retain_at(&state, reference).await.unwrap();
        let body: String = sqlx::query_scalar(
            "SELECT body FROM source_metrics WHERE source_device_id='source' AND report_id=?",
        )
        .bind(format!("source-rollup-{}", (from / 300.0) as i64))
        .fetch_one(&state.0.db)
        .await
        .unwrap();
        let merged: ChildMetrics = serde_json::from_str(&body).unwrap();
        assert_eq!(merged.cpu_percent, Some(30.0));
        assert_eq!(merged.memory_used_bytes, Some(300.0));
        assert_eq!(merged.network_rx_bytes, Some(50.0));

        sqlx::query("INSERT INTO source_metrics(source_device_id,report_id,observed_at,resolution,body) VALUES('source','expired',?,300,'{}')")
            .bind(reference - 8.0 * 86400.0)
            .execute(&state.0.db)
            .await
            .unwrap();
        retain_at(&state, reference).await.unwrap();
        let expired: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM source_metrics WHERE report_id='expired'")
                .fetch_one(&state.0.db)
                .await
                .unwrap();
        assert_eq!(expired, 0);
    }

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
                540.0,
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
                610.0,
            ),
        ];
        let result = weighted_bucket(points, 900);
        assert_eq!(result[0].metrics.cpu_percent, Some(40.0));
        assert!(result[0].metrics.memory_used_bytes.is_none());
    }
}
