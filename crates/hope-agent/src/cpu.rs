//! Machine-boundary CPU counters. A host uses sysinfo; container probes use PID 1's hierarchy.
#[cfg(any(target_os = "linux", test))]
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy)]
struct CgroupCpuSample {
    usage_usec: u64,
    cores: f64,
}

#[derive(Default)]
pub(crate) struct CgroupCpuProbe {
    previous: Option<(CgroupCpuSample, Instant)>,
}

impl CgroupCpuProbe {
    pub(crate) fn sample(&mut self, fallback_cores: usize) -> Option<f64> {
        self.record(read_cgroup_cpu_sample(fallback_cores), Instant::now())
    }

    fn record(&mut self, current: Option<CgroupCpuSample>, at: Instant) -> Option<f64> {
        let Some(current) = current else {
            self.previous = None;
            return None;
        };
        let (previous, previous_at) = self.previous.replace((current, at))?;
        if previous.cores != current.cores {
            return None;
        }
        cpu_percent_from_samples(
            previous,
            current,
            at.duration_since(previous_at).as_secs_f64(),
        )
    }
}

/// A systemd guest's init process lives in init.scope, directly below its machine boundary.
#[cfg(any(target_os = "linux", test))]
fn guest_group(init_group: &str) -> &str {
    init_group
        .strip_suffix("/init.scope")
        .map_or(
            init_group,
            |parent| {
                if parent.is_empty() { "/" } else { parent }
            },
        )
}

#[cfg(target_os = "linux")]
fn read_cgroup_cpu_sample(fallback_cores: usize) -> Option<CgroupCpuSample> {
    // PID 1 defines the guest boundary. The agent's own group is usually just its service.
    let membership = std::fs::read_to_string("/proc/1/cgroup").ok()?;
    for line in membership.lines() {
        let mut fields = line.splitn(3, ':');
        let _hierarchy = fields.next()?;
        let controllers = fields.next()?;
        let group = guest_group(fields.next()?);
        if controllers.is_empty() {
            if let Some(sample) = read_v2_cpu_sample(group, fallback_cores) {
                return Some(sample);
            }
        } else if controllers.split(',').any(|name| name == "cpuacct")
            && let Some(sample) = read_v1_cpu_sample(group, fallback_cores)
        {
            return Some(sample);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_cgroup_cpu_sample(_fallback_cores: usize) -> Option<CgroupCpuSample> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn safe_cgroup_path(root: &Path, group: &str) -> Option<PathBuf> {
    let relative = Path::new(group).strip_prefix("/").ok()?;
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(relative))
}

#[cfg(any(target_os = "linux", test))]
fn cgroup_ancestors(path: &Path) -> Vec<&Path> {
    let mut paths = Vec::new();
    let mut current = Some(path);
    while let Some(directory) = current {
        paths.push(directory);
        current = directory.parent();
    }
    paths
}

#[cfg(any(target_os = "linux", test))]
fn parse_cpu_list(value: &str) -> Option<usize> {
    let mut count = 0usize;
    for item in value.trim().split(',').filter(|item| !item.is_empty()) {
        let (first, last) = item
            .split_once('-')
            .map_or((item, item), |(first, last)| (first, last));
        let first = first.parse::<usize>().ok()?;
        let last = last.parse::<usize>().ok()?;
        if last < first {
            return None;
        }
        count = count.checked_add(last - first + 1)?;
    }
    (count > 0).then_some(count)
}

#[cfg(any(target_os = "linux", test))]
fn v2_cpu_capacity(path: &Path, fallback_cores: usize) -> Option<f64> {
    let mut capacity: Option<f64> = None;
    for directory in cgroup_ancestors(path) {
        if let Ok(value) = std::fs::read_to_string(directory.join("cpu.max"))
            && let Some((quota, period)) = value.split_once(' ')
            && quota != "max"
        {
            let quota = quota.parse::<f64>().ok()?;
            let period = period.trim().parse::<f64>().ok()?;
            if quota > 0.0 && period > 0.0 {
                let cores = quota / period;
                capacity = Some(capacity.map_or(cores, |current| current.min(cores)));
            }
        }
    }
    let cpus = std::fs::read_to_string(path.join("cpuset.cpus.effective"))
        .ok()
        .and_then(|value| parse_cpu_list(&value))
        .or((fallback_cores > 0).then_some(fallback_cores));
    match (capacity, cpus) {
        (Some(quota), Some(cpus)) => Some(quota.min(cpus as f64)),
        (Some(quota), None) => Some(quota),
        (None, Some(cpus)) => Some(cpus as f64),
        (None, None) => None,
    }
}

#[cfg(target_os = "linux")]
fn read_v2_cpu_sample(group: &str, fallback_cores: usize) -> Option<CgroupCpuSample> {
    read_v2_cpu_at(Path::new("/sys/fs/cgroup"), group, fallback_cores)
}

#[cfg(any(target_os = "linux", test))]
fn read_v2_cpu_at(root: &Path, group: &str, fallback_cores: usize) -> Option<CgroupCpuSample> {
    let path = safe_cgroup_path(root, group)?;
    let usage_path = path.join("cpu.stat");
    if !usage_path.exists() && path != root {
        return None;
    }
    let usage_path = if usage_path.exists() {
        usage_path
    } else {
        root.join("cpu.stat")
    };
    let stats = std::fs::read_to_string(usage_path).ok()?;
    let usage_usec = stats.lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        (key == "usage_usec")
            .then(|| value.trim().parse().ok())
            .flatten()
    })?;
    let cores = v2_cpu_capacity(&path, fallback_cores)?;
    Some(CgroupCpuSample { usage_usec, cores })
}

