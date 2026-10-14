//! Consistent online backup of the SQLite database.
//!
//! `VACUUM INTO` reads one consistent snapshot through the normal WAL protocol, so it is safe
//! while the server is running and includes everything committed to the write-ahead log. Copying
//! `hope.db` alone would silently omit committed data still in `hope.db-wal`.
//!
//! The destination must be on a writable path: in the container the root filesystem is read-only,
//! so use the mounted data volume (`/var/lib/hope`) or another writable mount.
use anyhow::{Context, Result, bail};
use sqlx::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use std::{path::Path, time::Duration};

pub async fn run(database: &Path, destination: &Path) -> Result<()> {
    if !database.is_file() {
        bail!("database {} does not exist", database.display());
    }
    if destination.exists() {
        bail!(
            "{} already exists; choose a new file name",
            destination.display()
        );
    }
    // Reserve the destination privately first: backups contain credential and session hashes.
    {
        use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .with_context(|| format!("cannot create {}", destination.display()))?;
    }
    let result = copy(database, destination).await;
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

async fn copy(database: &Path, destination: &Path) -> Result<()> {
    let mut live = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(database)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10)),
    )
    .await
    .context("cannot open the live database")?;
    sqlx::query("VACUUM INTO ?")
        .bind(destination.to_str().context("backup path must be UTF-8")?)
        .execute(&mut live)
        .await
        .context("backup failed")?;
    live.close().await?;
    // The copy must open, pass SQLite's integrity check and be a standalone file.
    let mut copy = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(destination)
            .read_only(true),
    )
    .await
    .context("backup does not open")?;
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut copy)
        .await?;
    copy.close().await?;
    if check != "ok" {
        bail!("backup failed the integrity check");
    }
    // Make the copy durable: the file, then the directory entry that names it.
    std::fs::File::open(destination)?.sync_all()?;
    if let Some(parent) = destination.parent() {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
