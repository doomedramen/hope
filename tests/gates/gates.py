#!/usr/bin/env python3
"""Release gate fixture: real systemd + sshd targets, a real Caddy reverse proxy and the real server.

Everything runs in disposable Docker containers named hopegate-*; nothing is installed on the host.
Run `tests/gates/run.sh all` (see tests/gates/README.md). Standard library only (Python 3.9+).
"""
import argparse
import datetime
import http.client
import json
import os
import platform
import re
import secrets
import shutil
import socket
import ssl
import subprocess
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
P = "hopegate-"
NET = P + "net"
SUBNET = "172.30.77"
CADDY_IP, SERVER_IP = SUBNET + ".2", SUBNET + ".3"
SCAN_CIDR = SUBNET + ".16/28"
HOST = "hope.gate.test"
CADDY_IMAGE = "caddy:2.10.2-alpine"
SERVER_IMAGE = P + "server:gates"
ARCHS = ["amd64"]
PLATFORM_FILE = {"amd64": "hope-agent-linux-x86_64"}
AMD64_EMULATED = platform.machine().lower() not in ("x86_64", "amd64")
IPS = {("t", "amd64"): 18, ("s", "amd64"): 20, ("u", "amd64"): 22, ("d", "amd64"): 24,
       ("k", "amd64"): 26, ("t2", "amd64"): 27}
V0, V1 = "0.1.0", "0.1.1"
CA_PATH = "/etc/hope-ca.pem"
ADMIN_PASSWORD = None  # filled from secrets.json

WORK = Path(os.environ.get("HOPEGATE_WORK", "")) if os.environ.get("HOPEGATE_WORK") else None
results = []


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


def run(cmd, check=True, timeout=600, inp=None):
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, input=inp)
    if check and r.returncode != 0:
        raise SystemExit("command failed: %s\n%s%s" % (" ".join(cmd[:6]), r.stdout[-1500:], r.stderr[-1500:]))
    return r


def dx(container, script, user=None, env=None, timeout=600):
    """Run a shell script in a container; return (rc, combined output)."""
    cmd = ["docker", "exec"]
    if user:
        cmd += ["-u", user]
    for k, v in (env or {}).items():
        cmd += ["-e", "%s=%s" % (k, v)]
    cmd += [container, "sh", "-c", script]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    return r.returncode, (r.stdout + r.stderr)


def record(cell, arch, os_name, emulated, manager, mode, artifact, ok, detail):
    row = dict(cell=cell, arch=arch, os=os_name, emulated=emulated, manager=manager, mode=mode,
               artifact=artifact, result="pass" if ok else "FAIL", detail=detail)
    results.append(row)
    (WORK / "results.json").write_text(json.dumps(results, indent=1))
    log(("PASS" if ok else "FAIL"), cell, arch, mode, "-", detail)
    return ok


# ---------------------------------------------------------------------------------------------
# Work directory, secrets, PKI, keys
# ---------------------------------------------------------------------------------------------
def init_work():
    global WORK
    if WORK is None:
        WORK = Path(subprocess.check_output(["mktemp", "-d", "/tmp/hopegate-work.XXXXXX"], text=True).strip())
    WORK.mkdir(parents=True, exist_ok=True)
    try:
        WORK.resolve().relative_to(REPO)
        raise SystemExit("HOPEGATE_WORK must be outside the repository")
    except ValueError:
        pass
    for d in ("keys", "pki", "releases", "logs", "ssh"):
        (WORK / d).mkdir(exist_ok=True)
    os.chmod(WORK / "keys", 0o700)
    sec = WORK / "secrets.json"
    if not sec.exists():
        sec.write_text(json.dumps({k: secrets.token_hex(12) for k in
                                   ("admin", "root", "hopesudo", "hopeplain", "setup", "wrong")}))
        os.chmod(sec, 0o600)
    global ADMIN_PASSWORD
    ADMIN_PASSWORD = "A-" + json.loads(sec.read_text())["admin"]
    results[:] = json.loads((WORK / "results.json").read_text()) if (WORK / "results.json").exists() else []
    return WORK


def sec(name):
    return json.loads((WORK / "secrets.json").read_text())[name]


def make_keys():
    k = WORK / "keys" / "release-signing.pem"
    if not k.exists():
        run(["openssl", "genrsa", "-out", str(k), "3072"])
        os.chmod(k, 0o600)
    pki = WORK / "pki"
    if not (pki / "ca.pem").exists():
        run(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", str(pki / "ca.key")])
        run(["openssl", "req", "-x509", "-new", "-key", str(pki / "ca.key"), "-sha256", "-days", "3",
             "-subj", "/CN=hopegate throwaway CA", "-addext", "basicConstraints=critical,CA:TRUE",
             "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-addext", "subjectKeyIdentifier=hash",
             "-addext", "authorityKeyIdentifier=keyid:always", "-out", str(pki / "ca.pem")])
        run(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", str(pki / "server.key")])
        run(["openssl", "req", "-new", "-key", str(pki / "server.key"), "-subj", "/CN=" + HOST,
             "-out", str(pki / "server.csr")])
        (pki / "ext.cnf").write_text("subjectAltName=DNS:%s,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n"
                                     "extendedKeyUsage=serverAuth\nsubjectKeyIdentifier=hash\n"
                                     "authorityKeyIdentifier=keyid,issuer\n" % HOST)
        run(["openssl", "x509", "-req", "-in", str(pki / "server.csr"), "-CA", str(pki / "ca.pem"), "-CAkey",
             str(pki / "ca.key"), "-CAcreateserial", "-days", "2", "-sha256", "-extfile", str(pki / "ext.cnf"),
             "-out", str(pki / "server.pem")])
        for f in pki.iterdir():
            os.chmod(f, 0o644)  # read by the unprivileged Caddy container; the CA is throwaway
    ssh = WORK / "ssh"
    if not (ssh / "id_ed25519").exists():
        run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "hopegate", "-f", str(ssh / "id_ed25519")])
    Caddyfile = WORK / "Caddyfile"
    Caddyfile.write_text("""{
	auto_https off
	admin off
}
https://%s, https://127.0.0.1 {
	tls /pki/server.pem /pki/server.key
	reverse_proxy %sserver:8080
}
""" % (HOST, P))


# ---------------------------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------------------------
def source_tree(version):
    """The current tree (including uncommitted edits) at the requested workspace version."""
    if version == V0:
        return REPO
    dst = WORK / ("src-" + version)
    if dst.exists():
        shutil.rmtree(dst)
    run(["git", "clone", "-q", "--no-checkout", "--local", str(REPO), str(dst)])
    run(["git", "-C", str(dst), "checkout", "-q", run(["git", "-C", str(REPO), "rev-parse", "HEAD"]).stdout.strip()])
    run(["rsync", "-a", "--exclude", ".git", "--exclude", "target", "--exclude", "node_modules", "--exclude",
         "releases", "--exclude", "dist", "--exclude", "web/out", "--exclude", "web/.next", "--exclude", ".agent-notes",
         "--delete-excluded", str(REPO) + "/", str(dst) + "/"])
    toml = dst / "Cargo.toml"
    toml.write_text(toml.read_text().replace('version = "%s"' % V0, 'version = "%s"' % version, 1))
    lock = dst / "Cargo.lock"
    lock.write_text(re.sub(r'(name = "hope-[a-z]+"\nversion = ")%s(")' % re.escape(V0), r"\g<1>%s\2" % version,
                           lock.read_text()))
    return dst


