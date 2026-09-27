"""Check planning boundaries and policy invariants without a database."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("storage_plan", Path(__file__).with_name("agent-storage-plan.py"))
planner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(planner)
CALIBRATION = {"schema_version": 1, "relations": {
    name: {"rows": 100, "total_bytes": 10000} for name in
    ("agent_metric_samples", "agent_log_entries", "storage_rollup_varied")
}}


class StoragePlanTests(unittest.TestCase):
    def test_retention_policy_counts(self):
        result = planner.estimate(CALIBRATION, agents=500)
        self.assertEqual(result["retained_rows"], {
            "raw_metrics": 20160000, "five_minute_rollups": 4320000,
            "hourly_rollups": 2160000, "host_logs": 302400000,
            "diagnostics": 360000,
        })

    def test_budget_boundary_includes_reserve(self):
        result = planner.estimate(CALIBRATION, agents=2, wal_reserve_gib=8)
        exact = result["planned_gib"]
        at = planner.estimate(CALIBRATION, agents=2, budget_gib=exact)
        below = planner.estimate(CALIBRATION, agents=2, budget_gib=exact - 0.001)
        self.assertTrue(at["fits_planning_budget"])
        self.assertEqual(at["maximum_agents_with_same_rates_and_policies"], 2)
        self.assertFalse(below["fits_planning_budget"])
        self.assertEqual(below["maximum_agents_with_same_rates_and_policies"], 1)
        self.assertEqual(planner.estimate(CALIBRATION, budget_gib=4)["maximum_agents_with_same_rates_and_policies"], 0)

    def test_disabling_host_logs_keeps_diagnostics_and_rollups(self):
        result = planner.estimate(CALIBRATION, logs_per_second=0, raw_days=1)
        self.assertEqual(result["retained_rows"]["host_logs"], 0)
        self.assertGreater(result["retained_rows"]["diagnostics"], 0)
        self.assertEqual(result["retained_rows"]["hourly_rollups"], 2160000)

    def test_invalid_inputs_do_not_produce_plausible_estimates(self):
        for args in ({"agents": 0}, {"logs_per_second": -1}, {"budget_gib": float("nan")},
                     {"logs_per_second": float("inf")}, {"raw_days": 0}, {"log_days": 91}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                planner.estimate(CALIBRATION, **args)


if __name__ == "__main__":
    unittest.main()
