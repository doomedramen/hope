#!/usr/bin/env python3
"""Run the opt-in TLS soak and sample native-process/PostgreSQL resources.

Requires HOPE_SOAK_DATABASE_URL for an empty disposable database. The PostgreSQL
role must be allowed to CHECKPOINT. HOPE_SOAK_ROUNDS defaults to 40 (10 minutes).
The native process includes simulated clients and the server; its resource use
must not be reported as standalone server usage. No database is deleted here.
"""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time


def cpu_seconds(value):
    days, _, clock = value.rpartition("-")
    total = 0.0
    for part in clock.split(":"):
        total = total * 60 + float(part)
    return total + int(days or 0) * 86400


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--postgres-container", required=True)
    parser.add_argument("--output", type=Path, required=True, help="Report directory")
    args = parser.parse_args()
    if not os.environ.get("HOPE_SOAK_DATABASE_URL"):
        parser.error("HOPE_SOAK_DATABASE_URL must name an empty disposable database")
    args.output.mkdir(parents=True, exist_ok=True)
    report = args.output / "workload.json"
    log = args.output / "workload.log"
    if report.exists() or log.exists():
        parser.error("output directory must not contain an earlier workload report")
    env = {**os.environ, "HOPE_SOAK_REPORT": str(report.resolve())}
    samples = []
    errors = []
    with log.open("w") as output:
        process = subprocess.Popen(
            ["cargo", "test", "-p", "server", "sustained_500_agent_tls_workload",
             "--", "--ignored", "--nocapture"],
            cwd=Path(__file__).resolve().parent.parent,
            stdout=output, stderr=subprocess.STDOUT, env=env, start_new_session=True,
        )
        try:
            while process.poll() is None:
                match = re.search(r"SOAK_STARTED pid=(\d+)", log.read_text())
                if match:
                    sample = {"at_unix_seconds": time.time()}
                    try:
                        result = subprocess.check_output(
                            ["ps", "-p", match[1], "-o", "rss=,time="], text=True,
                        ).strip().split()
                        sample.update(native_rss_bytes=int(result[0]) * 1024,
                                      native_cpu_seconds=cpu_seconds(result[1]))
                        stats = subprocess.check_output(
                            ["docker", "stats", "--no-stream", "--format", "{{json .}}",
                             args.postgres_container], text=True, timeout=10,
                        )
                        sample["postgres_container"] = json.loads(stats)
                        samples.append(sample)
                    except (subprocess.SubprocessError, ValueError, IndexError) as error:
                        errors.append({"at_unix_seconds": time.time(), "error": str(error)})
                time.sleep(10)
        except BaseException:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait()
            raise
        finally:
            summary = {
                "scope": "Native RSS/CPU includes simulated clients and server together. Docker stats covers the named PostgreSQL container, including other local databases.",
                "samples": samples, "sampling_errors": errors,
                "workload_exit_code": process.poll(),
            }
            if samples:
                summary["native_peak_rss_bytes"] = max(s["native_rss_bytes"] for s in samples)
            if len(samples) > 1:
                elapsed = samples[-1]["at_unix_seconds"] - samples[0]["at_unix_seconds"]
                summary["native_average_cpu_percent_of_one_core"] = 100 * (
                    samples[-1]["native_cpu_seconds"] - samples[0]["native_cpu_seconds"]
                ) / elapsed
            (args.output / "resources.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"Workload exit {process.returncode}. Reports: {args.output}")
    raise SystemExit(process.returncode)


if __name__ == "__main__":
    main()
