//! Hand-off to the root-owned updater.
//!
//! The agent runs as an unprivileged account and cannot replace its own binary, which is what makes
//! an update safe to trigger from the network. When the server says an administrator wants a newer
//! release, the agent only leaves a request file in `update-queue/`; a service-manager path unit
//! started as root then runs the updater that the installer put in place, which checks the signed
//! release against the key pinned at installation. The updater writes its outcome to `update-result`
//! for the agent to report. Nothing in either file is trusted beyond being shown.
use hope_protocol::CollectorStatus;
use std::{
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;

const QUEUE: &str = "update-queue";
const REQUEST: &str = "request";
const RESULT: &str = "update-result";
/// What this agent last asked for and when, kept in its state directory. A failed update rolls back
/// by restarting the agent, so the memory of having asked must survive a restart; otherwise the
/// restarted agent asks again at once and a release that cannot report is retried in a loop.
const ASKED: &str = "update-asked";
/// A request that did not lead to an update is repeated no sooner than this.
const RETRY_AFTER: Duration = Duration::from_secs(3600);

pub const COLLECTOR: &str = "agent_update";

/// Leaves requests for the updater. It holds no state of its own (see `ASKED`).
pub struct Requests;

impl Requests {
    /// Leaves a request for the updater when the server asks for `version`, unless the same release
    /// was asked for within the last hour. Returns whether a file was written.
    pub async fn handle_now(&mut self, dir: &Path, version: &str) -> bool {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.handle(dir, version, seconds).await
    }

    /// As `handle_now`, at `now` seconds since the epoch.
    pub async fn handle(&mut self, dir: &Path, version: &str, now: u64) -> bool {
        if !plain_version(version) || !queue_ready(dir) || recently_asked(dir, version, now) {
            return false;
        }
        let path = dir.join(QUEUE).join(REQUEST);
        // A request already waiting is the updater's to pick up; do not queue a second.
        if path.symlink_metadata().is_ok() {
            return false;
        }
        // Creating the request can immediately stop this process. Persist the
        // rollback/restart cooldown first; never start an update if that fails.
        let asked = dir.join(ASKED);
        if crate::config::write_private(&asked, format!("{version} {now}\n").as_bytes())
            .await
            .is_err()
        {
            return false;
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path).await {
            Ok(mut request) => {
                // Only the file's existence wakes the updater; its contents
                // are deliberately ignored. It may be consumed before this
                // optional version annotation is written.
                let _ = request.write_all(format!("{version}\n").as_bytes()).await;
                let _ = request.flush().await;
                true
            }
            Err(_) => {
                // No update started. Do not hold off a later repaired queue.
                let _ = tokio::fs::remove_file(asked).await;
                false
            }
        }
    }
}

fn recently_asked(dir: &Path, version: &str, now: u64) -> bool {
    let path = dir.join(ASKED);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > 128 {
        return false;
    }
    let mut text = String::new();
    if std::fs::File::open(&path)
        .and_then(|file| file.take(128).read_to_string(&mut text))
        .is_err()
    {
        return false;
    }
    let Some((asked, at)) = text.trim().split_once(' ') else {
        return false;
    };
    let Ok(at) = at.parse::<u64>() else {
        return false;
    };
    // A time in the future (the clock was set back) does not hold requests off for long.
    asked == version
        && at <= now.saturating_add(RETRY_AFTER.as_secs())
        && now.saturating_sub(at) < RETRY_AFTER.as_secs()
}

fn plain_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// The queue directory exists as a real directory, which only a system installation creates.
fn queue_ready(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(QUEUE)).is_ok_and(|meta| meta.is_dir())
}

/// What to tell the server about updates on this host. `available` means an updater is installed
/// (so a request can lead somewhere), not that the last update worked; the detail says that.
pub fn status(dir: &Path) -> CollectorStatus {
    let (available, detail) = describe(dir);
    CollectorStatus {
        name: COLLECTOR.into(),
        available,
        detail: Some(detail),
    }
}

