#!/usr/bin/env python3
"""Estimate retained agent storage from measured PostgreSQL relation sizes.

Read-only planning tool. It never connects to a database or changes retention.
Defaults use the September 2026 synthetic workload, including a varied-rollup
compression sensitivity fixture. Recalibrate for different payloads or indexes.
"""
import argparse
import json
import math
from pathlib import Path

GIB = 1024 ** 3
DEFAULT_CALIBRATION = Path(__file__).resolve().parent.parent / (
    "docs/operations/validation/agent-storage-calibration-2026-09-27.json"
)


def estimate(calibration, *, agents=500, logs_per_second=1.0, raw_days=7,
             log_days=7, diagnostics_per_day=24.0, budget_gib=36.0,
             headroom_percent=30.0, wal_reserve_gib=8.0):
    if not isinstance(agents, int) or isinstance(agents, bool) or agents < 1:
        raise ValueError("agents must be a positive integer")
    if not isinstance(raw_days, int) or not 1 <= raw_days <= 3650:
        raise ValueError("raw_days must be an integer from 1 to 3650")
    if not isinstance(log_days, int) or not 1 <= log_days <= 90:
        raise ValueError("log_days must be an integer from 1 to 90")
    for name, value in (("logs_per_second", logs_per_second),
                        ("diagnostics_per_day", diagnostics_per_day),
                        ("budget_gib", budget_gib),
                        ("headroom_percent", headroom_percent),
                        ("wal_reserve_gib", wal_reserve_gib)):
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"{name} must be finite and nonnegative")
    if calibration.get("schema_version") != 1:
        raise ValueError("unsupported calibration schema")
    relations = calibration["relations"]
    costs = {}
    for name in ("agent_metric_samples", "agent_log_entries", "storage_rollup_varied"):
        table = relations[name]
        rows, total = table["rows"], table["total_bytes"]
        if not math.isfinite(rows) or not math.isfinite(total) or rows <= 0 or total <= 0:
            raise ValueError(f"{name} needs positive finite row and byte measurements")
        costs[name] = total / rows
    per_agent_rows = {
        "raw_metrics": 86400 / 15 * raw_days,
        "five_minute_rollups": 86400 / 300 * 30,
        "hourly_rollups": 86400 / 3600 * 180,
        "host_logs": 86400 * logs_per_second * log_days,
        "diagnostics": diagnostics_per_day * 30,
    }
    per_agent_bytes = {
        name: rows * costs[
            "agent_metric_samples" if name == "raw_metrics" else
            "storage_rollup_varied" if name.endswith("rollups") else
            "agent_log_entries"
        ] for name, rows in per_agent_rows.items()
    }
    retained = sum(per_agent_bytes.values()) * agents
    headroom = retained * headroom_percent / 100
    reserve = wal_reserve_gib * GIB
    planned = retained + headroom + reserve
    per_agent_with_headroom = sum(per_agent_bytes.values()) * (1 + headroom_percent / 100)
    maximum = max(0, math.floor((budget_gib * GIB - reserve) / per_agent_with_headroom))
    return {
        "kind": "planning_estimate_not_capacity_certification",
        "inputs": {"agents": agents, "logs_per_agent_second": logs_per_second,
                   "raw_metric_days": raw_days, "host_log_days": log_days,
                   "diagnostics_per_agent_day": diagnostics_per_day,
                   "budget_gib": budget_gib, "headroom_percent": headroom_percent,
                   "wal_reserve_gib": wal_reserve_gib},
        "fixed_policy": {"sample_interval_seconds": 15, "five_minute_days": 30,
                         "hourly_days": 180, "diagnostic_days": 30},
        "bytes_per_row_including_indexes": costs,
        "retained_rows": {name: count * agents for name, count in per_agent_rows.items()},
        "retained_gib_by_stream": {name: size * agents / GIB for name, size in per_agent_bytes.items()},
        "retained_gib": retained / GIB,
        "headroom_gib": headroom / GIB,
        "planned_gib": planned / GIB,
        "fits_planning_budget": planned <= budget_gib * GIB,
        "maximum_agents_with_same_rates_and_policies": maximum,
        "limitations": [
            "Synthetic compact tables are calibration inputs, not full-retention measurements.",
            "Varied rollups model lower compression; different metric dimensions can cost more.",
            "Diagnostics use the measured host-log row cost and the explicitly assumed daily rate.",
            "Headroom and WAL reserve are planning allowances, not measured worst-case bounds.",
            "Budget must exclude other application data, backups, archives and host free-space reserve.",
            "WAL generation rate is not retained WAL size; archives and replication backlog need separate budgets.",
            "No collection or retention settings have been changed.",
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--calibration", type=Path, default=DEFAULT_CALIBRATION)
    parser.add_argument("--agents", type=int, default=500)
    parser.add_argument("--logs-per-second", type=float, default=1.0, help="Per agent")
    parser.add_argument("--raw-days", type=int, default=7)
    parser.add_argument("--log-days", type=int, default=7)
    parser.add_argument("--diagnostics-per-day", type=float, default=24.0, help="Per agent")
    parser.add_argument("--budget-gib", type=float, default=36.0)
    parser.add_argument("--headroom-percent", type=float, default=30.0)
    parser.add_argument("--wal-reserve-gib", type=float, default=8.0)
    args = vars(parser.parse_args())
    try:
        calibration = json.loads(args.pop("calibration").read_text())
        result = estimate(calibration, **args)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2, allow_nan=False))


if __name__ == "__main__":
    main()
