//! Opt-in bounded journal and Docker collection with durable source checkpoints.
use crate::outbox::{Outbox, atomic_write, now};
use protocol::{CollectionConfig, LogBatch, LogEntry, Message};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
use uuid::Uuid;

const MAX_CAPTURE: usize = 64 * 1024;
#[derive(Default, Clone, Serialize, Deserialize)]
struct Cursor {
    journal: Option<String>,
    since: String,
    #[serde(default)]
    counts: BTreeMap<String, usize>,
    #[serde(default)]
    budget: SourceBudget,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct SourceBudget {
    started_at: u64,
    events: u32,
    bytes: u32,
    #[serde(default)]
    dropped_records: u64,
    #[serde(default)]
    last_dropped_at: u64,
}
impl SourceBudget {
    fn admit(&mut self, policy: &protocol::LogSourcePolicy, bytes: u32, at: u64) -> bool {
        if at >= self.started_at.saturating_add(60) || at < self.started_at {
            *self = Self {
                started_at: at,
                dropped_records: self.dropped_records,
                last_dropped_at: self.last_dropped_at,
                ..Default::default()
            };
        }
        if self.events >= policy.max_events_per_minute
            || self.bytes.saturating_add(bytes) > policy.max_bytes_per_minute
        {
            self.dropped_records = self.dropped_records.saturating_add(1);
            self.last_dropped_at = at;
            return false;
        }
        self.events += 1;
        self.bytes += bytes;
        true
    }
}
fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}
fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}
fn bounded(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
pub fn redact(text: &str, rules: &[String]) -> String {
    let mut text = text.to_string();
    for rule in rules {
        text = text.replace(rule, "[REDACTED]");
    }
    bounded(&text, 2048)
}

pub fn diagnostic(queue: &Arc<Mutex<Outbox>>, agent_id: Uuid, message: &str, severity: &str) {
    let entry = LogEntry {
        event_id: Uuid::new_v4().to_string(),
        observed_at_unix_ms: (now() * 1000) as i64,
        source: "agent".into(),
        severity: severity.into(),
        message: bounded(message, 2048),
        attributes: json!({}),
    };
    if let Err(error) = queue.lock().unwrap().enqueue(Message::LogBatch(LogBatch {
        batch_id: Uuid::new_v4(),
        agent_id,
        entries: vec![entry],
    })) {
        tracing::warn!(%error, "could not persist diagnostic event");
    }
}

pub async fn collect(
    state_dir: String,
    agent_id: Uuid,
    queue: Arc<Mutex<Outbox>>,
    config: Arc<Mutex<CollectionConfig>>,
) {
    let mut ticks = tokio::time::interval(Duration::from_secs(5));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failures = BTreeMap::<String, String>::new();
    let mut health = BTreeMap::<String, u64>::new();
    let mut budgets = BTreeMap::<String, SourceBudget>::new();
    loop {
        ticks.tick().await;
        let settings = config.lock().unwrap().clone();
        for (kind, names) in [
            ("journal", &settings.journal_units),
            ("docker", &settings.docker_containers),
        ] {
            for name in names {
                let source = format!("{kind}:{name}");
                match collect_source(
                    Path::new(&state_dir),
                    agent_id,
                    kind,
                    name,
                    &settings,
                    &queue,
                )
                .await
                {
                    Ok(budget) => {
                        budgets.insert(source.clone(), budget);
                        health.insert(source.clone(), now());
                        if failures.remove(&source).is_some() {
                            diagnostic(
                                &queue,
                                agent_id,
                                &format!("{source}: collection recovered"),
                                "notice",
                            );
                        }
                    }
                    Err(error) => {
                        let message = format!("{source}: {error}");
                        if failures.get(&source) != Some(&message) {
                            diagnostic(
                                &queue,
                                agent_id,
                                &redact(&message, &settings.redact),
                                "warning",
                            );
                            failures.insert(source, message);
                        }
                    }
                }
            }
        }
        health.retain(|source, _| {
            settings
                .journal_units
                .iter()
                .any(|name| source == &format!("journal:{name}"))
                || settings
                    .docker_containers
                    .iter()
                    .any(|name| source == &format!("docker:{name}"))
        });
        budgets.retain(|source, _| health.contains_key(source));
        if let Err(error) = atomic_write(
            &Path::new(&state_dir).join("log-budgets.json"),
            &serde_json::to_vec(&budgets).unwrap(),
        ) {
            tracing::warn!(%error, "cannot persist log budget status");
        }
        if let Err(error) = atomic_write(
            &Path::new(&state_dir).join("log-health.json"),
            &serde_json::to_vec(&health).unwrap(),
        ) {
            tracing::warn!(%error, "cannot persist log collector health");
        }
    }
}

async fn capture(program: &str, args: &[String]) -> anyhow::Result<(Vec<u8>, Vec<u8>, bool, bool)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap().take((MAX_CAPTURE + 1) as u64);
    let mut stderr = child.stderr.take().unwrap().take((MAX_CAPTURE + 1) as u64);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))
    })
    .await;
    let truncated = out.len() > MAX_CAPTURE || err.len() > MAX_CAPTURE || result.is_err();
    if truncated {
        let _ = child.kill().await;
    }
    let status = child.wait().await?;
    if let Ok(result) = result {
        result?;
    }
    Ok((out, err, truncated, status.success()))
}

