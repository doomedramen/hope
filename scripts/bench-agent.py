#!/usr/bin/env python3
"""Repeatable resource benchmark for a release hope-agent.

Starts a throwaway hope-server (temporary data directory, free loopback port) behind a
byte-counting TLS proxy, enrolls a foreground `hope-agent run` (temporary HOPE_AGENT_DIR),
warms up, samples the agent once per second for the measurement window, then kills the
server for an outage window and measures recovery. Results are written as JSON.

Nothing here touches ./releases, ./data or any running server. Python 3.9+, standard library only.
Supported sampling back ends: macOS (libproc via ctypes) and Linux (/proc). Only macOS arm64
has been exercised; see docs/benchmarks and docs/validation.md for what was actually run.
"""
import argparse
import asyncio
import hashlib
import json
import multiprocessing
import os
import platform
import re
import secrets
import shutil
import signal
import ssl
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DARWIN = sys.platform == "darwin"


# ---------------------------------------------------------------- process sampling
if DARWIN:
    import ctypes

    class _RusageV2(ctypes.Structure):
        _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [
            (name, ctypes.c_uint64)
            for name in (
                "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins",
                "wired_size", "resident_size", "phys_footprint", "proc_start_abstime",
                "proc_exit_abstime", "child_user_time", "child_system_time",
                "child_pkg_idle_wkups", "child_interrupt_wkups", "child_pageins",
                "child_elapsed_abstime", "diskio_bytesread", "diskio_byteswritten",
            )
        ]

    class _Timebase(ctypes.Structure):
        _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]

    _libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    _timebase = _Timebase()
    _libc.mach_timebase_info(ctypes.byref(_timebase))
    _NS = _timebase.numer / _timebase.denom / 1e9  # mach absolute units -> seconds

    def _rusage(pid):
        info = _RusageV2()
        if _libc.proc_pid_rusage(pid, 2, ctypes.byref(info)) != 0:
            raise ProcessLookupError(pid)
        return info

    def read_process(pid):
        info = _rusage(pid)
        return {
            "rss": info.resident_size,
            "footprint": info.phys_footprint,
            "cpu": (info.user_time + info.system_time) * _NS,
            "child_cpu": (info.child_user_time + info.child_system_time) * _NS,
            "start": info.proc_start_abstime,
        }

    def child_rss(pid):
        buffer = (ctypes.c_int * 256)()
        count = _libc.proc_listchildpids(pid, buffer, ctypes.sizeof(buffer))
        total, n = 0, 0
        for child in list(buffer)[: max(0, count)]:
            if child > 0:
                try:
                    total += _rusage(child).resident_size
                    n += 1
                except ProcessLookupError:
                    pass
        return total, n

    def cpu_model():
        return subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()

    def os_version():
        return "macOS " + subprocess.check_output(["sw_vers", "-productVersion"], text=True).strip() + " (" + \
            subprocess.check_output(["sw_vers", "-buildVersion"], text=True).strip() + ")"

    def memory_total():
        return int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True))
else:
    _TICKS = os.sysconf("SC_CLK_TCK")

    def read_process(pid):
        stat = Path("/proc/%d/stat" % pid).read_text()
        fields = stat[stat.rfind(")") + 2:].split()
        rss = 0
        for line in Path("/proc/%d/status" % pid).read_text().splitlines():
            if line.startswith("VmRSS:"):
                rss = int(line.split()[1]) * 1024
        return {
            "rss": rss, "footprint": None,
            "cpu": (int(fields[11]) + int(fields[12])) / _TICKS,
            "child_cpu": (int(fields[13]) + int(fields[14])) / _TICKS,
            "start": fields[19],
        }

    def child_rss(pid):
        total, n = 0, 0
        try:
            for task in Path("/proc/%d/task" % pid).iterdir():
                for child in (task / "children").read_text().split():
                    for line in Path("/proc/%s/status" % child).read_text().splitlines():
                        if line.startswith("VmRSS:"):
                            total += int(line.split()[1]) * 1024
                            n += 1
        except OSError:
            pass
        return total, n

    def cpu_model():
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
        return "unavailable"

    def os_version():
        for line in Path("/etc/os-release").read_text().splitlines():
            if line.startswith("PRETTY_NAME="):
                return line.split("=", 1)[1].strip('"')
        return platform.platform()

    def memory_total():
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1]) * 1024
        return 0