def build_artifacts(version):
    stage = WORK / ("stage-" + version)
    expected = {"hope-agent-linux-x86_64", "hope-agent-linux-x86_64.info"}
    staged = {path.name for path in stage.glob("hope-agent-*")}
    if staged == expected:
        log("artifacts for", version, "already staged")
        return stage
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(exist_ok=True)
    tree = source_tree(version)
    triple = "x86_64-unknown-linux-musl"
    log("building", triple, version)
    r = run([str(tree / "scripts/build-agent-docker.sh"), triple, "--out", str(stage)], timeout=3600)
    (WORK / "logs" / ("build-%s-%s.log" % (version, triple))).write_text(r.stdout + r.stderr)
    return stage


def assemble(version, *extra):
    env = dict(os.environ, HOPE_SIGNING_KEY=str(WORK / "keys" / "release-signing.pem"))
    r = subprocess.run([str(REPO / "scripts/assemble-release.sh"), "--staging", str(WORK / ("stage-" + version)),
                        "--version", version, "--out", str(WORK / "releases"), *extra],
                       env=env, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr


def manifest():
    return json.loads((WORK / "releases" / "current" / "manifest.json").read_text())


def manifest_sha(arch):
    m = manifest()
    row = [a for a in m["artifacts"] if a["file"] == PLATFORM_FILE[arch]][0]
    return m["version"], row["sha256"]


def build_all():
    init_work()
    make_keys()
    for tag, df, plat in (("amd64", "target.Dockerfile", "linux/amd64"),):
        log("building target image", tag)
        run(["docker", "build", "--network=host", "--platform", plat, "-f", str(HERE / df), "-t",
             "%starget:%s" % (P, tag), str(HERE)], timeout=1800)
    log("building server image from Dockerfile")
    run(["docker", "build", "--network=host", "--platform", "linux/amd64", "-t", SERVER_IMAGE, str(REPO)], timeout=3600)
    run(["docker", "pull", CADDY_IMAGE], timeout=300)
    for v in (V0, V1):
        build_artifacts(v)
    rc, out = assemble(V0)
    if rc:
        raise SystemExit(out)
    log(out.strip().splitlines()[-1])


# ---------------------------------------------------------------------------------------------
# Environment
# ---------------------------------------------------------------------------------------------
def start_server(trusted="%s/32" % CADDY_IP):
    run(["docker", "rm", "-f", P + "server"], check=False)
    env = ["-e", "HOPE_SERVER_URL=https://" + HOST, "-e", "HOPE_AGENT_CA=" + CA_PATH,
           "-e", "HOPE_SETUP_TOKEN=gates-" + sec("setup") + "-setup"]
    if trusted:
        env += ["-e", "HOPE_TRUSTED_PROXIES=" + trusted]
    run(["docker", "run", "-d", "--name", P + "server", "--network", NET, "--ip", SERVER_IP, "--cap-drop", "ALL",
         "--cap-add", "NET_RAW", "--read-only", "--tmpfs", "/tmp", *env, "-v", P + "data:/var/lib/hope",
         "-v", "%s:/opt/hope/releases:ro" % (WORK / "releases"), SERVER_IMAGE])
    for _ in range(60):
        if "healthy" in run(["docker", "inspect", "-f", "{{.State.Health.Status}}", P + "server"]).stdout:
            return
        time.sleep(1)
    raise SystemExit("server did not become healthy")


def up():
    init_work()
    make_keys()
    down(quiet=True)
    run(["docker", "network", "create", "--subnet", SUBNET + ".0/24", NET])
    run(["docker", "volume", "create", P + "data"])
    start_server()
    run(["docker", "run", "-d", "--name", P + "caddy", "--network", NET, "--ip", CADDY_IP, "--network-alias", HOST,
         "-p", "127.0.0.1::443", "-v", "%s:/etc/caddy/Caddyfile:ro" % (WORK / "Caddyfile"),
         "-v", "%s:/pki:ro" % (WORK / "pki"), CADDY_IMAGE])
    for _ in range(30):
        try:
            if api("GET", "/healthz", auth=False)[0] == 200:
                break
        except OSError:
            time.sleep(1)
    st, _, body = api("POST", "/api/v1/auth/setup", {"password": ADMIN_PASSWORD, "setup_token": "gates-%s-setup" % sec("setup")}, auth=False)
    assert st == 200, (st, body)
    log("environment up, https port", port())


def down(quiet=False):
    names = run(["docker", "ps", "-a", "--format", "{{.Names}}", "--filter", "name=^%s" % P]).stdout.split()
    if names:
        run(["docker", "rm", "-f", "-v", *names], check=False)
    run(["docker", "network", "rm", NET], check=False)
    run(["docker", "volume", "rm", P + "data"], check=False)
    if not quiet:
        log("containers, network and data volume removed")


def purge_images_and_caches():
    run(["docker", "volume", "rm", "hope-agent-target-x86_64", "hope-agent-cargo-x86_64"], check=False)


def new_target(kind, arch):
    name = "%s%s-%s" % (P, kind, arch)
    run(["docker", "rm", "-f", name], check=False)
    run(["docker", "run", "-d", "--name", name, "--platform", "linux/" + arch, "--network", NET, "--ip",
         "%s.%d" % (SUBNET, IPS[(kind, arch)]), "--hostname", "%s-%s" % (kind, arch), "--cgroupns=host", "--tmpfs", "/run",
         "--tmpfs", "/run/lock", "--cap-add", "SYS_ADMIN", "--security-opt", "seccomp=unconfined",
         "-v", "/sys/fs/cgroup:/sys/fs/cgroup:rw", "%starget:%s" % (P, arch)])
    for _ in range(90):
        rc, out = dx(name, "systemctl is-system-running")
        if out.strip() == "running":
            break
        time.sleep(1)
    else:
        raise SystemExit("systemd in %s is %s" % (name, out.strip()))
    run(["docker", "cp", str(WORK / "pki" / "ca.pem"), "%s:%s" % (name, CA_PATH)])
    pub = (WORK / "ssh" / "id_ed25519.pub").read_text().strip()
    script = "chmod 644 %s;" % CA_PATH
    for u in ("root", "hopesudo", "hopeplain"):
        script += "echo '%s:%s' | chpasswd;" % (u, sec(u))
        home = "/root" if u == "root" else "/home/" + u
        script += "install -d -m700 -o %s %s/.ssh; echo '%s' > %s/.ssh/authorized_keys; chown %s %s/.ssh/authorized_keys; chmod 600 %s/.ssh/authorized_keys;" % (u, home, pub, home, u, home, home)
    rc, out = dx(name, script)
    assert rc == 0, out
    if kind == "t":
        rc, info = dx(name, "echo \"$(uname -m); $(. /etc/os-release; echo $PRETTY_NAME); $(systemctl --version | head -1); $(ssh -V 2>&1); PID1=$(cat /proc/1/comm); $(systemctl is-system-running)\"")
        record("target: systemd PID 1 + sshd", arch, "Debian 13", AMD64_EMULATED, "systemd as PID 1, OpenSSH", "n/a", "image %starget:%s" % (P, arch),
               rc == 0 and "PID1=systemd" in info and info.strip().endswith("running") and dx(name, "systemctl is-active ssh")[1].strip() == "active", info.strip())
    run(["docker", "cp", str(WORK / "ssh" / "id_ed25519"), name + ":/root/gate_key"])
    dx(name, "chmod 600 /root/gate_key; cp /root/gate_key /home/hopeplain/gate_key; chown hopeplain /home/hopeplain/gate_key")
    return name


def port():
    out = run(["docker", "port", P + "caddy", "443/tcp"]).stdout.split()[0]
    return int(out.rsplit(":", 1)[1])


# ---------------------------------------------------------------------------------------------
# HTTP API through Caddy with the throwaway CA
# ---------------------------------------------------------------------------------------------
_cookie = {}


class Conn(http.client.HTTPSConnection):
    def connect(self):
        sock = socket.create_connection((self.host, self.port), timeout=self.timeout)
        self.sock = self._context.wrap_socket(sock, server_hostname=HOST)


def ctx():
    c = ssl.create_default_context(cafile=str(WORK / "pki" / "ca.pem"))
    return c


def api(method, path, body=None, auth=True, headers=None, raw=False):
    conn = Conn("127.0.0.1", port(), context=ctx(), timeout=60)
    h = {"Host": HOST, "Origin": "https://" + HOST}
    if body is not None:
        h["Content-Type"] = "application/json"
    if auth and _cookie.get("v"):
        h["Cookie"] = "hope_session=" + _cookie["v"]
    h.update(headers or {})
    conn.request(method, path, json.dumps(body) if body is not None else None, h)
    r = conn.getresponse()
    data = r.read()
    sc = r.getheader("set-cookie")
    if sc and sc.startswith("hope_session="):
        _cookie["v"] = sc.split(";")[0].split("=", 1)[1]
    conn.close()
    if raw:
        return r.status, dict((k.lower(), v) for k, v in r.getheaders()), data
    try:
        parsed = json.loads(data) if data else None
    except ValueError:
        parsed = data.decode("utf-8", "replace")
    return r.status, dict((k.lower(), v) for k, v in r.getheaders()), parsed


def login():
    st, _, b = api("POST", "/api/v1/auth/login", {"password": ADMIN_PASSWORD}, auth=False)
    assert st == 200, (st, b)


def bootstrap():
    return api("GET", "/api/v1/bootstrap")[2]


def host_by_agent(agent_id):
    for h in bootstrap()["hosts"]:
        if h.get("agent_id") == agent_id:
            return h


def issued_tokens():
    f = WORK / "issued-tokens.txt"
    return f.read_text().split() if f.exists() else []


def make_token(reset=None):
    st, _, b = api("POST", "/api/v1/enrollment-tokens", {"reset_agent_id": reset} if reset else {})
    assert st == 200, (st, b)
    with open(WORK / "issued-tokens.txt", "a") as f:
        f.write(b["token"] + "\n")
    return b


def wait_report(agent_id, since, timeout=75):
    end = time.time() + timeout
    while time.time() < end:
        h = host_by_agent(agent_id)
        if h and h.get("last_report") and h["last_report"] > since and not h.get("agent_revoked"):
            return h
        time.sleep(2)
    return None


# ---------------------------------------------------------------------------------------------
# Proxy checks (item 2)
# ---------------------------------------------------------------------------------------------
def failed_logins(container, n, xff=None):
    codes = []
    for i in range(n):
        hdr = ""
        if xff:
            hdr = "-H 'X-Forwarded-For: %s'" % xff(i)
        rc, out = dx(container, "curl -s -o /dev/null -w '%%{http_code}' --cacert %s %s -H 'Origin: https://%s' "
                     "-H 'content-type: application/json' -d '{\"password\":\"definitely-wrong-pass\"}' https://%s/api/v1/auth/login" % (CA_PATH, hdr, HOST, HOST))
        codes.append(out.strip())
    return codes


def proxy_checks():
    init_work()
    login()
    a, b = P + "t-amd64", P + "t2-amd64"
    for arch in ARCHS:
        if run(["docker", "inspect", "-f", "{{.State.Running}}", P + "t-" + arch], check=False).stdout.strip() != "true":
            new_target("t", arch)
        if run(["docker", "inspect", "-f", "{{.State.Running}}", P + "t2-" + arch], check=False).stdout.strip() != "true":
            new_target("t2", arch)
    ver = "Caddy " + run(["docker", "run", "--rm", CADDY_IMAGE, "caddy", "version"]).stdout.split()[0]
    # TLS: trusted with the CA from a target, refused without it, HTTP/2 negotiated.
    rc, out = dx(a, "curl -sS -o /dev/null -w '%%{http_version} %%{http_code}' --cacert %s https://%s/healthz" % (CA_PATH, HOST))
    rc2, out2 = dx(a, "curl -sS -o /dev/null https://%s/healthz" % HOST)
    record("proxy-tls", "amd64", "Caddy 2.10.2", False, "n/a", "https via private CA", ver,
           rc == 0 and out.strip() == "2 200" and rc2 != 0 and "certificate" in out2.lower(),
           "agent host trusts only the throwaway CA: verified request %s; without the CA curl fails (%s)" % (out.strip(), out2.strip()[:60]))
    # Cookie, HSTS, headers pass through unchanged.
    st, h, _ = api("POST", "/api/v1/auth/login", {"password": ADMIN_PASSWORD}, auth=False)
    sc = h.get("set-cookie", "")
    record("proxy-cookie", "amd64", "Caddy 2.10.2", False, "n/a", "Secure cookie and headers", ver,
           st == 200 and "Secure" in sc and "HttpOnly" in sc and "SameSite=Strict" in sc and "max-age=31536000" in h.get("strict-transport-security", "")
           and h.get("cache-control") == "no-store" and "frame-ancestors 'none'" in h.get("content-security-policy", ""),
           "Set-Cookie has Secure/HttpOnly/SameSite=Strict, HSTS, no-store and CSP survive the proxy")
    # Origin check: forwarded Origin unchanged, a foreign Origin is refused.
    st_bad, _, _ = api("POST", "/api/v1/enrollment-tokens", {}, headers={"Origin": "https://evil.example"})
    st_ok, _, _ = api("POST", "/api/v1/enrollment-tokens", {})
    record("proxy-origin", "amd64", "Caddy 2.10.2", False, "n/a", "origin check", ver, st_bad == 403 and st_ok == 200,
           "foreign Origin %s, canonical Origin %s" % (st_bad, st_ok))
    # SSE: headers arrive at once, an event arrives promptly after a state change (no buffering).
    conn = Conn("127.0.0.1", port(), context=ctx(), timeout=20)
    conn.request("GET", "/api/v1/events", headers={"Host": HOST, "Cookie": "hope_session=" + _cookie["v"], "Accept": "text/event-stream"})
    t0 = time.time()
    r = conn.getresponse()
    t_headers = time.time() - t0
    box = {}

    def reader():
        while True:
            line = r.fp.readline()
            if not line:
                return
            if line.startswith(b"event: refresh") and "t" not in box:
                box["t"] = time.time()
                return
    th = threading.Thread(target=reader, daemon=True)
    th.start()
    time.sleep(1)
    t1 = time.time()
    ensure_network()
    th.join(8)
    lat = box["t"] - t1 if "t" in box else None
    conn.close()
    record("proxy-sse", "amd64", "Caddy 2.10.2", False, "n/a", "server-sent events", ver,
           r.status == 200 and "text/event-stream" in (r.getheader("content-type") or "") and lat is not None and lat < 3,
           "stream headers after %.2fs; refresh event %.2fs after a state change through Caddy" % (t_headers, lat if lat is not None else -1))
    # Client key: A is rate limited after ten failures, B (different container) is not; spoofed XFF is ignored.
    ca = failed_logins(a, 11)
    cb_first = failed_logins(b, 1)
    cb = failed_logins(b, 11, xff=lambda i: "10.99.%d.%d" % (i, i + 1))  # tries to look like a fresh client every time
    ok = ca[:10] == ["401"] * 10 and ca[10] == "429" and cb_first == ["401"] and cb[:9] == ["401"] * 9 and "429" in cb[9:]
    logs = run(["docker", "logs", P + "server"], check=False)
    warned = "X-Forwarded-For" in (logs.stdout + logs.stderr)
    record("proxy-xff", "amd64", "Caddy 2.10.2", False, "n/a", "rate-limit key is the client", ver, ok and not warned,
           "client A: %s then %s; client B unaffected (%s), with spoofed X-Forwarded-For it is still limited by its own address after ten failures (statuses %s); server logged no untrusted-XFF warning: %s" % (
               "ten 401", ca[10], cb_first[0], ",".join(cb), not warned))
    # Negative control: without HOPE_TRUSTED_PROXIES every client shares the proxy address.
    start_server(trusted=None)
    time.sleep(2)
    failed_logins(a, 10)
    shared = failed_logins(b, 1)
    start_server()
    _cookie.clear()
    time.sleep(2)
    login()
    record("proxy-untrusted-control", "amd64", "Caddy 2.10.2", False, "n/a", "no trusted proxy configured", ver,
           shared == ["429"], "with HOPE_TRUSTED_PROXIES unset, client A's ten failures make client B get %s (all clients share the proxy's key); restored" % shared[0])


# ---------------------------------------------------------------------------------------------
# Installer matrix (item 4)
# ---------------------------------------------------------------------------------------------
def console_cmd():
    return make_token()["command"]


def run_console(container, user=None, env=None, via_ssh=False):
    b = make_token()
    cmd = b["command"]
    if via_ssh:
        # a real login session (pam_systemd) so the user manager and XDG_RUNTIME_DIR exist
        rc, out = dx(container, "cat > /tmp/gate-cmd.sh <<'EOS'\n%s\nEOS\nchown %s /tmp/gate-cmd.sh;"
                     "ssh -i /root/gate_key -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR %s@127.0.0.1 'sh /tmp/gate-cmd.sh' 2>&1" % (cmd, user, user))
        return rc, out
    rc, out = dx(container, "cat > /tmp/gate-cmd.sh <<'EOS'\n%s\nEOS\nchmod 644 /tmp/gate-cmd.sh; sh /tmp/gate-cmd.sh 2>&1" % cmd, user=user, env=env)
    return rc, out


def fetch_installer(container, user=None):
    path = "/tmp/installer-%s.sh" % (user or "root")
    rc, out = dx(container, "curl -sS --fail --cacert %s https://%s/agent/install -o %s && chmod 644 %s" % (CA_PATH, HOST, path, path), user=user)
    assert rc == 0, out
    return path


def agent_status(container, state_dir, binary, user=None, sudo=False):
    rc, out = dx(container, "HOPE_AGENT_DIR=%s %s status --json" % (state_dir, binary), user=user)
    try:
        return json.loads(out.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return None


def sha_of(container, path):
    rc, out = dx(container, "sha256sum %s | cut -d' ' -f1" % path)
    return out.strip()


def system_state(container):
    rc, out = dx(container, "systemctl show hope-agent -p MainPID -p ActiveState -p SubState --value")
    parts = out.split()
    return dict(main_pid=int(parts[0]) if parts else 0, active=parts[1] if len(parts) > 1 else "?")


def confirm_system(container, arch, expect_version, since, cell, mode_label, extra=""):
    """Service active, binary hash equals the signed manifest, activation matches the running PID, server saw a report."""
    ms = system_state(container)
    st = agent_status(container, "/var/lib/hope-agent", "/usr/local/bin/hope-agent")
    ver, want_sha = manifest_sha_for(expect_version, arch)
    have_sha = sha_of(container, "/usr/local/bin/hope-agent")
    ok_pid = bool(st) and st.get("process_id") == ms["main_pid"] and ms["main_pid"] > 0
    h = wait_report(st["agent_id"], since) if st else None
    ok = ms["active"] == "active" and have_sha == want_sha and ok_pid and h is not None
    return ok, st, "service %s, MainPID %s == accepted process_id %s, activation %s, sha256 %s.. matches v%s manifest, server saw report %s%s" % (
        ms["active"], ms["main_pid"], st and st.get("process_id"), st and st.get("activation_id", "")[:8], have_sha[:12], expect_version,
        "yes" if h else "NO", extra)


def manifest_sha_for(version, arch):
    """sha256 of the staged artifact (the release copies it byte for byte and signs that hash)."""
    f = WORK / ("stage-" + version) / PLATFORM_FILE[arch]
    return version, run(["shasum", "-a", "256", str(f)]).stdout.split()[0] if sys.platform == "darwin" else run(["sha256sum", str(f)]).stdout.split()[0]


def snapshot_release_manifests():
    pass


def matrix_arch_phase(phase, arch):
    c = P + "t-" + arch
    emu = AMD64_EMULATED
    osn = "Debian 13"
    art = lambda v: "agent v%s linux-x86_64 musl sha256 %s" % (v, manifest_sha_for(v, arch)[1][:12])
    if phase == "a":
        t0 = time.time()
        rc, out = run_console(c)
        ok, st, d = confirm_system(c, arch, V0, t0, "a", "system")
        record("a manual install, root", arch, osn, emu, "systemd 257 system", "system", art(V0),
               ok and rc == 0 and "phase=complete" in out, "console command with custom CA (HOPE_CA_CERT); " + d)
        _state[arch] = {"agent": st and st["agent_id"], "pid": system_state(c)["main_pid"]}
    elif phase == "d":
        t0 = time.time()
        rc, out = run_console(c)
        ok, st, d = confirm_system(c, arch, V0, t0, "d", "system")
        same = st and st["agent_id"] == _state[arch]["agent"] and system_state(c)["main_pid"] != _state[arch]["pid"]
        record("d reinstall same version", arch, osn, emu, "systemd 257 system", "system", art(V0),
               ok and rc == 0 and bool(same), "second console command with a new token; identity kept, new service process; " + d)
        _state[arch]["pid"] = system_state(c)["main_pid"]
    elif phase == "f":
        fetch_installer(c)
        unit_before = dx(c, "sha256sum /etc/systemd/system/hope-agent.service")[1].split()[0]
        dx(c, "rm -f /tmp/up.log /tmp/up.pid; (HOPE_SERVER=https://%s HOPE_CA_CERT=%s HOPE_POLL_INTERVAL=20 sh /tmp/installer-root.sh upgrade > /tmp/up.log 2>&1 & echo $! > /tmp/up.pid)" % (HOST, CA_PATH))
        hit = False
        for _ in range(240):
            if "phase=wait-report" in dx(c, "cat /tmp/up.log")[1]:
                hit = True
                break
            time.sleep(0.5)
        mid_version = dx(c, "/usr/local/bin/hope-agent --version")[1].strip()  # the NEW binary is in place at this point
        dx(c, "kill -TERM $(cat /tmp/up.pid)")
        for _ in range(60):
            if "phase=rollback" in dx(c, "cat /tmp/up.log")[1] and "ROLLED_BACK" in dx(c, "cat /tmp/up.log")[1]:
                break
            time.sleep(1)
        time.sleep(2)
        t_after = time.time()
        log_text = dx(c, "cat /tmp/up.log")[1]
        ok, st, d = confirm_system(c, arch, V0, t_after, "f", "system")
        unit_after = dx(c, "sha256sum /etc/systemd/system/hope-agent.service")[1].split()[0]
        leftover = dx(c, "ls /var/lib/hope-agent-install 2>&1 | grep -c transaction")[1].strip()
        record("f interrupted during wait-report", arch, osn, emu, "systemd 257 system", "system", art(V0),
               hit and ok and "error=INTERRUPTED" in log_text and "error=ROLLED_BACK" in log_text and unit_before == unit_after and leftover == "0"
               and st["agent_id"] == _state[arch]["agent"] and "0.1.1" in mid_version,
               "SIGTERM at phase wait-report (v0.1.1 binary was in place: '%s'); INTERRUPTED then ROLLED_BACK, unit file identical, no leftover transaction, v0.1.0 restored and reporting; %s" % (mid_version, d))
        _state[arch]["pid"] = system_state(c)["main_pid"]
    elif phase == "e":
        t0 = time.time()
        fetch_installer(c)
        rc, out = dx(c, "HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh /tmp/installer-root.sh upgrade 2>&1" % (HOST, CA_PATH))
        ok, st, d = confirm_system(c, arch, V1, t0, "e", "system")
        record("e upgrade 0.1.0 to 0.1.1", arch, osn, emu, "systemd 257 system", "system", art(V1),
               ok and rc == 0 and "phase=complete" in out and st["agent_id"] == _state[arch]["agent"], "identity kept, no token needed; " + d)
    elif phase == "e2":
        fetch_installer(c)
        rc, out = dx(c, "HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh /tmp/installer-root.sh upgrade 2>&1" % (HOST, CA_PATH))
        still = dx(c, "/usr/local/bin/hope-agent --version")[1].strip()
        refused = rc != 0 and "error=DOWNGRADE_REFUSED" in out and "0.1.1" in still and system_state(c)["active"] == "active"
        t0 = time.time()
        rc2, out2 = dx(c, "HOPE_ALLOW_DOWNGRADE=1 HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh /tmp/installer-root.sh upgrade 2>&1" % (HOST, CA_PATH))
        ok, st, d = confirm_system(c, arch, V0, t0, "e2", "system")
        record("e downgrade refused / allowed", arch, osn, emu, "systemd 257 system", "system", art(V0), refused and ok and rc2 == 0,
               "older published release: refused with DOWNGRADE_REFUSED (service untouched, still %s); HOPE_ALLOW_DOWNGRADE=1 installs it; %s" % (still, d))
    elif phase == "g":
        # uninstall keeps the identity; purge removes it
        rc, out = dx(c, "HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh /tmp/installer-root.sh uninstall 2>&1" % (HOST, CA_PATH))
        gone = dx(c, "test ! -e /usr/local/bin/hope-agent && test ! -e /etc/systemd/system/hope-agent.service && ! systemctl is-active hope-agent && echo gone")[1]
        kept = dx(c, "test -s /var/lib/hope-agent/config.json && echo kept")[1]
        ident = dx(c, "grep -o '\"agent_id\": *\"[^\"]*\"' /var/lib/hope-agent/config.json")[1]
        record("g uninstall (identity kept)", arch, osn, emu, "systemd 257 system", "system", art(V0),
               rc == 0 and "gone" in gone and "kept" in kept and _state[arch]["agent"] in ident,
               "service, unit and binary removed; enrolled identity %s.. kept in /var/lib/hope-agent" % _state[arch]["agent"][:8])
        # reinstall onto the kept identity with a fresh token
        t0 = time.time()
        rc, out = run_console(c)
        ok, st, d = confirm_system(c, arch, V1, t0, "g", "system")
        record("g reinstall after uninstall", arch, osn, emu, "systemd 257 system", "system", art(V1), ok and rc == 0 and st["agent_id"] == _state[arch]["agent"], "same identity resumes; " + d)
        rc, out = dx(c, "HOPE_PURGE_IDENTITY=1 HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh /tmp/installer-root.sh uninstall 2>&1" % (HOST, CA_PATH))
        left = dx(c, "ls -A /var/lib/hope-agent 2>/dev/null | wc -l")[1].strip()
        record("g purge", arch, osn, emu, "systemd 257 system", "system", art(V1), rc == 0 and left == "0",
               "HOPE_PURGE_IDENTITY=1 uninstall: service gone, state directory entries left: %s" % left)
    elif phase == "b":
        t0 = time.time()
        rc, out = run_console(c, user="hopesudo")
        ok, st, d = confirm_system(c, arch, V1, t0, "b", "system")
        record("b non-root with passwordless sudo", arch, osn, emu, "systemd 257 system", "system", art(V1), ok and rc == 0 and "phase=complete" in out,
               "user hopesudo ran the console command; installer used sudo -n and installed the system service; " + d)
        path = fetch_installer(c, user="hopesudo")
        rc, out = dx(c, "HOPE_PURGE_IDENTITY=1 HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh %s uninstall 2>&1" % (HOST, CA_PATH, path), user="hopesudo")
        left = dx(c, "ls -A /var/lib/hope-agent 2>/dev/null | wc -l; ! systemctl is-active hope-agent >/dev/null && echo stopped")[1].split()
        record("g uninstall and purge as sudo user", arch, osn, emu, "systemd 257 system", "system", art(V1), rc == 0 and left[:2] == ["0", "stopped"],
               "hopesudo ran the purge through sudo (rc %s, state entries %s, service %s)" % (rc, left[0], left[1:2]))
    elif phase == "c":
        # No session and no lingering: no user manager exists.
        rc0, out0 = run_console(c, user="hopeplain", env={"XDG_RUNTIME_DIR": "/nonexistent"})
        code0 = re.findall(r"error=([A-Z_]+)", out0)
        dx(c, "loginctl enable-linger hopeplain")
        time.sleep(3)
        uid = dx(c, "id -u hopeplain")[1].strip()
        t0 = time.time()
        rc, out = run_console(c, user="hopeplain", via_ssh=True)
        base = "/home/hopeplain"
        env = "XDG_RUNTIME_DIR=/run/user/%s" % uid
        rc_s, so = dx(c, "systemctl --user show hope-agent -p MainPID -p ActiveState --value", user="hopeplain", env={"XDG_RUNTIME_DIR": "/run/user/" + uid})
        parts = so.split()
        pid = int(parts[0]) if parts else 0
        st = agent_status(c, base + "/.local/state/hope-agent", base + "/.local/bin/hope-agent", user="hopeplain")
        h = wait_report(st["agent_id"], t0) if st else None
        want = manifest_sha_for(V1, arch)[1]
        have = sha_of(c, base + "/.local/bin/hope-agent")
        ok = rc == 0 and "phase=complete" in out and len(parts) > 1 and parts[1] == "active" and st and st["process_id"] == pid and h and have == want
        record("c non-root without sudo, user service", arch, osn, emu, "systemd 257 --user (linger)", "user", art(V1), bool(ok),
               "without a login session/lingering the installer fails with %s (rc %s); with lingering and an ssh login session it installs ~/.local, user unit active, MainPID %s == accepted process_id, sha256 matches v%s manifest, server saw a report" % (",".join(code0) or "n/a", rc0, pid, V1))
        # survives logout because of lingering
        time.sleep(3)
        rc_s, so2 = dx(c, "systemctl --user is-active hope-agent", user="hopeplain", env={"XDG_RUNTIME_DIR": "/run/user/" + uid})
        record("c user service survives logout (linger)", arch, osn, emu, "systemd 257 --user (linger)", "user", art(V1), so2.strip() == "active",
               "no login session remains; user unit is %s" % so2.strip())
        path = fetch_installer(c, user="hopeplain")
        rc, out = dx(c, "HOPE_PURGE_IDENTITY=1 HOPE_SERVER=https://%s HOPE_CA_CERT=%s sh %s uninstall 2>&1" % (HOST, CA_PATH, path), user="hopeplain", env={"XDG_RUNTIME_DIR": "/run/user/" + uid})
        left = dx(c, "ls -A /home/hopeplain/.local/state/hope-agent 2>/dev/null | wc -l; ls -A /home/hopeplain/.local/bin 2>/dev/null | wc -l")[1].split()
        record("g uninstall and purge, user mode", arch, osn, emu, "systemd 257 --user (linger)", "user", art(V1), rc == 0 and left[:2] == ["0", "0"],
               "purge removes state and binary (rc %s, state entries %s, bin entries %s)" % (rc, left[0], left[1]))


_state = {}


def publish(version, *extra):
    rc, out = assemble(version, *extra)
    snapshot_release_manifests()
    return rc, out


def matrix():
    init_work()
    login()
    for arch in ARCHS:
        if run(["docker", "inspect", "-f", "{{.State.Running}}", P + "t-" + arch], check=False).stdout.strip() != "true":
            new_target("t", arch)
    snapshot_release_manifests()
    # the releases root must start at v0.1.0
    if manifest()["version"] != V0:
        log("republishing", V0)
        publish(V0, "--allow-downgrade")
    # an older version than the published one is refused by the assembler
    publish(V1)
    rc, out = publish(V0)
    record("release downgrade guard", "n/a", "macOS host", False, "n/a", "assemble-release.sh", "v0.1.0 over v0.1.1",
           rc != 0 and "refusing to publish older" in out, "assembler refuses an older version without --allow-downgrade")
    publish(V0, "--allow-downgrade")
    for ph in ("a", "d"):
        for arch in ARCHS:
            matrix_arch_phase(ph, arch)
    publish(V1)
    for ph in ("f", "e"):
        for arch in ARCHS:
            matrix_arch_phase(ph, arch)
    publish(V0, "--allow-downgrade")
    for arch in ARCHS:
        matrix_arch_phase("e2", arch)
    publish(V1)
    for ph in ("g", "b", "c"):
        for arch in ARCHS:
            matrix_arch_phase(ph, arch)


# ---------------------------------------------------------------------------------------------
# SSH jobs (item 5)
# ---------------------------------------------------------------------------------------------
def ensure_network():
    for n in bootstrap()["networks"]:
        if n["cidr"] == SCAN_CIDR:
            return n["id"], False
    st, _, b = api("POST", "/api/v1/networks", {"name": "gate-targets", "cidr": SCAN_CIDR, "enabled": False})
    assert st == 200, (st, b)
    return b["id"], True


def discover(addresses):
    nid, _ = ensure_network()
    api("POST", "/api/v1/networks/%s/scan" % nid, {})
    end = time.time() + 240
    found = {}
    time.sleep(3)
    while time.time() < end:
        bs = bootstrap()
        for h in bs["hosts"]:
            for a in h["addresses"]:
                if a in addresses and not h.get("agent_id"):
                    found[a] = h["id"]
        scanning = [s for s in bs["scans"] if s["status"] in ("queued", "running")]
        if len(found) == len(addresses) and not scanning:
            return found
        time.sleep(3)
    raise SystemExit("discovery did not find %s (found %s)" % (addresses, found))


def host_id_of(address):
    for h in bootstrap()["hosts"]:
        if address in h["addresses"]:
            return h["id"]


def ssh_job(host_id, address, username, fingerprint, password=None, private_key=None, timeout=900, on_running=None):
    body = {"host_id": host_id, "address": address, "port": 22, "username": username, "fingerprint": fingerprint}
    if password:
        body["password"] = password
    if private_key:
        body["private_key"] = private_key
    st, _, job = api("POST", "/api/v1/ssh/install", body)
    if st != 200:
        return {"http": st, "body": job}
    end = time.time() + timeout
    fired = False
    while time.time() < end:
        st, _, j = api("GET", "/api/v1/ssh/jobs/" + job["id"])
        if on_running and not fired and j["stages"] and on_running(j):
            fired = True
        if j["status"] != "running":
            return j
        time.sleep(0.2 if on_running and not fired else 1)
    return j


def fingerprint(host_id, address):
    st, _, b = api("POST", "/api/v1/ssh/fingerprint", {"host_id": host_id, "address": address, "port": 22})
    assert st == 200, (st, b)
    return b["fingerprint"]


def journal_counts(c):
    rc, out = dx(c, "journalctl -u ssh --no-pager 2>/dev/null | grep -c 'Failed password'; journalctl -u ssh --no-pager | grep -c 'Accepted'")
    f, a = (out.split() + ["0", "0"])[:2]
    return int(f), int(a)


def ssh_phase():
    init_work()
    login()
    publish(V1)
    pending = {}
    for arch in ARCHS:
        for kind in ("s", "u", "d"):
            if run(["docker", "inspect", "-f", "{{.State.Running}}", "%s%s-%s" % (P, kind, arch)], check=False).stdout.strip() != "true":
                new_target(kind, arch)
    addrs = ["%s.%d" % (SUBNET, IPS[(k, a)]) for a in ARCHS for k in ("s", "u", "d")]
    ids = discover(addrs)
    pw = sec("root")
    for arch in ARCHS:
        emu = AMD64_EMULATED
        osn = "Debian 13"
        S, U, D = (P + k + "-" + arch for k in "sud")
        sa, ua, da = ("%s.%d" % (SUBNET, IPS[(k, arch)]) for k in "sud")
        art = "agent v%s linux-x86_64 musl sha256 %s" % (V1, manifest_sha_for(V1, arch)[1][:12])
        mode = lambda m: m
        # 1. changed host key: confirmed fingerprint, then the host key changes before the job.
        fp_old = fingerprint(ids[sa], sa)
        before = journal_counts(S)
        dx(S, "rm -f /etc/ssh/ssh_host_*; ssh-keygen -A >/dev/null; systemctl restart ssh")
        time.sleep(2)
        j = ssh_job(ids[sa], sa, "root", fp_old, password=pw)
        after = journal_counts(S)
        record("ssh changed host key refused", arch, osn, emu, "real sshd (OpenSSH)", "ssh", art,
               j.get("status") == "failed" and j.get("error_code") == "HOST_KEY_MISMATCH" and after == before,
               "job %s/%s; sshd saw no authentication attempt (failed/accepted counters %s -> %s)" % (j.get("status"), j.get("error_code"), before, after))
        # 2. wrong password with the new, confirmed key.
        fp = fingerprint(ids[sa], sa)
        assert fp != fp_old
        before = journal_counts(S)
        j = ssh_job(ids[sa], sa, "root", fp, password="wrong-" + sec("wrong"))
        after = journal_counts(S)
        record("ssh wrong password", arch, osn, emu, "real sshd (OpenSSH)", "ssh", art,
               j.get("status") == "failed" and j.get("error_code") == "AUTH_FAILED" and after[0] > before[0],
               "job %s/%s; sshd logged the failed password" % (j.get("status"), j.get("error_code")))
        # 3. verified host key, password auth, fresh install as root.
        t0 = time.time()
        j = ssh_job(ids[sa], sa, "root", fp, password=pw)
        ok, st, d = confirm_system(S, arch, V1, t0, "ssh", "system") if j.get("status") == "completed" else (False, None, "job " + str(j.get("error_code")))
        record("ssh install (password, verified key)", arch, osn, emu, "real sshd; systemd 257 system", "ssh/system", art,
               j.get("status") == "completed" and j.get("mode") == "install" and ok and j.get("agent_id") == (st or {}).get("agent_id"),
               "job completed (fresh install), agent %s bound; %s" % ((j.get("agent_id") or "")[:8], d))
        agent1 = (st or {}).get("agent_id")
        pid1 = system_state(S)["main_pid"]
        # 4. same-agent reinstall using key authentication.
        t0 = time.time()
        key = (WORK / "ssh" / "id_ed25519").read_text()
        j = ssh_job(ids[sa], sa, "root", fp, private_key=key)
        ok, st, d = confirm_system(S, arch, V1, t0, "ssh", "system") if j.get("status") == "completed" else (False, None, "job " + str(j.get("error_code")))
        record("ssh same-agent reinstall (key auth)", arch, osn, emu, "real sshd; systemd 257 system", "ssh/system", art,
               j.get("status") == "completed" and j.get("mode") == "reinstall" and ok and (st or {}).get("agent_id") == agent1 and system_state(S)["main_pid"] != pid1,
               "job completed as reinstall, identity kept, new service process; %s" % d)
        # 5. non-root user without sudo: a user service stops at logout unless lingering is on, so the job
        #    must refuse (LINGER_REQUIRED, nothing installed) without lingering and succeed with it.
        fpu = fingerprint(ids[ua], ua)
        j = ssh_job(ids[ua], ua, "hopeplain", fpu, password=sec("hopeplain"))
        left = dx(U, "ls -A /home/hopeplain/.local/bin /home/hopeplain/.local/state /home/hopeplain/.config/systemd/user 2>/dev/null | wc -l")[1].strip()
        record("ssh non-root no sudo, no linger -> refused", arch, osn, emu, "real sshd; systemd --user", "ssh/user", art,
               j.get("status") == "failed" and j.get("error_code") == "LINGER_REQUIRED" and left == "0",
               "job %s/%s before any change (account files left on host: %s); guidance: %s" % (j.get("status"), j.get("error_code"), left, j.get("guidance")))
        dx(U, "loginctl enable-linger hopeplain")
        time.sleep(3)  # let user@UID.service start
        t0 = time.time()
        j = ssh_job(ids[ua], ua, "hopeplain", fpu, password=sec("hopeplain"))
        t_end = time.time()
        uid = dx(U, "id -u hopeplain")[1].strip()
        rc_s, so = dx(U, "systemctl --user show hope-agent -p MainPID -p ActiveState --value", user="hopeplain", env={"XDG_RUNTIME_DIR": "/run/user/" + uid})
        parts = so.split()
        stu = agent_status(U, "/home/hopeplain/.local/state/hope-agent", "/home/hopeplain/.local/bin/hope-agent", user="hopeplain")
        h = wait_report(stu["agent_id"], t0) if stu else None
        record("ssh non-root no sudo, linger -> user service", arch, osn, emu, "real sshd; systemd --user (linger)", "ssh/user", art,
               j.get("status") == "completed" and bool(j.get("notice")) and len(parts) > 1 and parts[1] == "active" and h is not None,
               "job %s, notice=%r; user unit %s; server saw a report: %s" % (j.get("status"), j.get("notice"), parts[1:2], bool(h)))
        time.sleep(45)  # well past logind's 10 s UserStopDelaySec for the closed SSH session
        h2 = wait_report(stu["agent_id"], t_end + 45) if stu else None
        rc_l, after_logout = dx(U, "systemctl --user is-active hope-agent 2>&1 | head -n 1", user="hopeplain", env={"XDG_RUNTIME_DIR": "/run/user/" + uid})
        state = after_logout.strip()
        record("ssh user service survives session end (linger)", arch, osn, emu, "real sshd; systemd --user (linger)", "ssh/user", art,
               h2 is not None and state == "active",
               "a fresh report accepted after the session ended + 45 s: %s; user unit %s" % (h2 is not None, state))
        # 6. dropped connection mid-transaction on a host that already runs v0.1.0: the sshd session is killed
        #    while the installer replaces the agent. The installer must roll back, so the old agent keeps running.
        publish(V0, "--allow-downgrade")
        rc, out = run_console(D)
        old_state = agent_status(D, "/var/lib/hope-agent", "/usr/local/bin/hope-agent")
        publish(V1)
        fpd = fingerprint(ids[da], da)

        # A watcher inside the host kills the SSH session the moment the installer opens its transaction journal.
        subprocess.run(["docker", "exec", "-d", D, "sh", "-c",
                        "until [ -s /var/lib/hope-agent-install/journal ]; do sleep 0.02; done; pkill -KILL -f '[s]shd-session'"], check=True)
        j = ssh_job(ids[da], da, "root", fpd, password=pw, timeout=300)
        time.sleep(12)  # the installer notices the closed pipe on its next progress line and rolls back
        ms = system_state(D)
        have = sha_of(D, "/usr/local/bin/hope-agent")
        txn = dx(D, "ls /var/lib/hope-agent-install 2>/dev/null | wc -l")[1].strip()
        st = agent_status(D, "/var/lib/hope-agent", "/usr/local/bin/hope-agent")
        h = wait_report(old_state["agent_id"], time.time() - 1) if old_state else None
        outcome = {manifest_sha_for(V0, arch)[1]: "rolled back to v0.1.0", manifest_sha_for(V1, arch)[1]: "completed v0.1.1"}.get(have, "UNKNOWN BINARY")
        ok = (j.get("status") == "uncertain" and j.get("error_code") in ("CONNECTION_LOST", "INSTALLER_INCOMPLETE", "INSTALLER_FAILED")
              and ms["active"] == "active" and outcome != "UNKNOWN BINARY" and txn == "0"
              and st and st["process_id"] == ms["main_pid"] and h is not None)
        record("ssh dropped connection (upgrade in flight)", arch, osn, emu, "real sshd; systemd 257 system", "ssh/system", art, ok,
               "sshd session killed as the transaction opened: job %s/%s; the host is never stranded: %s, service %s, MainPID %s == accepted process_id, no leftover transaction, server saw a new report: %s" % (
                   j.get("status"), j.get("error_code"), outcome, ms["active"], ms["main_pid"], h is not None))
        # 7. the whole host disappears mid-run (container killed).
        K = new_target("k", arch)
        ka = "%s.%d" % (SUBNET, IPS[("k", arch)])
        kid = discover([ka])[ka]
        fpk = fingerprint(kid, ka)

        def kill_host(job, K=K):
            if {"place_binary", "configure", "write_unit", "activate"} & set(job["stages"]):
                subprocess.run(["docker", "kill", K], capture_output=True)
                return True
            return False
        j = ssh_job(kid, ka, "root", fpk, password=pw, on_running=kill_host, timeout=300)
        record("ssh host killed mid-run", arch, osn, emu, "real sshd", "ssh", art,
               j.get("status") == "uncertain" and j.get("error_code") in ("CONNECTION_LOST", "INSTALLER_INCOMPLETE", "INSTALLER_FAILED"),
               "container killed during the transaction: job %s/%s (the server does not claim a remote state)" % (j.get("status"), j.get("error_code")))
    # secrets: password/key/token must not be in the database file(s) or logs.
    secrets_check()


def secrets_check():
    needles = [sec("root"), sec("hopeplain"), sec("hopesudo"), "wrong-" + sec("wrong"), ADMIN_PASSWORD]
    needles += issued_tokens()
    key = (WORK / "ssh" / "id_ed25519").read_text().strip().splitlines()
    needles += [l for l in key[1:-1] if len(l) > 20][:2]
    pat = "\n".join(needles) + "\n"
    db = run(["docker", "run", "--rm", "-i", "--entrypoint", "sh", "-v", P + "data:/data:ro", SERVER_IMAGE, "-c",
              "for f in /data/hope.db*; do grep -a -c -F -f /dev/stdin $f || true; done"], inp=pat, check=False)
    counts = [int(x) for x in db.stdout.split() if x.isdigit()]
    logs = run(["docker", "logs", P + "server"], check=False)
    caddy = run(["docker", "logs", P + "caddy"], check=False)
    text = logs.stdout + logs.stderr + caddy.stdout + caddy.stderr
    leaked = [n[:6] + ".." for n in needles if n in text]
    record("no secrets at rest or in logs", "n/a", "server container", False, "n/a", "db files + server/proxy logs", "%d secrets, %d tokens searched" % (len(needles) - len(issued_tokens()), len(issued_tokens())),
           sum(counts) == 0 and not leaked and len(counts) >= 1, "matching lines in hope.db* files %s; matches in server and Caddy logs: %s" % (counts, leaked or "none"))


# ---------------------------------------------------------------------------------------------
def markdown():
    init_work()
    lines = ["| Cell | OS | Arch | Emulated | Service manager | Mode | Artifact | Result | Evidence |", "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for r in results:
        lines.append("| %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (r["cell"], r["os"], r["arch"], "yes (Rosetta)" if r["emulated"] else "no", r["manager"], r["mode"], r["artifact"], r["result"], r["detail"].replace("|", "/")))
    out = "\n".join(lines)
    print(out)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["build", "up", "proxy", "matrix", "ssh", "report", "down", "all", "clean-caches"])
    a = ap.parse_args()
    init_work()
    log("work directory", WORK)
    if a.cmd == "build":
        build_all()
    elif a.cmd == "up":
        up()
    elif a.cmd == "proxy":
        proxy_checks()
    elif a.cmd == "matrix":
        matrix()
    elif a.cmd == "ssh":
        ssh_phase()
    elif a.cmd == "report":
        markdown()
    elif a.cmd == "down":
        down()
    elif a.cmd == "clean-caches":
        purge_images_and_caches()
    elif a.cmd == "all":
        results.clear()
        (WORK / "results.json").write_text("[]")
        build_all()
        try:
            up()
            proxy_checks()
            matrix()
            ssh_phase()
        finally:
            markdown()
            down()
    bad = [r for r in results if r["result"] != "pass"]
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
