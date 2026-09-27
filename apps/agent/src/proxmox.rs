//! Local Proxmox inventory. The root oneshot exports only allowlisted fields;
//! the network-facing agent reads the resulting cache without elevated access.
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const CACHE: &str = "/run/hope-proxmox/inventory.json";
const MAX_BYTES: usize = 128 * 1024;
const MAX_GUESTS: usize = 128;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn detected() -> bool {
    cfg!(target_os = "linux")
        && Path::new("/usr/bin/pvesh").is_file()
        && Path::new("/etc/pve").is_dir()
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Proxmox inventory exceeds size limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn cached(value: Value, at: i64) -> anyhow::Result<Value> {
    let collected = value["collected_at_unix_secs"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("missing collection time"))?;
    anyhow::ensure!(
        (0..=180).contains(&(at - collected)),
        "Proxmox helper data is stale; check hope-agent-proxmox.timer"
    );
    anyhow::ensure!(
        value["guests"]
            .as_array()
            .is_some_and(|guests| guests.len() <= MAX_GUESTS),
        "invalid Proxmox guest list"
    );
    Ok(value)
}

pub fn collect() -> Option<Value> {
    if !detected() {
        return None;
    }
    Some(
        match read_json(Path::new(CACHE)).and_then(|value| cached(value, now())) {
            Ok(value) => value,
            Err(_) => {
                json!({"status":"unavailable", "complete":false, "error":"Proxmox inventory unavailable or stale. Rerun the current installer on this host, then check hope-agent-proxmox.timer and hope-agent-proxmox.service."})
            }
        },
    )
}

fn node_name(members: &Value) -> anyhow::Result<&str> {
    let node = members["nodename"].as_str().unwrap_or("");
    anyhow::ensure!(
        !node.is_empty()
            && node.len() <= 255
            && node
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_'),
        "invalid Proxmox node name"
    );
    Ok(node)
}

async fn list(node: &str, kind: &str) -> anyhow::Result<Value> {
    let mut command = Command::new("/usr/bin/pvesh");
    command
        .args([
            "get",
            &format!("/nodes/{node}/{kind}"),
            "--output-format",
            "json",
        ])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    timeout(Duration::from_secs(10), async {
        let mut child = command.spawn()?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(
            bytes.len() <= MAX_BYTES,
            "Proxmox response exceeds size limit"
        );
        anyhow::ensure!(child.wait().await?.success(), "Proxmox read failed");
        Ok::<_, anyhow::Error>(serde_json::from_slice(&bytes)?)
    })
    .await?
}

fn sanitize(value: &Value, kind: &str) -> anyhow::Result<(Vec<Value>, bool)> {
    let rows = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid Proxmox response"))?;
    let mut guests = Vec::new();
    let mut complete = rows.len() <= MAX_GUESTS;
    let mut ids = std::collections::HashSet::new();
    for row in rows.iter().take(MAX_GUESTS) {
        let Some(vmid) = row["vmid"]
            .as_u64()
            .filter(|id| (100..=999_999_999).contains(id))
        else {
            complete = false;
            continue;
        };
        if !ids.insert(vmid) {
            complete = false;
            continue;
        }
        let state = match row["status"].as_str() {
            Some("running") => "running",
            Some("stopped") => "stopped",
            Some("paused") => "paused",
            _ => "unknown",
        };
        guests.push(json!({"vmid":vmid, "kind":kind, "name":row["name"].as_str().map(|s| s.chars().take(128).collect::<String>()),
            "status":state, "template":row["template"].as_u64()==Some(1), "cpus":row["cpus"].as_u64(), "memory_bytes":row["maxmem"].as_u64(), "uptime_seconds":row["uptime"].as_u64()}));
    }
    guests.sort_by_key(|guest| guest["vmid"].as_u64());
    Ok((guests, complete))
}

pub async fn export() -> anyhow::Result<()> {
    anyhow::ensure!(detected(), "Proxmox VE is not installed on this host");
    let members = read_json(Path::new("/etc/pve/.members"))?;
    let node = node_name(&members)?;
    let mut guests = Vec::new();
    let mut errors = Vec::new();
    let mut complete = true;
    for (path, kind) in [("qemu", "vm"), ("lxc", "lxc")] {
        match list(node, path)
            .await
            .and_then(|value| sanitize(&value, kind))
        {
            Ok((rows, all)) => {
                guests.extend(rows);
                complete &= all;
            }
            Err(_) => {
                complete = false;
                errors.push(format!("Could not read local {kind} inventory"));
            }
        }
    }
    if guests.len() > MAX_GUESTS {
        guests.truncate(MAX_GUESTS);
        complete = false;
    }
    if !complete && errors.is_empty() {
        errors.push("Guest inventory is truncated or contains invalid records".into());
    }
    let payload = json!({"node":node, "status":if complete {"available"} else {"partial"}, "complete":complete,
        "collected_at_unix_secs":now(), "guests":guests, "error":if errors.is_empty() {None} else {Some(errors.join("; "))}});
    let bytes = serde_json::to_vec(&payload)?;
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Proxmox export exceeds size limit"
    );
    write_cache(Path::new(CACHE), &bytes)?;
    Ok(())
}

fn write_cache(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_only_allowlisted_fields_and_valid_ids() {
        let (rows, complete) = sanitize(&json!([
            {"vmid":101,"name":"guest","status":"running","cpus":2,"maxmem":1048576,"password":"secret","config":{"cipassword":"secret"}},
            {"vmid":"../../etc/shadow","status":"running"}]), "vm").unwrap();
        assert!(!complete);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "running");
        assert!(!serde_json::to_string(&rows).unwrap().contains("secret"));
    }
    #[test]
    fn bounds_records_and_rejects_unsafe_nodes_and_stale_cache() {
        let rows: Vec<_> = (100..300).map(|vmid| json!({"vmid":vmid})).collect();
        let (guests, complete) = sanitize(&json!(rows), "lxc").unwrap();
        assert_eq!(guests.len(), 128);
        assert!(!complete);
        assert_eq!(guests[0]["status"], "unknown");
        assert!(node_name(&json!({"nodename":"../../bad"})).is_err());
        assert!(node_name(&json!({"nodename":"pve-01"})).is_ok());
        let value = json!({"collected_at_unix_secs":1000,"guests":[]});
        assert!(cached(value.clone(), 1180).is_ok());
        assert!(cached(value.clone(), 1181).is_err());
        assert!(cached(value, 999).is_err());
    }
}