#[cfg(target_os = "linux")]
fn v1_cpu_capacity(path: &Path, cpuset: &Path, fallback_cores: usize) -> Option<f64> {
    let mut capacity: Option<f64> = None;
    for directory in cgroup_ancestors(path) {
        if let (Ok(quota), Ok(period)) = (
            std::fs::read_to_string(directory.join("cpu.cfs_quota_us")),
            std::fs::read_to_string(directory.join("cpu.cfs_period_us")),
        ) && let (Ok(quota), Ok(period)) =
            (quota.trim().parse::<f64>(), period.trim().parse::<f64>())
            && quota > 0.0
            && period > 0.0
        {
            let cores = quota / period;
            capacity = Some(capacity.map_or(cores, |current| current.min(cores)));
        }
    }
    let cpus = cgroup_ancestors(cpuset)
        .into_iter()
        .find_map(|directory| {
            std::fs::read_to_string(directory.join("cpuset.cpus"))
                .ok()
                .and_then(|value| parse_cpu_list(&value))
        })
        .or((fallback_cores > 0).then_some(fallback_cores));
    match (capacity, cpus) {
        (Some(quota), Some(cpus)) => Some(quota.min(cpus as f64)),
        (Some(quota), None) => Some(quota),
        (None, Some(cpus)) => Some(cpus as f64),
        (None, None) => None,
    }
}

