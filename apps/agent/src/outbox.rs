//! Bounded, crash-safe delivery queue. Files are durable before they become visible.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: Uuid,
    pub created_at: u64,
    pub message: protocol::Message,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Loss {
    pub records: u64,
    pub first_at: Option<u64>,
    pub last_at: Option<u64>,
    pub reason: String,
}

pub struct Outbox {
    directory: PathBuf,
    records: VecDeque<(PathBuf, Record, u64)>,
    max_bytes: u64,
    max_age: u64,
    bytes: u64,
    pub loss: Loss,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_mode(path, bytes, 0o600)
}
pub fn atomic_write_mode(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

impl Outbox {
    pub fn open(directory: PathBuf, max_bytes: u64, max_age: u64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            max_bytes >= protocol::MAX_ENVELOPE_BYTES as u64,
            "outbox budget too small"
        );
        std::fs::create_dir_all(&directory)?;
        let loss = std::fs::read(directory.join("loss.json"))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        let mut queue = Self {
            directory,
            records: VecDeque::new(),
            max_bytes,
            max_age,
            bytes: 0,
            loss,
        };
        let mut paths = std::fs::read_dir(&queue.directory)?.collect::<Result<Vec<_>, _>>()?;
        paths.sort_by_key(|entry| entry.file_name());
        for entry in paths {
            let path = entry.path();
            if !entry.file_name().to_string_lossy().starts_with("record-")
                || path.extension().is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let length = entry.metadata()?.len();
            let record = if length <= protocol::MAX_ENVELOPE_BYTES as u64 {
                std::fs::read(&path)
                    .ok()
                    .and_then(|data| serde_json::from_slice::<Record>(&data).ok())
                    .filter(|record| record.message.validate().is_ok())
            } else {
                None
            };
            if let Some(record) = record {
                queue.bytes += length;
                queue.records.push_back((path, record, length));
            } else {
                queue.record_loss(now(), "corrupt queue record")?;
                // Remove corrupt bytes after recording the loss; never replay them.
                std::fs::remove_file(path)?;
            }
        }
        queue.trim(now(), 0)?;
        Ok(queue)
    }

    fn record_loss(&mut self, at: u64, reason: &str) -> anyhow::Result<()> {
        self.loss.records = self.loss.records.saturating_add(1);
        self.loss.first_at.get_or_insert(at);
        self.loss.last_at = Some(at);
        self.loss.reason = reason.into();
        atomic_write(
            &self.directory.join("loss.json"),
            &serde_json::to_vec(&self.loss)?,
        )
    }

    fn trim(&mut self, time: u64, incoming: u64) -> anyhow::Result<()> {
        while let Some((_, record, _)) = self.records.front() {
            let expired = time.saturating_sub(record.created_at) > self.max_age;
            if !expired && self.bytes.saturating_add(incoming) <= self.max_bytes {
                break;
            }
            let id = record.id;
            let at = record.created_at;
            self.record_loss(
                at,
                if expired {
                    "outbox age limit"
                } else {
                    "outbox byte limit"
                },
            )?;
            self.acknowledge(id)?;
        }
        Ok(())
    }

    pub fn enqueue(&mut self, message: protocol::Message) -> anyhow::Result<Uuid> {
        message.validate()?;
        let record = Record {
            id: Uuid::new_v4(),
            created_at: now(),
            message,
        };
        let bytes = serde_json::to_vec(&record)?;
        anyhow::ensure!(
            bytes.len() <= protocol::MAX_ENVELOPE_BYTES,
            "queue record too large"
        );
        self.trim(record.created_at, bytes.len() as u64)?;
        let path = self.directory.join(format!(
            "record-{:020}-{}.json",
            record.created_at, record.id
        ));
        if let Err(error) = atomic_write(&path, &bytes) {
            // Retain the counter in memory even if a full disk prevents persisting it.
            let _ = self.record_loss(record.created_at, "outbox write failed");
            return Err(error);
        }
        let _ = atomic_write(
            &self.directory.join("loss.json"),
            &serde_json::to_vec(&self.loss)?,
        );
        self.bytes += bytes.len() as u64;
        let id = record.id;
        self.records.push_back((path, record, bytes.len() as u64));
        Ok(id)
    }

    pub fn front(&mut self) -> anyhow::Result<Option<Record>> {
        self.trim(now(), 0)?;
        Ok(self.records.front().map(|(_, record, _)| record.clone()))
    }

    pub fn acknowledge(&mut self, id: Uuid) -> anyhow::Result<()> {
        if let Some(index) = self
            .records
            .iter()
            .position(|(_, record, _)| record.id == id)
        {
            let (path, _, bytes) = &self.records[index];
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            std::fs::File::open(&self.directory)?.sync_all()?;
            self.bytes -= bytes;
            self.records.remove(index);
        }
        Ok(())
    }

    pub fn reject(&mut self, id: Uuid) -> anyhow::Result<()> {
        if let Some((_, record, _)) = self.records.iter().find(|(_, record, _)| record.id == id) {
            self.record_loss(record.created_at, "server rejected record")?;
            self.acknowledge(id)?;
        }
        Ok(())
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({"queued_records": self.records.len(), "queued_bytes": self.bytes,
            "oldest_at": self.records.front().map(|(_, record, _)| record.created_at), "loss": self.loss})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message() -> protocol::Message {
        protocol::Message::Heartbeat(protocol::Heartbeat {
            agent_id: Uuid::new_v4(),
            uptime_secs: 1,
        })
    }
    #[test]
    fn restart_replays_stable_ids_until_acknowledged() {
        let path = std::env::temp_dir().join(format!("hope-outbox-{}", Uuid::new_v4()));
        let mut queue = Outbox::open(path.clone(), 1024 * 1024, 3600).unwrap();
        let id = queue.enqueue(message()).unwrap();
        drop(queue);
        let mut queue = Outbox::open(path.clone(), 1024 * 1024, 3600).unwrap();
        assert_eq!(queue.front().unwrap().unwrap().id, id);
        queue.acknowledge(Uuid::new_v4()).unwrap();
        assert!(queue.front().unwrap().is_some());
        queue.acknowledge(id).unwrap();
        drop(queue);
        assert!(
            Outbox::open(path.clone(), 1024 * 1024, 3600)
                .unwrap()
                .front()
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn corrupt_and_expired_records_do_not_block_valid_data() {
        let path = std::env::temp_dir().join(format!("hope-outbox-{}", Uuid::new_v4()));
        let mut queue = Outbox::open(path.clone(), 1024 * 1024, 3600).unwrap();
        queue.enqueue(message()).unwrap();
        queue.trim(now() + 3601, 0).unwrap();
        assert_eq!(queue.loss.records, 1);
        std::fs::write(path.join("record-000.json"), b"invalid").unwrap();
        queue.enqueue(message()).unwrap();
        drop(queue);
        let mut queue = Outbox::open(path.clone(), 1024 * 1024, 3600).unwrap();
        assert_eq!(queue.loss.records, 2);
        assert!(queue.front().unwrap().is_some());
        std::fs::remove_dir_all(path).unwrap();
    }
}
