#!/usr/bin/env python3
"""Exercise the packaged server installer and rollback under a real Linux systemd.

Builds or accepts the Linux x86_64 server package, then runs a disposable Debian systemd container.
The container is the only machine changed. Standard library only.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import shutil
import subprocess
import tarfile
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]
IMAGE = "hopegate-server-install:ci"
RELEASE_NAME = "hope-server-x86_64-linux.tar.gz"
SETUP_TOKEN = re.compile(r"first-run setup token: ([A-Za-z0-9_-]+)")
API_SCRIPT = r'''
import http.cookiejar
import json
import os
import urllib.request

base = "http://127.0.0.1:8080"
cookies = http.cookiejar.CookieJar()
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cookies))
def post(path, value):
    data = json.dumps(value).encode()
    request = urllib.request.Request(base + path, data=data, headers={
        "Content-Type": "application/json", "Origin": base,
    })
    with client.open(request, timeout=10) as response:
        return json.load(response)

post("/api/v1/auth/setup", {
    "password": os.environ["GATE_PASSWORD"],
    "setup_token": os.environ["GATE_SETUP_TOKEN"],
})
network = post("/api/v1/networks", {
    "name": "server-activation-gate",
    "cidr": "192.0.2.0/24",
    "enabled": False,
})
request = urllib.request.Request(base + "/api/v1/bootstrap")
with client.open(request, timeout=10) as response:
    bootstrap = json.load(response)
assert any(item["id"] == network["id"] for item in bootstrap["networks"])
'''
DB_SCRIPT = r'''
import sqlite3
db = sqlite3.connect("/var/lib/hope/hope.db")
version = db.execute("PRAGMA user_version").fetchone()[0]
networks = db.execute("SELECT COUNT(*) FROM networks WHERE name=?", ("server-activation-gate",)).fetchone()[0]
retirement = db.execute("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='retired_devices'").fetchone()[0]
print(version, networks, retirement)
'''


def log(*items):
    print(time.strftime("%H:%M:%S"), *items, flush=True)


def run(args, *, timeout=900, check=True):
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if check and result.returncode:
        detail = (result.stderr or result.stdout)[-1800:]
        detail = SETUP_TOKEN.sub("first-run setup token: [redacted]", detail)
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(args[:5])}\n{detail}")
    return result


def docker_exec(container, *args, timeout=900, check=True, env=None):
    command = ["docker", "exec"]
    for key, value in (env or {}).items():
        command.extend(("--env", f"{key}={value}"))
    result = run(command + [container, *args], timeout=timeout, check=False)
    if check and result.returncode:
        detail = (result.stderr or result.stdout)[-1800:]
        detail = SETUP_TOKEN.sub("first-run setup token: [redacted]", detail)
        raise RuntimeError(f"container command failed ({result.returncode}): {' '.join(args[:5])}\n{detail}")
    return result


def work_directory(value):
    path = Path(value).resolve() if value else Path(tempfile.mkdtemp(prefix="hope-server-install-"))
    path.mkdir(parents=True, exist_ok=True)
    try:
        path.relative_to(REPO)
    except ValueError:
        return path
    raise SystemExit("work directory must be outside the repository")


def build_package(work):
    image = IMAGE
    log("building the packaged Linux x86_64 server")
    run(["docker", "build", "--platform", "linux/amd64", "--target", "server-package",
         "-t", image, str(REPO)], timeout=3600)
    container = f"hopegate-server-package-{os.getpid()}-{secrets.token_hex(3)}"
    run(["docker", "create", "--name", container, image])
    output = work / "server-release"
    output.mkdir(exist_ok=True)
    try:
        run(["docker", "cp", f"{container}:/build/dist/.", str(output)])
    finally:
        run(["docker", "rm", container], check=False)
    return output


def validate_package(directory):
    archive = directory / RELEASE_NAME
    sums = directory / "SHA256SUMS-server"
    if not archive.is_file() or not sums.is_file():
        raise RuntimeError(f"package directory must contain {RELEASE_NAME} and SHA256SUMS-server")
    entries = [line.split() for line in sums.read_text().splitlines() if line.strip()]
    listed = [row[0] for row in entries if len(row) >= 2 and row[-1] == RELEASE_NAME]
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if listed != [digest]:
        raise RuntimeError("server package checksum is absent, duplicated or incorrect")
    with tarfile.open(archive, "r:gz") as package:
        names = set()
        for member in package.getmembers():
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts:
                raise RuntimeError("server package contains an unsafe path")
            if not (member.isfile() or member.isdir()):
                raise RuntimeError("server package contains a link or special file")
            normalized = str(path)
            if normalized in names:
                raise RuntimeError("server package contains a duplicate path")
            names.add(normalized)
        required = {"hope-server", "web/index.html", "VERSION", "tools/activate-server.py"}
        if not required.issubset(names):
            raise RuntimeError("server package is missing required installation files")
    return archive


def broken_package(source, work):
    output = work / "broken-release"
    output.mkdir(exist_ok=True)
    extracted = work / "broken-package"
    shutil.rmtree(extracted, ignore_errors=True)
    extracted.mkdir()
    with tarfile.open(source, "r:gz") as package:
        package.extractall(extracted)
    binary = extracted / "hope-server"
    binary.write_text("#!/bin/sh\nexit 19\n")
    binary.chmod(0o755)
    archive = output / RELEASE_NAME
    with tarfile.open(archive, "w:gz") as package:
        for path in sorted(extracted.rglob("*")):
            package.add(path, arcname=path.relative_to(extracted).as_posix(), recursive=False)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output / "SHA256SUMS-server").write_text(f"{digest}  {RELEASE_NAME}\n")
    validate_package(output)
    return output


def install(container, script, release_url, *, expect_success):
    result = docker_exec(
        container,
        "sh", script,
        timeout=900,
        check=False,
        env={
            "HOPE_INSTALL_NONINTERACTIVE": "1",
            "HOPE_INSTALL_TARGET": "host",
            "HOPE_INSTALL_SOURCE": "release",
            "HOPE_INSTALL_PROXY": "none",
            "HOPE_INSTALL_URL": "http://127.0.0.1:8080",
            "HOPE_INSTALL_RELEASE_URL": release_url,
        },
    )
    output = result.stdout + result.stderr
    token = SETUP_TOKEN.search(output)
    if expect_success and result.returncode:
        diagnostics = docker_exec(
            container,
            "sh", "-c", "systemctl status hope --no-pager; journalctl -u hope -n 80 --no-pager",
            check=False,
        )
        detail = SETUP_TOKEN.sub("first-run setup token: [redacted]", diagnostics.stdout + diagnostics.stderr)
        raise RuntimeError("server installation failed: " + SETUP_TOKEN.sub("[redacted]", output[-1800:])
                            + "\nservice diagnostics:\n" + detail[-3500:])
    if not expect_success and result.returncode == 0:
        raise RuntimeError("deliberately unhealthy server package unexpectedly activated")
    if not expect_success and "Server activation failed" not in output:
        raise RuntimeError("unhealthy replacement failed for a reason other than activation")
    if expect_success and not re.search(r"H\.O\.P\.E is installed\.", output):
        raise RuntimeError("installer exited successfully without its success message")
    return token.group(1) if token else None


def wait_systemd(container):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        result = docker_exec(container, "systemctl", "is-system-running", check=False)
        state = (result.stdout + result.stderr).strip()
        if state in ("running", "degraded"):
            init = docker_exec(container, "cat", "/proc/1/comm", check=False)
            units = docker_exec(container, "systemctl", "list-units", "--no-pager", "--no-legend",
                                check=False)
            if init.returncode == 0 and init.stdout.strip() == "systemd" and units.returncode == 0:
                if state == "degraded":
                    failed = docker_exec(container, "systemctl", "--failed", "--no-legend", check=False)
                    details = (failed.stdout or failed.stderr).strip() or "failed services unavailable"
                    log("target systemd is degraded by baseline services; continuing after manager readiness:", details)
                else:
                    log("target systemd is", state)
                # The image may have unrelated failed units before installation. The gate still
                # requires the installed HOPE unit and its HTTP health to pass in validate_running.
                return
        time.sleep(1)
    raise RuntimeError("systemd did not become available in the disposable target")


def validate_running(container):
    active = docker_exec(container, "systemctl", "is-active", "hope").stdout.strip()
    enabled = docker_exec(container, "systemctl", "is-enabled", "hope").stdout.strip()
    pid = docker_exec(container, "systemctl", "show", "--property=MainPID", "--value", "hope").stdout.strip()
    health = docker_exec(container, "curl", "--fail", "--silent", "--show-error", "--max-time", "3",
                         "http://127.0.0.1:8080/healthz").stdout
    body = json.loads(health)
    if active != "active" or enabled != "enabled" or not pid.isdecimal() or int(pid) <= 0:
        raise RuntimeError(f"server unit state is active={active}, enabled={enabled}, MainPID={pid}")
    if body.get("status") != "ok" or body.get("pid") != int(pid):
        raise RuntimeError(f"health is not served by the systemd MainPID: {body}, MainPID={pid}")
    return int(pid)


def validate_data(container):
    output = docker_exec(container, "python3", "-c", DB_SCRIPT).stdout.strip().split()
    if len(output) != 3 or output[0] != "19" or output[1:] != ["1", "1"]:
        raise RuntimeError(f"server database/data did not survive activation: {output}")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-dir", help="prebuilt package directory; otherwise build it with Docker")
    parser.add_argument("--work", help="preserve artifacts in this directory outside the repository")
    args = parser.parse_args()
    if not shutil.which("docker"):
        raise SystemExit("Docker is required")

    work = work_directory(args.work or os.environ.get("HOPEGATE_WORK"))
    log("work directory", work)
    package_dir = Path(args.package_dir).resolve() if args.package_dir else build_package(work)
    archive = validate_package(package_dir)
    bad_dir = broken_package(archive, work)
    container = f"hopegate-server-install-{os.getpid()}-{secrets.token_hex(3)}"
    image = os.environ.get("HOPEGATE_SERVER_INSTALL_IMAGE", IMAGE)
    if not os.environ.get("HOPEGATE_SERVER_INSTALL_IMAGE"):
        log("building disposable Debian 13 systemd target")
        run(["docker", "build", "--platform", "linux/amd64", "-f", str(REPO / "tests/gates/target.Dockerfile"),
             "-t", image, str(REPO / "tests/gates")], timeout=1800)
    run(["docker", "rm", "-f", container], check=False)
    run(["docker", "run", "-d", "--name", container, "--platform", "linux/amd64", "--privileged",
         "--cgroupns=host", "--tmpfs", "/tmp:exec,mode=1777", "--tmpfs", "/run", "--tmpfs", "/run/lock",
         "-v", "/sys/fs/cgroup:/sys/fs/cgroup:rw", image])
    try:
        wait_systemd(container)
        docker_exec(container, "mkdir", "-p", "/root/hope-release", "/root/hope-release-broken")
        for name in (RELEASE_NAME, "SHA256SUMS-server"):
            run(["docker", "cp", str(package_dir / name), f"{container}:/root/hope-release/{name}"])
            run(["docker", "cp", str(bad_dir / name), f"{container}:/root/hope-release-broken/{name}"])
        run(["docker", "cp", str(REPO / "scripts/install-hope.sh"), f"{container}:/root/hope-install.sh"])
        staged = docker_exec(container, "ls", "/root/hope-release").stdout.split()
        if RELEASE_NAME not in staged or "SHA256SUMS-server" not in staged:
            raise RuntimeError(f"release package was not staged in the target: {staged}")
        docker_exec(container, "curl", "--fail", "--silent", "--show-error", "--head",
                    "file:///root/hope-release/" + RELEASE_NAME)

        token = install(container, "/root/hope-install.sh", "file:///root/hope-release", expect_success=True)
        first_pid = validate_running(container)
        if not token:
            journal = docker_exec(container, "journalctl", "-u", "hope", "--no-pager").stdout
            match = SETUP_TOKEN.search(journal)
            token = match.group(1) if match else None
        if not token:
            raise RuntimeError("first-run setup token was not emitted by the real server")
        password = secrets.token_urlsafe(24)
        docker_exec(container, "python3", "-c", API_SCRIPT,
                    env={"GATE_SETUP_TOKEN": token, "GATE_PASSWORD": password})
        log("first install active; initial setup and persisted network succeeded")

        # A successful repeat install exercises service stop, database backup, replacement and
        # readiness on the same systemd host while preserving operator configuration and data.
        install(container, "/root/hope-install.sh", "file:///root/hope-release", expect_success=True)
        second_pid = validate_running(container)
        validate_data(container)
        if second_pid == first_pid:
            raise RuntimeError("repeat installation did not replace the running service process")
        log("repeat install restarted the service and retained the network and schema")

        # The candidate checksum is correct but its executable exits immediately. The installer
        # must reject activation, restore the prior files/database, and start the old service again.
        install(container, "/root/hope-install.sh", "file:///root/hope-release-broken", expect_success=False)
        recovered_pid = validate_running(container)
        validate_data(container)
        journal_check = docker_exec(container, "sh", "-c",
                                    "test ! -e /var/lib/hope-activation/transaction/journal.json && test -f /var/lib/hope-activation/previous/hope.db",
                                    check=False)
        if journal_check.returncode:
            raise RuntimeError("failed activation did not leave a clean journal and retained recovery snapshot")
        log("failed replacement was rejected; the prior service and database recovered")

        install(container, "/root/hope-install.sh", "file:///root/hope-release", expect_success=True)
        final_pid = validate_running(container)
        validate_data(container)
        if final_pid == recovered_pid:
            raise RuntimeError("post-recovery install did not activate a fresh service process")
        log("post-recovery reinstall passed; Linux x86_64 systemd server gate is green")
    finally:
        run(["docker", "rm", "-f", container], check=False)


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.SubprocessError, RuntimeError, tarfile.TarError) as error:
        raise SystemExit(f"server-install gate failed: {error}")