#[cfg(target_os = "linux")]
fn read_v1_cpu_sample(group: &str, fallback_cores: usize) -> Option<CgroupCpuSample> {
    let cpu_paths = [
        Path::new("/sys/fs/cgroup/cpu"),
        Path::new("/sys/fs/cgroup/cpu,cpuacct"),
    ];
    let cpu_path = cpu_paths
        .iter()
        .filter_map(|root| safe_cgroup_path(root, group))
        .find(|path| path.exists())?;
    let cpuacct_path = safe_cgroup_path(Path::new("/sys/fs/cgroup/cpuacct"), group);
    let usage_path = [
        cpuacct_path.map(|path| path.join("cpuacct.usage")),
        Some(cpu_path.join("cpuacct.usage")),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| candidate.exists())?;
    let usage_usec = std::fs::read_to_string(usage_path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        / 1_000;
    let cores = v1_cpu_capacity(
        &cpu_path,
        &Path::new("/sys/fs/cgroup/cpuset").join(group.trim_start_matches('/')),
        fallback_cores,
    )?;
    Some(CgroupCpuSample { usage_usec, cores })
}

fn cpu_percent_from_samples(
    previous: CgroupCpuSample,
    current: CgroupCpuSample,
    elapsed_seconds: f64,
) -> Option<f64> {
    if !elapsed_seconds.is_finite()
        || elapsed_seconds <= 0.0
        || current.usage_usec < previous.usage_usec
        || current.cores <= 0.0
    {
        return None;
    }
    Some(
        ((current.usage_usec - previous.usage_usec) as f64
            / (elapsed_seconds * 1_000_000.0 * current.cores)
            * 100.0)
            .clamp(0.0, 100.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn guest_cpu_uses_the_machine_boundary_and_its_own_counter_clock() {
        assert_eq!(guest_group("/init.scope"), "/");
        assert_eq!(guest_group("/lxc/guest/init.scope"), "/lxc/guest");
        assert_eq!(guest_group("/docker/guest"), "/docker/guest");
        let start = Instant::now();
        let mut probe = CgroupCpuProbe::default();
        let sample = |usage_usec, cores| Some(CgroupCpuSample { usage_usec, cores });
        assert_eq!(probe.record(sample(0, 2.0), start), None);
        assert_eq!(
            probe.record(sample(1_000_000, 2.0), start + Duration::from_secs(2)),
            Some(25.0)
        );
        // Changing capacity, losing counters, and counter reset all require a new baseline.
        assert_eq!(
            probe.record(sample(2_000_000, 1.0), start + Duration::from_secs(3)),
            None
        );
        assert_eq!(probe.record(None, start + Duration::from_secs(4)), None);
        assert_eq!(
            probe.record(sample(3_000_000, 1.0), start + Duration::from_secs(5)),
            None
        );
        assert_eq!(
            probe.record(sample(0, 1.0), start + Duration::from_secs(6)),
            None
        );
    }

    #[test]
    fn guest_cpu_reads_aggregate_usage_instead_of_the_agent_service() {
        let dir = tempfile::tempdir().expect("cgroup fixture");
        std::fs::create_dir_all(dir.path().join("system.slice/hope-agent.service")).unwrap();
        std::fs::write(dir.path().join("cpu.stat"), "usage_usec 9000000\n").unwrap();
        std::fs::write(dir.path().join("cpu.max"), "200000 100000\n").unwrap();
        std::fs::write(dir.path().join("cpuset.cpus.effective"), "0-3\n").unwrap();
        std::fs::write(
            dir.path().join("system.slice/hope-agent.service/cpu.stat"),
            "usage_usec 1000\n",
        )
        .unwrap();
        let sample = read_v2_cpu_at(dir.path(), guest_group("/init.scope"), 4).unwrap();
        assert_eq!(sample.usage_usec, 9_000_000);
        assert_eq!(sample.cores, 2.0);
        assert!(read_v2_cpu_at(dir.path(), "/../outside", 4).is_none());
        assert!(read_v2_cpu_at(dir.path(), "/missing", 4).is_none());
    }

    #[test]
    fn lxc_cpu_usage_is_normalized_to_its_allowed_cpus() {
        let previous = CgroupCpuSample {
            usage_usec: 1_000_000,
            cores: 3.0,
        };
        let current = CgroupCpuSample {
            usage_usec: 1_141_600,
            cores: 3.0,
        };
        assert_eq!(cpu_percent_from_samples(previous, current, 1.0), Some(4.72));
        assert_eq!(parse_cpu_list("0-2"), Some(3));
        assert_eq!(parse_cpu_list("0-1,4"), Some(3));
    }
}