# ---------------------------------------------------------------- counting TLS proxy
def run_proxy(front_port, back_port, cert, key, counters):
    """Separate process so its CPU is never confused with the sampler's.

    Counters are plaintext HTTP bytes after TLS termination: TLS record overhead and handshakes
    are NOT included. The agent still performs the TLS work, so its CPU/RSS include it."""
    signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
    up, down, reports, connections, refused = counters

    def bump(counter, amount):
        with counter.get_lock():
            counter.value += amount

    async def pump(reader, writer, counter, report_marker):
        try:
            while True:
                chunk = await reader.read(65536)
                if not chunk:
                    break
                bump(counter, len(chunk))
                if report_marker:
                    bump(reports, chunk.count(report_marker))
                writer.write(chunk)
                await writer.drain()
        except (OSError, ssl.SSLError, asyncio.CancelledError):
            pass
        finally:
            try:
                writer.close()
            except Exception:
                pass

    async def handle(reader, writer):
        bump(connections, 1)
        try:
            upstream_reader, upstream_writer = await asyncio.open_connection("127.0.0.1", back_port)
        except OSError:
            bump(refused, 1)
            writer.close()
            return
        await asyncio.gather(
            pump(reader, upstream_writer, up, b"POST /api/v1/agent/report"),
            pump(upstream_reader, writer, down, None),
        )

    async def main():
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        server = await asyncio.start_server(handle, "127.0.0.1", front_port, ssl=context)
        async with server:
            await server.serve_forever()

    asyncio.run(main())


# ---------------------------------------------------------------- helpers
def free_port():
    import socket
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def rel(path):
    try:
        return str(Path(path).resolve().relative_to(REPO))
    except ValueError:
        return Path(path).name


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def sh(*args, **kw):
    return subprocess.check_output(args, text=True, **kw).strip()


def percentile(values, q):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(q * (len(ordered) - 1))))]


def summarize_costs(costs):
    by_name, totals = {}, []
    for sample in costs:
        totals.append(sum(item["milliseconds"] for item in sample))
        for item in sample:
            by_name.setdefault(item["collector"], []).append(item["milliseconds"])
    stats = lambda v: {"n": len(v), "median_ms": statistics.median(v), "p95_ms": percentile(v, 0.95), "max_ms": max(v)}
    return {"collectors": {k: stats(v) for k, v in sorted(by_name.items())}, "sum_of_collectors": stats(totals)}


def make_certificates(directory):
    ca_key, ca = directory / "ca.key", directory / "ca.pem"
    key, csr, leaf, ext = directory / "leaf.key", directory / "leaf.csr", directory / "leaf.pem", directory / "leaf.ext"
    quiet = {"stderr": subprocess.DEVNULL}
    sh("openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes",
       "-subj", "/CN=hope-bench-ca", "-days", "2", "-addext", "basicConstraints=critical,CA:TRUE",
       "-addext", "keyUsage=critical,keyCertSign", "-keyout", str(ca_key), "-out", str(ca), **quiet)
    sh("openssl", "req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes",
       "-subj", "/CN=127.0.0.1", "-keyout", str(key), "-out", str(csr), **quiet)
    ext.write_text("subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n"
                   "authorityKeyIdentifier=keyid\nsubjectKeyIdentifier=hash\n")
    sh("openssl", "x509", "-req", "-in", str(csr), "-CA", str(ca), "-CAkey", str(ca_key), "-CAcreateserial",
       "-days", "2", "-extfile", str(ext), "-out", str(leaf), **quiet)
    return ca, leaf, key