fn describe(dir: &Path) -> (bool, String) {
    if !queue_ready(dir) {
        return (
            false,
            "No updater is installed here, so updates cannot start from H.O.P.E. Run the upgrade command on this host once to enable them.".into(),
        );
    }
    let Some(result) = read_result(dir) else {
        return (true, "Updater ready".into());
    };
    let field = |name: &str| {
        result
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
    };
    match field("status") {
        Some("updated") => match field("version").filter(|v| plain_version(v)) {
            Some(version) => (true, format!("Updated to {version}")),
            None => (true, "Updated".into()),
        },
        Some("current") => match field("version").filter(|v| plain_version(v)) {
            Some(version) => (true, format!("Checked: already on {version}")),
            None => (true, "Checked: already current".into()),
        },
        Some("failed") => {
            let code = field("code").filter(|c| {
                !c.is_empty()
                    && c.len() <= 40
                    && c.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_')
            });
            (
                true,
                format!("The last update failed ({})", code.unwrap_or("UNKNOWN")),
            )
        }
        _ => (true, "Updater ready".into()),
    }
}

fn read_result(dir: &Path) -> Option<String> {
    let path = dir.join(RESULT);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 1024 {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("state directory");
        std::fs::create_dir(dir.path().join(QUEUE)).expect("queue");
        dir
    }

    #[tokio::test]
    async fn a_request_is_left_once_and_retried_hourly() {
        let dir = installed();
        let request = dir.path().join(QUEUE).join(REQUEST);
        let mut requests = Requests;
        let start = 1_000_000;
        assert!(requests.handle(dir.path(), "0.1.60", start).await);
        assert_eq!(
            std::fs::read_to_string(&request).expect("request"),
            "0.1.60\n"
        );
        // The updater consumes the file; a failed update is not retried on the next report.
        std::fs::remove_file(&request).expect("consumed");
        assert!(!requests.handle(dir.path(), "0.1.60", start + 10).await);
        assert!(!request.exists());
        assert!(
            requests
                .handle(dir.path(), "0.1.60", start + RETRY_AFTER.as_secs())
                .await
        );
        // A newer release is asked for at once.
        std::fs::remove_file(&request).expect("consumed");
        assert!(
            requests
                .handle(dir.path(), "0.1.61", start + RETRY_AFTER.as_secs())
                .await
        );
    }

    #[tokio::test]
    async fn a_restarted_agent_does_not_ask_again_for_a_release_that_just_failed() {
        // A failed update rolls back by restarting the agent; the new process has no memory of its own.
        let dir = installed();
        let request = dir.path().join(QUEUE).join(REQUEST);
        let start = 1_000_000;
        assert!(Requests.handle(dir.path(), "0.1.60", start).await);
        std::fs::remove_file(&request).expect("consumed");
        assert!(!Requests.handle(dir.path(), "0.1.60", start + 150).await);
        assert!(!request.exists());
        assert!(Requests.handle(dir.path(), "0.1.61", start + 150).await);
        std::fs::remove_file(&request).expect("consumed");
        assert!(
            Requests
                .handle(dir.path(), "0.1.61", start + 150 + RETRY_AFTER.as_secs())
                .await
        );
    }

    #[tokio::test]
    async fn a_future_or_damaged_memory_of_asking_does_not_hold_requests_off() {
        let dir = installed();
        let request = dir.path().join(QUEUE).join(REQUEST);
        let now = 1_000_000;
        for written in [
            "0.1.60 99999999999\n",
            "0.1.60\n",
            "0.1.60 soon\n",
            "garbage",
            "",
        ] {
            std::fs::write(dir.path().join(ASKED), written).expect("memory");
            assert!(
                Requests.handle(dir.path(), "0.1.60", now).await,
                "{written:?}"
            );
            std::fs::remove_file(&request).expect("consumed");
        }
        std::fs::write(dir.path().join(ASKED), "x".repeat(500)).expect("memory");
        assert!(Requests.handle(dir.path(), "0.1.60", now).await);
    }

    #[tokio::test]
    async fn a_waiting_request_is_not_duplicated() {
        let dir = installed();
        let mut requests = Requests;
        let start = 1_000_000;
        assert!(requests.handle(dir.path(), "0.1.60", start).await);
        std::fs::remove_file(dir.path().join(ASKED)).expect("forget");
        assert!(!Requests.handle(dir.path(), "0.1.60", start).await);
    }

    #[tokio::test]
    async fn cooldown_must_be_durable_before_a_request_can_wake_the_updater() {
        let dir = installed();
        let asked = dir.path().join(ASKED);
        // A failed cooldown write used to leave a live update request anyway,
        // allowing rollback to restart an agent with no memory of asking.
        std::fs::create_dir(&asked).expect("blocked cooldown path");
        assert!(!Requests.handle(dir.path(), "0.1.60", 1_000_000).await);
        assert!(!dir.path().join(QUEUE).join(REQUEST).exists());
        std::fs::remove_dir(&asked).expect("repair state");
        assert!(Requests.handle(dir.path(), "0.1.60", 1_000_000).await);
        assert_eq!(
            std::fs::read_to_string(&asked).expect("cooldown"),
            "0.1.60 1000000\n"
        );
        // No staged file is left in a directory launchd watches for contents.
        assert_eq!(
            std::fs::read_dir(dir.path().join(QUEUE))
                .expect("queue")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn nothing_is_written_without_an_updater_or_for_a_strange_version() {
        let bare = tempfile::tempdir().expect("state directory");
        let mut requests = Requests;
        assert!(!requests.handle(bare.path(), "0.1.60", 1_000_000).await);
        assert!(!bare.path().join(QUEUE).exists());
        let dir = installed();
        for version in ["", "../../x", "1.0; rm -rf /", "a b", &"9".repeat(65)] {
            assert!(
                !requests.handle(dir.path(), version, 1_000_000).await,
                "{version:?}"
            );
        }
        assert_eq!(
            std::fs::read_dir(dir.path().join(QUEUE))
                .expect("queue")
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_linked_queue_is_not_written_through() {
        let dir = tempfile::tempdir().expect("state directory");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(QUEUE)).expect("link");
        assert!(!Requests.handle(dir.path(), "0.1.60", 1_000_000).await);
        assert_eq!(std::fs::read_dir(elsewhere.path()).expect("dir").count(), 0);
    }

    #[test]
    fn status_says_what_the_updater_last_did() {
        let bare = tempfile::tempdir().expect("state directory");
        let missing = status(bare.path());
        assert!(!missing.available);
        assert!(missing.detail.expect("detail").contains("upgrade command"));

        let dir = installed();
        assert_eq!(status(dir.path()).detail.as_deref(), Some("Updater ready"));
        for (written, available, detail) in [
            (
                "status=updated\nversion=0.1.60\nat=1",
                true,
                "Updated to 0.1.60",
            ),
            (
                "status=current\nversion=0.1.60\nat=1",
                true,
                "Checked: already on 0.1.60",
            ),
            (
                "status=failed\ncode=MANIFEST_INVALID\nat=1",
                true,
                "The last update failed (MANIFEST_INVALID)",
            ),
            (
                "status=failed\ncode=bad code!\nat=1",
                true,
                "The last update failed (UNKNOWN)",
            ),
            ("status=updated\nversion=<script>\n", true, "Updated"),
            ("status=unknown", true, "Updater ready"),
            ("garbage", true, "Updater ready"),
        ] {
            std::fs::write(dir.path().join(RESULT), written).expect("result");
            let got = status(dir.path());
            assert_eq!(
                (got.available, got.detail.as_deref()),
                (available, Some(detail)),
                "{written:?}"
            );
            assert_eq!(got.name, COLLECTOR);
        }
    }

    #[test]
    fn an_oversized_or_linked_result_is_ignored() {
        let dir = installed();
        std::fs::write(
            dir.path().join(RESULT),
            "status=failed\ncode=X\n".repeat(200),
        )
        .expect("big");
        assert_eq!(status(dir.path()).detail.as_deref(), Some("Updater ready"));
        std::fs::remove_file(dir.path().join(RESULT)).expect("remove");
        #[cfg(unix)]
        {
            let target = dir.path().join("elsewhere");
            std::fs::write(&target, "status=failed\ncode=X\n").expect("target");
            std::os::unix::fs::symlink(&target, dir.path().join(RESULT)).expect("link");
            assert_eq!(status(dir.path()).detail.as_deref(), Some("Updater ready"));
        }
    }
}
