//! A fresh memory snapshot prevents failed refreshes from reusing old readings.
#[derive(Debug)]
pub struct Sample {
    pub used: Option<f64>,
    pub total: Option<f64>,
    pub swap_used: Option<f64>,
    pub swap_total: Option<f64>,
}

pub fn sample() -> Sample {
    // sysinfo's memory refresh has no success result and may leave old values
    // untouched on failure. Start empty so a failed read stays unknown.
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    from_counters(
        system.total_memory(),
        system.used_memory(),
        system.total_swap(),
        system.used_swap(),
    )
}

fn pair(total: u64, used: u64) -> (Option<f64>, Option<f64>) {
    if total == 0 || total as f64 > 1e18 {
        return (None, None);
    }
    ((used <= total).then_some(used as f64), Some(total as f64))
}

fn from_counters(total: u64, used: u64, swap_total: u64, swap_used: u64) -> Sample {
    let (used, total) = pair(total, used);
    let (swap_used, swap_total) = pair(swap_total, swap_used);
    Sample {
        used,
        total,
        swap_used,
        swap_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_or_inconsistent_memory_does_not_become_zero_usage() {
        let missing = from_counters(0, 0, 0, 0);
        assert!(missing.used.is_none() && missing.total.is_none());
        let inconsistent = from_counters(100, 101, 50, 51);
        assert_eq!(inconsistent.total, Some(100.0));
        assert!(inconsistent.used.is_none());
        assert_eq!(inconsistent.swap_total, Some(50.0));
        assert!(inconsistent.swap_used.is_none());
        let idle = from_counters(100, 0, 50, 0);
        assert_eq!(idle.used, Some(0.0));
        assert_eq!(idle.swap_used, Some(0.0));
        let valid = from_counters(100, 40, 50, 20);
        assert_eq!(valid.used, Some(40.0));
        assert_eq!(valid.swap_used, Some(20.0));
    }
}
