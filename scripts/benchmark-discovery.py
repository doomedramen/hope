#!/usr/bin/env python3
"""Disposable Linux bridge fixture. Never probes the developer's LAN."""
import argparse
import json
import secrets
import subprocess
import time
import urllib.request
import http.cookiejar
import uuid


def docker(*arguments):
    return subprocess.check_output(["docker", *arguments], text=True).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="hope:validation")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--on-link", action="store_true", help="Attach the server to both ranges instead of routing the second")
    parser.add_argument("--output", default="target/discovery-benchmark.json")
    args = parser.parse_args()
    identifier = "hope-fixture-" + uuid.uuid4().hex[:8]
    containers, networks = [], []
    result = {"fixture": "Two isolated /24 bridges; one HTTP-only responder each; remaining addresses silent; second range routed through .252" if not args.on_link else "Two directly attached isolated /24 bridges; one HTTP-only responder each; remaining addresses silent", "probe_starts_per_second_limit": 200, "runs": [], "docker": docker("version", "--format", "{{.Server.Version}}"), "image": docker("image", "inspect", args.image, "--format", "{{.Id}}")}
    try:
        docker("build", "-q", "-t", "hope:discovery-responder", "tests/discovery")
        for index, subnet in enumerate(["198.18.1.0/24", "198.18.30.0/24"]):
            network = identifier + "-net-" + str(index)
            docker("network", "create", "--internal", "--subnet", subnet, "--gateway", subnet.replace("0/24", "253"), network)
            networks.append(network)
            name = identifier + "-responder-" + str(index)
            containers.append(name)
            docker("run", "--detach", "--rm", "--name", name, "--network", network, "--ip", subnet.replace("0/24", "1"), "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "--cap-add", "NET_RAW", "hope:discovery-responder")
        if not args.on_link:
            router = identifier + "-router"
            containers.append(router)
            docker("run", "--detach", "--rm", "--name", router, "--network", networks[0], "--ip", "198.18.1.252", "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "--sysctl", "net.ipv4.ip_forward=1", "hope:discovery-responder", "sh", "-c", "iptables -P FORWARD ACCEPT; sleep 600")
            docker("network", "connect", "--ip", "198.18.30.252", networks[1], router)
            docker("exec", identifier + "-responder-1", "ip", "route", "add", "198.18.1.0/24", "via", "198.18.30.252")
        server = identifier + "-server"
        token = secrets.token_hex(32)
        docker("run", "--detach", "--rm", "--name", server, "--network", "bridge", "--cap-drop", "ALL", "--cap-add", "NET_RAW", "-p", "127.0.0.1::8080", "-e", "HOPE_SERVER_URL=http://localhost:8080", "-e", "HOPE_DISCOVERY_MODE=native", "-e", "HOPE_SETUP_TOKEN=" + token, "--entrypoint", "/bin/sh", args.image, "-c", "sleep 2; exec hope-server")
        containers.append(server)
        docker("network", "connect", "--ip", "198.18.1.254", networks[0], server)
        if args.on_link:
            docker("network", "connect", "--ip", "198.18.30.254", networks[1], server)
        else:
            docker("run", "--rm", "--network", "container:" + server, "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "hope:discovery-responder", "ip", "route", "add", "198.18.30.0/24", "via", "198.18.1.252")
        binding = json.loads(docker("inspect", server))[0]["NetworkSettings"]["Ports"]["8080/tcp"][0]
        base = "http://127.0.0.1:" + binding["HostPort"]
        opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

        def request(path, body=None):
            req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers={"Content-Type": "application/json"})
            with opener.open(req, timeout=5) as response:
                return json.load(response)

        for _ in range(100):
            try:
                request("/healthz")
                break
            except (OSError, ValueError):
                time.sleep(0.1)
        request("/api/v1/auth/setup", {"password": secrets.token_hex(24), "setup_token": token})
        net_ids = [request("/api/v1/networks", {"name": name, "cidr": cidr, "enabled": False})["id"] for name, cidr in [("HTTP-only", "198.18.1.0/24"), ("Second range", "198.18.30.0/24")]]
        for run in range(args.runs):
            start = time.monotonic()
            jobs = [request("/api/v1/networks/" + net + "/scan", {}) for net in net_ids]
            ids = {job["id"] for job in jobs}
            first = None
            while time.monotonic() - start < 60:
                data = request("/api/v1/bootstrap")
                elapsed = time.monotonic() - start
                if first is None and any(host["last_seen"] >= jobs[0]["started_at"] and any(address in ["198.18.1.1", "198.18.30.1"] for address in host["addresses"]) for host in data["hosts"]):
                    first = elapsed
                scans = [scan for scan in data["scans"] if scan["id"] in ids]
                if len(scans) == 2 and all(scan["status"] == "completed" for scan in scans):
                    result["runs"].append({"run": run + 1, "first_responsive_host_seconds": first, "both_completed_seconds": elapsed, "scans": scans})
                    print(json.dumps(result["runs"][-1]), flush=True)
                    break
                if any(scan["status"] in ["failed", "interrupted"] for scan in scans):
                    raise RuntimeError("Discovery failed: " + json.dumps(scans))
                time.sleep(0.05)
            else:
                raise RuntimeError("Discovery exceeded the 60-second fixture deadline")
        result["capabilities"] = data["capabilities"]
        result["http_only_hosts_detected"] = all(any(address in host["addresses"] and any(service["port"] == 80 for service in host["services"]) for host in data["hosts"]) for address in ["198.18.1.1", "198.18.30.1"])
        result["targets_passed"] = result["http_only_hosts_detected"] and all(run["first_responsive_host_seconds"] is not None and run["first_responsive_host_seconds"] < 5 and run["both_completed_seconds"] < 30 for run in result["runs"])
    finally:
        for name in reversed(containers):
            subprocess.run(["docker", "rm", "--force", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for name in reversed(networks):
            subprocess.run(["docker", "network", "rm", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        from pathlib import Path
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2) + "\n")
    if not result.get("targets_passed"):
        raise SystemExit("Discovery targets not met; inspect " + args.output)


if __name__ == "__main__":
    main()