class Api:
    """Plain-HTTP calls straight to the server's loopback port (not through the proxy)."""

    def __init__(self, port):
        self.base = "http://127.0.0.1:%d" % port
        self.cookie = None

    def call(self, path, body=None):
        headers = {"Content-Type": "application/json"}
        if self.cookie:
            headers["Cookie"] = self.cookie
        request = urllib.request.Request(self.base + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
        with urllib.request.urlopen(request, timeout=10) as response:
            cookie = response.headers.get("Set-Cookie")
            if cookie:
                self.cookie = cookie.split(";", 1)[0]
            return json.load(response)


def wait_healthy(port, deadline=30):
    end = time.monotonic() + deadline
    while time.monotonic() < end:
        try:
            urllib.request.urlopen("http://127.0.0.1:%d/healthz" % port, timeout=2).read()
            return
        except OSError:
            time.sleep(0.2)
    raise RuntimeError("server did not become healthy")


def spool_count(path):
    try:
        data = json.loads(Path(path).read_text())
        return len(data), Path(path).stat().st_size
    except (OSError, ValueError):
        return None, None


# ---------------------------------------------------------------- measurement
class Sampler:
    def __init__(self, agent_pid, agent_dir, counters):
        self.pid, self.dir, self.counters = agent_pid, agent_dir, counters
        self.rows = []
        self.self_cpu0 = time.process_time()
        self.t0 = time.monotonic()
        self.start = read_process(agent_pid)["start"]

    def counter_values(self):
        return [c.value for c in self.counters]

    def sample(self, phase, with_spool=False):
        now = time.monotonic()
        process = read_process(self.pid)
        if process["start"] != self.start:
            raise RuntimeError("agent process restarted during measurement")
        children, child_count = child_rss(self.pid)
        row = {"t": round(now - self.t0, 3), "phase": phase, "rss": process["rss"], "footprint": process["footprint"],
               "cpu": process["cpu"], "child_cpu": process["child_cpu"], "child_rss": children, "children": child_count,
               "sampler_cpu": time.process_time() - self.self_cpu0, "proxy": self.counter_values()}
        if with_spool:
            row["spool_reports"], row["spool_bytes"] = spool_count(self.dir / "spool.json")
        self.rows.append(row)
        return row

    def phase_rows(self, phase):
        return [r for r in self.rows if r["phase"] == phase]


def phase_stats(rows):
    if len(rows) < 2:
        return None
    wall = rows[-1]["t"] - rows[0]["t"]
    agent_cpu = rows[-1]["cpu"] - rows[0]["cpu"]
    child_cpu = rows[-1]["child_cpu"] - rows[0]["child_cpu"]
    sampler_cpu = rows[-1]["sampler_cpu"] - rows[0]["sampler_cpu"]
    rss = [r["rss"] for r in rows]
    footprints = [r["footprint"] for r in rows if r["footprint"] is not None]
    up, down, reports = (rows[-1]["proxy"][i] - rows[0]["proxy"][i] for i in range(3))
    result = {
        "samples": len(rows), "wall_seconds": wall,
        "rss_median_bytes": statistics.median(rss), "rss_p95_bytes": percentile(rss, 0.95), "rss_peak_bytes": max(rss),
        "child_rss_peak_sampled_bytes": max(r["child_rss"] for r in rows),
        "agent_cpu_seconds": agent_cpu, "collector_child_cpu_seconds": child_cpu,
        "cpu_percent_one_core_agent_only": agent_cpu / wall * 100,
        "cpu_percent_one_core_including_children": (agent_cpu + child_cpu) / wall * 100,
        "sampler_cpu_seconds": sampler_cpu, "sampler_cpu_percent_one_core": sampler_cpu / wall * 100,
        "bytes_client_to_server": up, "bytes_server_to_client": down, "report_requests": reports,
    }
    if footprints:
        result["footprint_median_bytes"] = statistics.median(footprints)
        result["footprint_peak_bytes"] = max(footprints)
    if reports:
        result["bytes_per_report_request_up"] = up / reports
        result["bytes_per_report_request_down"] = down / reports
    result["bytes_per_hour_both_directions_extrapolated"] = (up + down) / wall * 3600
    return result


def tick_until(sampler, phase, seconds, with_spool=False, stop=None):
    start = time.monotonic()
    nxt = start
    while True:
        sampler.sample(phase, with_spool)
        if stop and stop():
            return
        nxt += 1.0
        if time.monotonic() - start >= seconds:
            return
        time.sleep(max(0.0, nxt - time.monotonic()))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--server-bin", default=str(REPO / "target/release/hope-server"))
    parser.add_argument("--agent-bin", default=str(REPO / "target/release/hope-agent"))
    parser.add_argument("--output", help="default: docs/benchmarks/agent-<platform>-<arch>-<cpu-slug>.json")
    parser.add_argument("--warmup", type=int, default=60)
    parser.add_argument("--duration", type=int, default=1800, help="steady-state measurement seconds")
    parser.add_argument("--outage", type=int, default=300, help="seconds the server stays killed after the steady phase; 0 skips")
    parser.add_argument("--recovery-max", type=int, default=300)
    parser.add_argument("--collector-samples", type=int, default=120, help="samples for `hope-agent benchmark` (2..120)")
    parser.add_argument("--keep", action="store_true", help="keep the temporary directory")
    args = parser.parse_args()
    for binary in (args.server_bin, args.agent_bin):
        if not os.access(binary, os.X_OK):
            sys.exit("missing executable: %s (run scripts/bench-agent.sh to build)" % binary)

    machine = platform.machine().lower().replace("aarch64", "arm64")
    plat = "macos" if DARWIN else platform.system().lower()
    cpu = cpu_model()
    slug = re.sub(r"[^a-z0-9]+", "-", cpu.lower()).strip("-")
    output = Path(args.output) if args.output else REPO / "docs/benchmarks" / ("agent-%s-%s-%s.json" % (plat, machine, slug))

    work = Path(tempfile.mkdtemp(prefix="hope-bench-"))
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, lambda *_: sys.exit(128 + 15))
    os.chmod(work, 0o700)
    proxy = None
    agent_dir = data_dir = server_env = counters = front = back = ca = None
    procs, log = [], (lambda m: print("[bench %s] %s" % (time.strftime("%H:%M:%S"), m), flush=True))
    try:
        agent_dir, data_dir = work / "agent", work / "data"
        for d in (data_dir, work / "web", work / "releases"):
            d.mkdir()
        ca, leaf, key = make_certificates(work)
        front, back = free_port(), free_port()
        counters = [multiprocessing.Value("Q", 0) for _ in range(5)]
        proxy = multiprocessing.Process(target=run_proxy, args=(front, back, str(leaf), str(key), counters), daemon=True)
        server_env = dict(os.environ, HOPE_DATA_DIR=str(data_dir), HOPE_BIND="127.0.0.1:%d" % back,
                          HOPE_SERVER_URL="https://127.0.0.1:%d" % front, HOPE_SETUP_TOKEN=secrets.token_hex(24),
                          HOPE_WEB_DIR=str(work / "web"), HOPE_RELEASE_DIR=str(work / "releases"), HOPE_DISCOVERY_MODE="limited")
        server_env.pop("HOPE_TRUSTED_PROXIES", None)
    except BaseException:
        shutil.rmtree(work, ignore_errors=True)
        raise

    def start_server():
        p = subprocess.Popen([args.server_bin], env=server_env, stdout=open(work / "server.log", "ab"), stderr=subprocess.STDOUT)
        procs.append(p)
        wait_healthy(back)
        return p

    result = {"tool": "scripts/bench-agent.py", "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    agent = server = None
    try:
        proxy.start()
        server = start_server()
        api = Api(back)
        password = secrets.token_hex(16)
        api.call("/api/v1/auth/setup", {"password": password, "setup_token": server_env["HOPE_SETUP_TOKEN"]})
        if not api.cookie:
            api.call("/api/v1/auth/login", {"password": password})
        token = api.call("/api/v1/enrollment-tokens", {})["token"]

        agent_env = dict(os.environ, HOPE_AGENT_DIR=str(agent_dir), HOPE_SERVER="https://127.0.0.1:%d" % front, HOPE_CA_CERT=str(ca))
        # Collector cost distribution: separate short-lived process, outside the measured window.
        log("collector benchmark: %d samples (own process, not part of the resource window)" % args.collector_samples)
        bench_out = subprocess.run([args.agent_bin, "benchmark", str(args.collector_samples)], env=dict(agent_env, HOPE_AGENT_DIR=str(work / "bench-agent")),
                                   capture_output=True, text=True, timeout=600, check=True).stdout
        bench = json.loads(bench_out)
        inspect = json.loads(subprocess.run([args.agent_bin, "inspect"], env=agent_env, capture_output=True, text=True, timeout=120, check=True).stdout)
        log("enrolling")
        subprocess.run([args.agent_bin, "enroll"], env=dict(agent_env, HOPE_ENROLLMENT_TOKEN=token), check=True, timeout=120,
                       stdout=subprocess.DEVNULL)
        agent = subprocess.Popen([args.agent_bin, "run"], env=agent_env, stdout=open(work / "agent.log", "ab"), stderr=subprocess.STDOUT)
        procs.append(agent)
        sampler = Sampler(agent.pid, agent_dir, counters)

        log("warmup %ds" % args.warmup)
        tick_until(sampler, "warmup", args.warmup)
        log("steady measurement %ds" % args.duration)
        tick_until(sampler, "steady", args.duration)
        steady = phase_stats(sampler.phase_rows("steady"))
        phases = {"warmup": phase_stats(sampler.phase_rows("warmup")), "steady": steady}

        outage = None
        if args.outage > 0:
            log("killing server for %ds" % args.outage)
            server.kill()
            server.wait()
            tick_until(sampler, "outage", args.outage, with_spool=True)
            restarted_at = time.time()
            server = start_server()
            log("server restarted; waiting for recovery")
            marker = agent_dir / "last-report"

            def recovered():
                n, _ = spool_count(agent_dir / "spool.json")
                try:
                    return n == 0 and float(marker.read_text()) > restarted_at
                except (OSError, ValueError):
                    return False

            tick_until(sampler, "recovery", args.recovery_max, with_spool=True, stop=recovered)
            recovery_rows = sampler.phase_rows("recovery")
            outage_rows = sampler.phase_rows("outage")
            try:
                accepted_after = float(marker.read_text()) - restarted_at
            except (OSError, ValueError):
                accepted_after = None
            phases["outage"] = phase_stats(outage_rows)
            phases["recovery"] = phase_stats(recovery_rows)
            peak_spool = max([r.get("spool_reports") or 0 for r in outage_rows + recovery_rows])
            peak_spool_bytes = max([r.get("spool_bytes") or 0 for r in outage_rows + recovery_rows])
            outage = {
                "outage_seconds": args.outage, "kill_signal": "SIGKILL",
                "attempts_refused_by_proxy_total": counters[4].value,
                "spool_reports_peak": peak_spool, "spool_bytes_peak": peak_spool_bytes,
                "spool_limits": {"max_reports": 120, "max_bytes": 4194304, "max_age_seconds": 3600},
                "spool_drained": recovered(),
                "seconds_to_empty_spool_after_restart": (recovery_rows[-1]["t"] - recovery_rows[0]["t"]) if recovered() else None,
                "last_report_marker_seconds_after_restart": accepted_after,
                "rss_growth_outage_vs_steady_median_bytes": (phases["outage"]["rss_median_bytes"] - steady["rss_median_bytes"]) if phases["outage"] else None,
                "note": "Spool bound (120 reports / 4 MiB / 1 h) is exercised by unit tests, not by this outage length.",
            }
            if args.outage < 3600:
                outage["note"] += " An outage of %d s queues about %d reports." % (args.outage, args.outage // 30)
        hosts = api.call("/api/v1/bootstrap")["hosts"]
        agent_dead = agent.poll() is not None
        target_rss, target_cpu = 30 * 1024 * 1024, 0.5
        result.update({
            "platform": plat, "architecture": machine, "os": os_version(), "cpu_model": cpu,
            "logical_cpus": os.cpu_count(), "memory_total_bytes": memory_total(),
            "rust": sh("rustc", "-V"), "python": platform.python_version(),
            "source_commit": sh("git", "-C", str(REPO), "rev-parse", "--short", "HEAD"),
            "rust_sources_dirty": bool(sh("git", "-C", str(REPO), "status", "--porcelain", "--", "crates", "Cargo.toml", "Cargo.lock")),
            "agent_binary": rel(args.agent_bin), "agent_binary_sha256": sha256(args.agent_bin),
            "server_binary_sha256": sha256(args.server_bin),
            "agent_version": sh(args.agent_bin, "--version"),
            "configured": {"warmup_seconds": args.warmup, "duration_seconds": args.duration, "outage_seconds": args.outage,
                           "report_interval": "30 s with 28-32 s jitter (agent default)", "sampling_interval_seconds": 1},
            "agent_inventory_gpus": inspect["inventory"].get("gpus"),
            "agent_collectors": inspect["collectors"],
            "transport": "https loopback via counting TLS proxy (private test CA, strict verification); TLS handled by the agent's rustls; "
                         "byte counts are plaintext HTTP bytes after TLS termination (no TLS record/handshake overhead); "
                         "traffic on loopback only, no real network latency",
            "measurement_method": {
                "cpu": "agent user+system CPU from the OS per-process accounting (macOS proc_pid_rusage, Linux /proc/stat), delta over wall clock; "
                       "waited collector children added separately (child CPU of terminated, waited-for children)",
                "memory": "resident set (RSS) of the agent process once per second; macOS also physical footprint; "
                          "child RSS only sampled at 1 s so short-lived collector children can be missed",
                "overhead": "sampler CPU = this script's own process CPU, reported separately; proxy and server run in other processes and are not charged to the sampler or agent",
            },
            "phases": phases, "outage_test": outage,
            "collector_costs": {"samples": bench["samples"], "total_collection_ms": {
                "median_ms": statistics.median(bench["collection_ms"]), "p95_ms": percentile(bench["collection_ms"], 0.95), "max_ms": max(bench["collection_ms"])},
                "per_collector": summarize_costs(bench["collector_costs"]),
                "note": "From `hope-agent benchmark` in a separate process; the first sample includes one-time initialisation. Inventory (hostname/OS/disks/network) is rebuilt on every sample and sent when it changes or hourly; gpu_inventory refreshes hourly."},
            "server_saw_agent_host": any(h.get("agent_id") for h in hosts),
            "agent_exited_early": agent_dead,
            "targets": {
                "rss_below_30_MiB": {"median": steady["rss_median_bytes"] < target_rss, "peak": steady["rss_peak_bytes"] < target_rss},
                "avg_cpu_below_0.5_percent_one_core": steady["cpu_percent_one_core_including_children"] < target_cpu,
            },
            "samples_columns": ["t", "phase", "rss", "footprint", "cpu", "child_cpu", "child_rss", "sampler_cpu"],
            "samples": [[r["t"], r["phase"], r["rss"], r["footprint"], round(r["cpu"], 4), round(r["child_cpu"], 4), r["child_rss"], round(r["sampler_cpu"], 4)] for r in sampler.rows],
            "finished_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        })
        output.parent.mkdir(parents=True, exist_ok=True)
        tmp = output.with_suffix(".tmp")
        tmp.write_text(json.dumps(result, indent=1) + "\n")
        tmp.replace(output)
        log("wrote %s" % output)
        log("steady: rss median %.2f MiB peak %.2f MiB, cpu %.4f%% of one core" % (
            steady["rss_median_bytes"] / 1048576, steady["rss_peak_bytes"] / 1048576, steady["cpu_percent_one_core_including_children"]))
    finally:
        for p in reversed(procs):
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(10)
                except subprocess.TimeoutExpired:
                    p.kill()
        if proxy is not None and proxy.is_alive():
            proxy.terminate()
        if args.keep:
            log("kept %s" % work)
        else:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
