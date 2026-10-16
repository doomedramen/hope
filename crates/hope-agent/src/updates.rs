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
    path::Path,
    time::{Duration, Instant},
};

const QUEUE: &str = "update-queue";
const REQUEST: &str = "request";
const RESULT: &str = "update-result";
/// A request that did not lead to an update is repeated no sooner than this.
const RETRY_AFTER: Duration = Duration::from_secs(3600);

pub const COLLECTOR: &str = "agent_update";

/// Remembers what was last asked of the updater, so a failing update is retried hourly, not on
/// every report.
#[derive(Default)]
pub struct Requests {
    last: Option<(String, Instant)>,
}

impl Requests {
    /// Leaves a request for the updater when the server asks for `version`. Returns whether a file
    /// was written.
    pub fn handle(&mut self, dir: &Path, version: &str, now: Instant) -> bool {
        if !plain_version(version) || !queue_ready(dir) {
            return false;
        }
        if let Some((asked, at)) = &self.last
            && asked == version
            && now.duration_since(*at) < RETRY_AFTER
        {
            return false;
        }
        let path = dir.join(QUEUE).join(REQUEST);
        // A request already waiting is the updater's to pick up; do not queue a second.
        if path.symlink_metadata().is_ok() {
            return false;
        }
        let written = std::fs::write(&path, format!("{version}\n")).is_ok();
        if written {
            self.last = Some((version.to_string(), now));
        }
        written
    }
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
    use std::io::Read;
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

    #[test]
    fn a_request_is_left_once_and_retried_hourly() {
        let dir = installed();
        let request = dir.path().join(QUEUE).join(REQUEST);
        let mut requests = Requests::default();
        let start = Instant::now();
        assert!(requests.handle(dir.path(), "0.1.60", start));
        assert_eq!(
            std::fs::read_to_string(&request).expect("request"),
            "0.1.60\n"
        );
        // The updater consumes the file; a failed update is not retried on the next report.
        std::fs::remove_file(&request).expect("consumed");
        assert!(!requests.handle(dir.path(), "0.1.60", start + Duration::from_secs(10)));
        assert!(!request.exists());
        assert!(requests.handle(dir.path(), "0.1.60", start + RETRY_AFTER));
        // A newer release is asked for at once.
        std::fs::remove_file(&request).expect("consumed");
        assert!(requests.handle(dir.path(), "0.1.61", start + RETRY_AFTER));
    }

    #[test]
    fn a_waiting_request_is_not_duplicated() {
        let dir = installed();
        let mut requests = Requests::default();
        let start = Instant::now();
        assert!(requests.handle(dir.path(), "0.1.60", start));
        let mut later = Requests::default();
        assert!(!later.handle(dir.path(), "0.1.60", start));
    }

    #[test]
    fn nothing_is_written_without_an_updater_or_for_a_strange_version() {
        let bare = tempfile::tempdir().expect("state directory");
        let mut requests = Requests::default();
        assert!(!requests.handle(bare.path(), "0.1.60", Instant::now()));
        assert!(!bare.path().join(QUEUE).exists());
        let dir = installed();
        for version in ["", "../../x", "1.0; rm -rf /", "a b", &"9".repeat(65)] {
            assert!(
                !requests.handle(dir.path(), version, Instant::now()),
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
    #[test]
    fn a_linked_queue_is_not_written_through() {
        let dir = tempfile::tempdir().expect("state directory");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(QUEUE)).expect("link");
        assert!(!Requests::default().handle(dir.path(), "0.1.60", Instant::now()));
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