async fn collect_source(
    root: &Path,
    agent_id: Uuid,
    kind: &str,
    name: &str,
    settings: &CollectionConfig,
    queue: &Arc<Mutex<Outbox>>,
) -> anyhow::Result<SourceBudget> {
    let source = format!("{kind}:{name}");
    let rules = &settings.redact;
    let policy = settings
        .source_policies
        .get(&source)
        .cloned()
        .unwrap_or_default();
    let checkpoint: PathBuf = root
        .join("log-cursors")
        .join(format!("{}.json", hash(&source)));
    let mut cursor: Cursor = match std::fs::read(&checkpoint) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Cursor {
            since: timestamp(),
            ..Default::default()
        },
        Err(error) => return Err(error.into()),
    };
    // Save the initial boundary even when this source currently has no entries.
    atomic_write(&checkpoint, &serde_json::to_vec(&cursor)?)?;
    let args: Vec<String> = if kind == "journal" {
        let mut args = vec![
            "--output=json".into(),
            "--no-pager".into(),
            "--quiet".into(),
            "--unit".into(),
            name.into(),
        ];
        if let Some(position) = &cursor.journal {
            args.push(format!("--after-cursor={position}"));
        } else {
            args.push(format!("--since={}", cursor.since));
        }
        args
    } else {
        vec![
            "logs".into(),
            "--timestamps".into(),
            "--since".into(),
            cursor.since.clone(),
            name.into(),
        ]
    };
    let (stdout, stderr, truncated, success) = capture(
        if kind == "journal" {
            "journalctl"
        } else {
            "docker"
        },
        &args,
    )
    .await?;
    if !success && !truncated {
        let error = String::from_utf8_lossy(&stderr);
        if kind == "journal" && cursor.journal.is_some() && error.to_lowercase().contains("cursor")
        {
            diagnostic(
                queue,
                agent_id,
                &format!(
                    "{source}: journal cursor expired; collection gap, restarting from current time"
                ),
                "warning",
            );
            cursor = Cursor {
                since: timestamp(),
                budget: cursor.budget.clone(),
                ..Default::default()
            };
            atomic_write(&checkpoint, &serde_json::to_vec(&cursor)?)?;
        }
        anyhow::bail!("source unavailable: {}", redact(&error, rules));
    }
    // A shared Docker checkpoint must not skip the shorter captured stream.
    let boundary = if kind == "docker" && truncated {
        [&stdout, &stderr]
            .into_iter()
            .filter(|bytes| !bytes.is_empty())
            .filter_map(|bytes| {
                bytes
                    .split_inclusive(|byte| *byte == b'\n')
                    .filter(|line| line.ends_with(b"\n"))
                    .filter_map(|line| {
                        std::str::from_utf8(line)
                            .ok()?
                            .split_once(' ')
                            .map(|(at, _)| at.to_owned())
                    })
                    .next_back()
            })
            .min()
    } else {
        None
    };
    let mut entries = Vec::new();
    let mut seen = BTreeMap::<String, usize>::new();
    let mut latest_counts = cursor.counts.clone();
    let original_since = cursor.since.clone();
    let mut changed = false;
    let mut dropped = Vec::new();
    let mut dropped_bytes = 0u64;
    for (stream, bytes) in [("stdout", &stdout), ("stderr", &stderr)] {
        if kind == "journal" && stream == "stderr" {
            continue;
        }
        // Incomplete trailing lines are replayed from the last complete cursor.
        for line in bytes
            .split_inclusive(|byte| *byte == b'\n')
            .filter(|line| line.ends_with(b"\n"))
        {
            let line = String::from_utf8_lossy(line)
                .trim_end_matches('\n')
                .to_string();
            let entry = if kind == "journal" {
                let value: serde_json::Value = serde_json::from_str(&line)?;
                let position = value["__CURSOR"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("journal entry missing cursor"))?;
                let micros = value["__REALTIME_TIMESTAMP"]
                    .as_str()
                    .and_then(|text| text.parse::<i64>().ok())
                    .unwrap_or((now() * 1_000_000) as i64);
                let severity = match value["PRIORITY"].as_str() {
                    Some("0" | "1" | "2") => "critical",
                    Some("3") => "error",
                    Some("4") => "warning",
                    Some("5") => "notice",
                    Some("6") => "info",
                    Some("7") => "debug",
                    _ => "unknown",
                };
                cursor.journal = Some(position.into());
                LogEntry {
                    event_id: hash(&format!("{source}:{position}")),
                    observed_at_unix_ms: micros / 1000,
                    source: source.clone(),
                    severity: severity.into(),
                    message: redact(
                        value["MESSAGE"]
                            .as_str()
                            .unwrap_or("[non-text journal message]"),
                        rules,
                    ),
                    attributes: json!({"unit": name, "boot_id": value["_BOOT_ID"], "truncated": value["MESSAGE"].as_str().is_some_and(|text| text.len() > 2048)}),
                }
            } else {
                let Some((at, message)) = line.split_once(' ') else {
                    continue;
                };
                if boundary
                    .as_ref()
                    .is_some_and(|boundary| at > boundary.as_str())
                {
                    continue;
                }
                let Ok(time) =
                    time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
                else {
                    continue;
                };
                let key = hash(&format!("{stream}:{at}:{message}"));
                let count = seen.entry(key.clone()).or_default();
                *count += 1;
                if at == original_since && *count <= cursor.counts.get(&key).copied().unwrap_or(0) {
                    continue;
                }
                if at > cursor.since.as_str() {
                    cursor.since = at.into();
                    latest_counts.clear();
                }
                if at == cursor.since {
                    latest_counts.insert(key.clone(), *count);
                }
                LogEntry {
                    event_id: hash(&format!("{source}:{key}:{count}")),
                    observed_at_unix_ms: (time.unix_timestamp_nanos() / 1_000_000) as i64,
                    source: source.clone(),
                    severity: "unknown".into(),
                    message: redact(message, rules),
                    attributes: json!({"container": name, "stream": stream, "truncated": message.len() > 2048}),
                }
            };
            changed = true;
            if !policy.includes(&entry.severity) {
                continue;
            }
            let bytes = serde_json::to_vec(&entry)?.len() as u32;
            if cursor.budget.admit(&policy, bytes, now()) {
                entries.push(entry);
            } else {
                dropped_bytes += u64::from(bytes);
                dropped.push(entry.event_id);
            }
        }
    }
    if !dropped.is_empty() {
        entries.push(LogEntry {
            event_id: hash(&format!("{source}:budget:{}:{}", dropped[0], dropped.last().unwrap())),
            observed_at_unix_ms: (now() * 1000) as i64,
            source: "agent".into(), severity: "warning".into(),
            message: format!("{source}: source budget exceeded; {} log records intentionally dropped ({dropped_bytes} bytes)", dropped.len()),
            attributes: json!({"source":source,"reason":"source_budget","dropped_records":dropped.len(),"dropped_bytes":dropped_bytes}),
        });
    }
    // Each batch is committed before moving the source checkpoint. Replays use event IDs.
    for chunk in entries.chunks(8) {
        queue.lock().unwrap().enqueue(Message::LogBatch(LogBatch {
            batch_id: Uuid::new_v4(),
            agent_id,
            entries: chunk.to_vec(),
        }))?;
    }
    cursor.counts = latest_counts;
    if (truncated && !changed) || cursor.counts.len() > 1024 {
        diagnostic(
            queue,
            agent_id,
            &format!(
                "{source}: oversized record or replay boundary exhausted; collection gap, restarting from current time"
            ),
            "warning",
        );
        cursor = Cursor {
            since: timestamp(),
            budget: cursor.budget.clone(),
            ..Default::default()
        };
    }
    atomic_write(&checkpoint, &serde_json::to_vec(&cursor)?)?;
    Ok(cursor.budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_budget_survives_restart_and_resets_only_after_window() {
        let policy = protocol::LogSourcePolicy {
            max_events_per_minute: 2,
            max_bytes_per_minute: 1024,
            ..Default::default()
        };
        let mut budget = SourceBudget::default();
        assert!(budget.admit(&policy, 512, 100));
        let mut resumed: SourceBudget =
            serde_json::from_slice(&serde_json::to_vec(&budget).unwrap()).unwrap();
        assert!(!resumed.admit(&policy, 513, 101));
        assert!(resumed.admit(&policy, 512, 102));
        assert!(!resumed.admit(&policy, 1, 159));
        assert!(resumed.admit(&policy, 1024, 160));
        let mut other = SourceBudget::default();
        assert!(other.admit(&policy, 1024, 159));
    }
    #[test]
    fn redaction_happens_before_utf8_safe_truncation() {
        let result = redact(
            &format!("token-secret{}", "é".repeat(3000)),
            &["token-secret".into()],
        );
        assert!(result.starts_with("[REDACTED]"));
        assert!(!result.contains("token-secret"));
        assert!(result.len() <= 2048);
    }
}
