#!/usr/bin/env python3
"""Real-systemd gate for the agent updater.

The installer tests (tests/installer) drive the updater with stubbed service managers. This gate runs it
for real: a Debian 13 container with systemd as PID 1, the real server behind a TLS proxy using a
throwaway CA, a release signed with a throwaway key and built from this tree, the console's own
installation command, and the console's own "update all" request. See tests/gates/README.md.

    tests/gates/run.sh updater

Environment:
  HOPEGATE_UPDATER_IMAGE  image with systemd as PID 1, curl and openssl (default: built from updater.Dockerfile)
  HOPEGATE_WORK           working directory for keys, releases and logs (default: a new temporary directory)
  HOPEGATE_KEEP=1         keep the working directory after a successful run
  HOPEGATE_SKIP_BUILD=1   reuse the server and agents already built in HOPEGATE_WORK / the cargo target directory
"""
import contextlib
import hashlib
import http.client
import json
import os
import pathlib
import platform
import shutil
import ssl
import subprocess
import sys
import tempfile
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
CONTAINER = 'hopegate-updater'
TLS_PORT = int(os.environ.get('HOPEGATE_TLS_PORT', '18443'))
BACKEND_PORT = int(os.environ.get('HOPEGATE_BACKEND_PORT', '18080'))
SETUP_TOKEN = 'hopegate-setup-token-0123456789'
PASSWORD = 'hopegate-administrator-password'
# Dotted versions the agents are built as. 0.9.3 is signed by a key the host never saw; 0.9.4 is the
# release that cannot reach the server once installed.
V1, V2, V3, V4 = '0.9.1', '0.9.2', '0.9.3', '0.9.4'
STATE = '/var/lib/hope-agent'
UPDATER_DIR = '/var/lib/hope-agent-update'

machine = platform.machine()
ARCH = 'x86_64' if machine.lower() in ('x86_64', 'amd64') else None
TRIPLE = 'x86_64-unknown-linux-musl'


class Failure(Exception):
    pass


def log(*parts):
    print('[updater-gate]', *parts, flush=True)


def run(command, check=True, timeout=900, env=None, cwd=None):
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout, env=env, cwd=cwd)
    if check and result.returncode != 0:
        raise Failure(f'{" ".join(map(str, command))[:200]} exited {result.returncode}: {(result.stderr or result.stdout)[-1500:]}')
    return result


def expect(condition, message):
    if not condition:
        raise Failure(message)


def wait_for(what, check, timeout, interval=2.0):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        last = check()
        if last:
            return last
        time.sleep(interval)
    raise Failure(f'timed out after {timeout}s waiting for {what}')


class Gate:
    def __init__(self):
        expect(ARCH, f'unsupported CPU architecture {machine}')
        for tool in ('docker', 'openssl', 'cargo', 'sh'):
            expect(shutil.which(tool), f'{tool} is required')
        self.keep = os.environ.get('HOPEGATE_KEEP') == '1'
        self.work = pathlib.Path(os.environ.get('HOPEGATE_WORK') or tempfile.mkdtemp(prefix='hopegate-updater-')).resolve()
        expect(REPO not in self.work.parents and self.work != REPO, 'HOPEGATE_WORK must be outside the repository')
        self.work.mkdir(parents=True, exist_ok=True)
        self.root = self.work / 'releases'
        self.root.mkdir(exist_ok=True)
        self.server = None
        self.proxy = None
        self.cookie = ''
        self.cells = []
        self.broken = False

    # ------------------------------------------------------------------ build
    def build(self):
        log('building the server and four x86_64 Linux agent versions (cached builds are quick)')
        if os.environ.get('HOPEGATE_SKIP_BUILD') != '1':
            run(['cargo', 'build', '--locked', '-p', 'hope-server'], cwd=REPO, timeout=3000)
        metadata = json.loads(run(['cargo', 'metadata', '--format-version', '1', '--no-deps'], cwd=REPO).stdout)
        self.server_binary = pathlib.Path(metadata['target_directory']) / 'debug' / 'hope-server'
        expect(self.server_binary.exists(), f'{self.server_binary} was not built')
        for version in (V1, V2, V3, V4):
            out = self.work / 'agents' / version
            if os.environ.get('HOPEGATE_SKIP_BUILD') == '1' and (out / f'hope-agent-linux-{ARCH}').exists():
                continue
            env = dict(os.environ, HOPE_BUILD_VERSION=version)
            run(['sh', 'scripts/build-agent.sh', TRIPLE, '--out', str(out)], cwd=REPO, env=env, timeout=3000)

    def make_keys(self):
        for name in ('good', 'other'):
            path = self.work / f'{name}.pem'
            if not path.exists():
                run(['openssl', 'genrsa', '-out', str(path), '3072'])
        if (self.work / 'ca.pem').exists():
            return
        extensions = self.work / 'leaf.ext'
        extensions.write_text('basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n'
                              'subjectAltName=IP:127.0.0.1\nauthorityKeyIdentifier=keyid\nsubjectKeyIdentifier=hash\n')
        ca_extensions = self.work / 'ca.ext'
        ca_extensions.write_text('basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n')
        w = str(self.work)
        run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', f'{w}/ca.key', '-out', f'{w}/ca.pem', '-days', '2',
             '-subj', '/CN=hopegate updater CA', '-extensions', 'v3', '-config', self.ca_config(ca_extensions)])
        run(['openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-keyout', f'{w}/leaf.key', '-out', f'{w}/leaf.csr', '-subj', '/CN=127.0.0.1'])
        run(['openssl', 'x509', '-req', '-in', f'{w}/leaf.csr', '-CA', f'{w}/ca.pem', '-CAkey', f'{w}/ca.key', '-CAcreateserial',
             '-out', f'{w}/leaf.pem', '-days', '2', '-extfile', str(extensions)])

    def ca_config(self, extensions):
        config = self.work / 'ca.cnf'
        config.write_text('[req]\ndistinguished_name=dn\nprompt=no\n[dn]\nCN=hopegate updater CA\n[v3]\n' + extensions.read_text())
        return str(config)

    def publish(self, version, key, root=None, rotate=False):
        """Sign a built agent and publish it under the root the server serves."""
        staging = self.work / 'staging' / f'{version}-{key}'
        shutil.rmtree(staging, ignore_errors=True)
        shutil.copytree(self.work / 'agents' / version, staging)
        env = dict(os.environ, HOPE_SIGNING_KEY=str(self.work / f'{key}.pem'))
        command = ['sh', 'scripts/assemble-release.sh', '--staging', str(staging), '--version', version,
                   '--out', str(root or self.root)]
        # Key rotation requires a complete release. Linux x86_64 is the only supported target,
        # so this gate's single staged artifact is complete when exercising key rotation.
        command.append('--rotate-key' if rotate else '--allow-partial')
        run(command, cwd=REPO, env=env)
        return (root or self.root) / 'current'

    def manifest_sha(self, version_hint=None):
        manifest = json.loads((self.root / 'current' / 'manifest.json').read_text())
        for entry in manifest['artifacts']:
            if (entry['platform'], entry['arch']) == ('linux', ARCH):
                return manifest['version'], entry['sha256']
        raise Failure('the published manifest has no entry for this platform')

    # ------------------------------------------------------------------ processes
    def start_proxy(self, reject_agent_version=''):
        self.proxy = subprocess.Popen(
            [sys.executable, '-I', str(REPO / 'tests/agent/https_proxy.py'), '--certificate', str(self.work / 'leaf.pem'), '--key',
             str(self.work / 'leaf.key'), '--port', str(TLS_PORT), '--backend-port', str(BACKEND_PORT)]
            + (['--reject-agent-version', reject_agent_version] if reject_agent_version else []),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        wait_for('the TLS proxy', lambda: self.tls_up(), 20, 0.5)

    def stop_proxy(self):
        if self.proxy and self.proxy.poll() is None:
            self.proxy.terminate()
            self.proxy.wait(timeout=10)

    def tls_up(self):
        try:
            self.api('GET', '/healthz')
            return True
        except (OSError, ssl.SSLError, http.client.HTTPException):
            return False

    def start_server(self):
        env = dict(os.environ, HOPE_BIND=f'127.0.0.1:{BACKEND_PORT}', HOPE_SERVER_URL=f'https://127.0.0.1:{TLS_PORT}',
                   HOPE_SETUP_TOKEN=SETUP_TOKEN, HOPE_DATA_DIR=str(self.work / 'data'), HOPE_RELEASE_DIR=str(self.root),
                   HOPE_WEB_DIR=str(REPO / 'web' / 'out'), HOPE_DISCOVERY_MODE='limited', HOPE_AGENT_CA='/etc/hope/ca.crt',
                   RUST_LOG='hope_server=warn')
        (self.work / 'data').mkdir(exist_ok=True)
        self.server_log = open(self.work / 'server.log', 'w')
        self.server = subprocess.Popen([str(self.server_binary)], env=env, stdout=self.server_log, stderr=subprocess.STDOUT)
        wait_for('the server', lambda: self.backend_up(), 30, 0.5)

    def backend_up(self):
        try:
            connection = http.client.HTTPConnection('127.0.0.1', BACKEND_PORT, timeout=3)
            connection.request('GET', '/healthz')
            return connection.getresponse().status == 200
        except OSError:
            return False

    # ------------------------------------------------------------------ server API
    def api(self, method, path, body=None, cookie=None):
        context = ssl.create_default_context(cafile=str(self.work / 'ca.pem'))
        connection = http.client.HTTPSConnection('127.0.0.1', TLS_PORT, context=context, timeout=30)
        headers = {'Origin': f'https://127.0.0.1:{TLS_PORT}', 'Content-Type': 'application/json'}
        if cookie or self.cookie:
            headers['Cookie'] = cookie or self.cookie
        connection.request(method, path, json.dumps(body) if body is not None else None, headers)
        response = connection.getresponse()
        data = response.read()
        try:
            parsed = json.loads(data) if data else None
        except ValueError:
            parsed = data.decode('utf-8', 'replace')
        return response.status, parsed, response.getheader('Set-Cookie')

    def login(self):
        status, body, cookie = self.api('POST', '/api/v1/auth/setup', {'password': PASSWORD, 'setup_token': SETUP_TOKEN})
        expect(status == 200 and cookie, f'administrator setup failed: {status} {body}')
        self.cookie = cookie.split(';')[0]
        status, body, _ = self.api('PUT', '/api/v1/settings/reporting', {'interval_seconds': 5})
        expect(status == 200, f'could not shorten the report interval: {status} {body}')

    def host(self):
        status, body, _ = self.api('GET', '/api/v1/bootstrap')
        expect(status == 200, f'bootstrap failed: {status}')
        hosts = [h for h in body['hosts'] if h.get('agent_id')]
        return hosts[0] if hosts else None

    def collector(self, host, name='agent_update'):
        return next((c for c in host.get('collectors', []) if c['name'] == name), None)

    # ------------------------------------------------------------------ container
    def start_container(self):
        image = os.environ.get('HOPEGATE_UPDATER_IMAGE')
        if not image:
            image = 'hopegate-updater:ci'
            run(['docker', 'build', '-q', '-f', str(REPO / 'tests/gates/updater.Dockerfile'), '-t', image, str(REPO / 'tests/gates')], timeout=1800)
        run(['docker', 'rm', '-f', CONTAINER], check=False)
        # systemd as PID 1 needs the cgroup tree and a writable /run; /tmp must allow execution because
        # the installer runs the downloaded, verified agent from there. Host networking reaches the
        # server and proxy on 127.0.0.1; this is a disposable machine.
        run(['docker', 'run', '-d', '--name', CONTAINER, '--privileged', '--cgroupns=host', '--network', 'host', '--tmpfs', '/tmp:exec,mode=1777',
             '--tmpfs', '/run', '--tmpfs', '/run/lock', '-v', '/sys/fs/cgroup:/sys/fs/cgroup:rw', image])

        def booted():
            state = self.dx('systemctl is-system-running').stdout.strip()
            return state in ('running', 'degraded')
        wait_for('systemd to finish booting', booted, 90, 1)
        self.dx('mkdir -p /etc/hope')
        run(['docker', 'cp', str(self.work / 'ca.pem'), f'{CONTAINER}:/etc/hope/ca.crt'])

    def dx(self, script, user=None, timeout=300):
        command = ['docker', 'exec'] + (['-u', user] if user else []) + [CONTAINER, 'sh', '-c', script]
        return subprocess.run(command, capture_output=True, text=True, timeout=timeout)

    def must(self, script, user=None, timeout=300):
        result = self.dx(script, user, timeout)
        expect(result.returncode == 0, f'`{script[:120]}` exited {result.returncode}: {(result.stderr or result.stdout)[-1000:]}')
        return result.stdout.strip()

    def result_file(self):
        result = self.dx(f'cat {STATE}/update-result')
        if result.returncode != 0:
            return {}
        return dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)

    def main_pid(self):
        return self.must('systemctl show -p MainPID --value hope-agent')

    def installed_sha(self):
        return self.must('sha256sum /usr/local/bin/hope-agent').split()[0]

    def installed_version(self):
        return self.must('/usr/local/bin/hope-agent --version').split()[-1]

    def updater_journal(self):
        return self.dx('journalctl -u hope-agent-update.service --no-pager -o cat').stdout

    # ------------------------------------------------------------------ cells
    @contextlib.contextmanager
    def cell(self, name):
        started = time.time()
        if self.broken:
            self.cells.append((name, 'skipped', 'an earlier cell failed', 0))
            yield False
            return
        try:
            yield True
            self.cells.append((name, 'passed', '', time.time() - started))
            log('PASS', name)
        except (Failure, subprocess.TimeoutExpired, OSError) as error:
            self.cells.append((name, 'FAILED', str(error), time.time() - started))
            self.broken = True
            log('FAIL', name, '-', error)

    def request_all(self, expected=1):
        status, body, _ = self.api('POST', '/api/v1/agents/update')
        expect(status == 200 and body.get('requested') == expected, f'update all answered {status} {body}, expected {expected} request')

    def run_all(self):
        self.make_keys()
        self.build()
        self.publish(V1, 'good')
        self.start_server()
        self.start_proxy()
        self.login()
        self.start_container()

        with self.cell('install as root with the console command puts a root-owned updater in place') as go:
            if go:
                status, token, _ = self.api('POST', '/api/v1/enrollment-tokens', {})
                expect(status == 200, f'no enrollment token: {status} {token}')
                result = self.dx(token['command'], timeout=400)
                expect(result.returncode == 0, f'installation failed: {result.stderr[-1500:]}')
                expect('result=installed ' + V1 in result.stderr, 'the installer did not report result=installed')
                expect(self.must('systemctl is-active hope-agent') == 'active', 'the agent service is not active')
                expect(self.must('systemctl is-enabled hope-agent-update.path') == 'enabled', 'the update path unit is not enabled')
                expect(self.must('systemctl is-active hope-agent-update.path') == 'active', 'the update path unit is not active')
                expect(self.must(f'stat -c "%U:%G %a" {STATE}/update-queue') == 'hope-agent:hope-agent 700', 'the queue directory has the wrong owner or mode')
                expect(self.must('stat -c "%U %a" /usr/local/sbin/hope-agent-update') == 'root 755', 'the updater script is not root-owned 755')
                expect(self.must(f'stat -c "%U %a" {UPDATER_DIR}/install-agent.sh') == 'root 755', 'the pinned installer is not root-owned 755')
                writable = self.dx(f'test -w {UPDATER_DIR}/install-agent.sh -o -w {UPDATER_DIR} -o -w /usr/local/sbin/hope-agent-update', user='hope-agent')
                expect(writable.returncode != 0, 'the agent account can write to the updater')
                host = wait_for('the agent to report with an updater', lambda: self.reporting(V1), 90)
                expect(self.collector(host)['available'], 'the agent did not report a working updater')

        with self.cell('update all from the console updates the agent through the real path unit') as go:
            if go:
                self.publish(V2, 'good')
                before_pid = self.main_pid()
                self.request_all()
                host = wait_for(f'the agent to report {V2}', lambda: self.reporting(V2), 180)
                # The installer still waits for its first accepted report and writes the outcome after it.
                wait_for('the updater to finish', lambda: self.result_file().get('status') == 'updated'
                         and self.dx('systemctl is-active hope-agent-update.service').stdout.strip() == 'inactive', 120)
                version, sha = self.manifest_sha()
                expect(version == V2, 'the published release is not the one just signed')
                expect(self.installed_version() == V2, 'the installed binary is not the new version')
                expect(self.installed_sha() == sha, 'the installed binary differs from the signed manifest')
                pid = self.main_pid()
                expect(pid not in ('0', before_pid), 'the agent service was not restarted')
                expect(self.must(f'readlink /proc/{pid}/exe') == '/usr/local/bin/hope-agent', 'the running agent is not the installed binary')
                journal = self.updater_journal()
                expect(f'result=updated {V2}' in journal, 'the updater journal does not show the update')
                expect(self.must('systemctl show -p Result --value hope-agent-update.service') == 'success', 'the updater unit did not succeed')
                expect(self.must('systemctl is-active hope-agent-update.path') == 'active', 'the path unit stopped watching')
                expect(self.dx(f'test -e {STATE}/update-queue/request').returncode != 0, 'the request file was not removed')
                expect(self.dx('test -d /var/lib/hope-agent-install').returncode != 0, 'an install transaction was left behind')
                expect(self.result_file().get('status') == 'updated', 'update-result does not say updated')
                expect(host.get('update_target') is None, 'the server still holds the request')
                detail = wait_for('the agent to report the update', lambda: (self.collector(self.host() or {}) or {}).get('detail') == f'Updated to {V2}', 60)
                expect(detail, 'the agent did not report its update')

        with self.cell('a request from the agent account for a current release changes nothing') as go:
            if go:
                pid, sha = self.main_pid(), self.installed_sha()
                previous = int(self.result_file().get('at', '0'))
                self.must(f'echo requested > {STATE}/update-queue/request', user='hope-agent')
                wait_for('the updater to answer', lambda: int(self.result_file().get('at', '0')) > previous, 60)
                expect(self.result_file().get('status') == 'current', f'update-result says {self.result_file()}')
                expect(self.main_pid() == pid, 'the agent service was restarted although nothing was newer')
                expect(self.installed_sha() == sha, 'the installed binary changed')
                expect(self.dx(f'test -e {STATE}/update-queue/request').returncode != 0, 'the request file was not removed')

        with self.cell('a server that switches signing keys cannot push an update') as go:
            if go:
                # Publish the new key under this same release root. Repointing current outside
                # HOPE_RELEASE_DIR no longer represents a valid published release.
                self.publish(V3, 'other', rotate=True)
                pid, sha = self.main_pid(), self.installed_sha()
                previous = int(self.result_file().get('at', '0'))
                self.request_all()
                wait_for('the updater to refuse the release', lambda: int(self.result_file().get('at', '0')) > previous, 120)
                result = self.result_file()
                expect(result.get('status') == 'failed' and result.get('code') == 'MANIFEST_INVALID', f'update-result says {result}')
                expect(self.installed_version() == V2 and self.installed_sha() == sha, 'the installed agent changed')
                expect(self.main_pid() == pid and self.must('systemctl is-active hope-agent') == 'active', 'the agent service was disturbed')
                expect('error=MANIFEST_INVALID' in self.updater_journal(), 'the updater journal does not show why')
                wait_for('the failure to be reported', lambda: 'MANIFEST_INVALID' in ((self.collector(self.host() or {}) or {}).get('detail') or ''), 60)

        with self.cell('a signed release whose agent cannot report is rolled back') as go:
            if go:
                self.publish(V4, 'good', rotate=True)
                sha = self.installed_sha()
                previous = int(self.result_file().get('at', '0'))
                # The server answers 503 to reports from the new version only: the installed agent keeps
                # reporting (and learns of the request), the update installs, and the new agent never
                # gets an accepted report. The journal still holds the earlier update's phases.
                self.stop_proxy()
                self.start_proxy(reject_agent_version=V4)
                waits = self.updater_journal().count('phase=wait-report')
                self.request_all()
                wait_for('the updater to wait for the new agent', lambda: self.updater_journal().count('phase=wait-report') > waits, 180, 1)
                wait_for('the updater to give up and roll back', lambda: int(self.result_file().get('at', '0')) > previous, 300, 3)
                result = self.result_file()
                expect(result.get('status') == 'failed' and result.get('code') == 'FIRST_REPORT_TIMEOUT', f'update-result says {result}')
                journal = self.updater_journal()
                expect('phase=rollback' in journal, 'the updater journal shows no rollback')
                # A rollback restarts the agent. It must not ask for the same release again at once
                # (it used to, which retried a failing release in a loop).
                attempts = journal.count('phase=wait-report')
                time.sleep(30)
                expect(self.updater_journal().count('phase=wait-report') == attempts, 'the updater tried the failing release again right after rolling back')
                expect(self.installed_version() == V2 and self.installed_sha() == sha, 'the previous agent was not restored')
                expect(self.must('systemctl is-active hope-agent') == 'active', 'the previous agent is not running')
                expect(self.dx('test -d /var/lib/hope-agent-install').returncode != 0, 'an install transaction was left behind')
                wait_for('the restored agent to report again', lambda: self.reporting(V2), 90)
                wait_for('the failure to be reported', lambda: 'FIRST_REPORT_TIMEOUT' in ((self.collector(self.host() or {}) or {}).get('detail') or ''), 60)

        with self.cell('uninstalling with the pinned installer removes the updater') as go:
            if go:
                result = self.dx(f'sh {UPDATER_DIR}/install-agent.sh uninstall', timeout=120)
                expect(result.returncode == 0, f'uninstall failed: {result.stderr[-1000:]}')
                expect(self.dx('systemctl is-active hope-agent').stdout.strip() != 'active', 'the agent is still running')
                units = self.dx("systemctl list-unit-files 'hope-agent-update*' --no-legend").stdout.strip()
                expect(units == '', f'updater units remain: {units}')
                for path in ('/usr/local/sbin/hope-agent-update', UPDATER_DIR, '/usr/local/bin/hope-agent',
                             '/etc/systemd/system/hope-agent-update.path', '/etc/systemd/system/hope-agent-update.service'):
                    expect(self.dx(f'test -e {path}').returncode != 0, f'{path} was not removed')

    def reporting(self, version):
        host = self.host()
        if host and host.get('agent_version') == version and host.get('last_report') and time.time() - host['last_report'] < 30 and self.collector(host):
            return host
        return None

    # ------------------------------------------------------------------ teardown and report
    def down(self):
        self.stop_proxy()
        if self.server and self.server.poll() is None:
            self.server.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                self.server.wait(timeout=15)
        run(['docker', 'rm', '-f', CONTAINER], check=False)

    def diagnostics(self):
        log('--- diagnostics (the run failed) ---')
        for name, command in (('updater journal', 'journalctl -u hope-agent-update.service --no-pager -o cat | tail -40'),
                              ('agent journal', 'journalctl -u hope-agent --no-pager -o cat | tail -20'),
                              ('update-result', f'cat {STATE}/update-result')):
            print(f'## {name}\n' + (self.dx(command).stdout if self.container_up() else '(container gone)'))
        if (self.work / 'server.log').exists():
            print('## server log\n' + (self.work / 'server.log').read_text()[-1500:])

    def container_up(self):
        return run(['docker', 'inspect', '-f', '{{.State.Running}}', CONTAINER], check=False).stdout.strip() == 'true'

    def report(self):
        width = max(len(name) for name, *_ in self.cells)
        print()
        for name, outcome, detail, seconds in self.cells:
            print(f'{outcome:8} {name.ljust(width)}  {seconds:6.1f}s  {detail}')
        (self.work / 'results.json').write_text(json.dumps([dict(cell=n, outcome=o, detail=d, seconds=round(s, 1)) for n, o, d, s in self.cells], indent=2))
        return all(outcome == 'passed' for _, outcome, *_ in self.cells)


def main():
    gate = Gate()
    ok = False
    try:
        gate.run_all()
        ok = gate.report()
        if not ok:
            gate.diagnostics()
    finally:
        gate.down()
        if ok and not gate.keep:
            shutil.rmtree(gate.work, ignore_errors=True)
        else:
            log('working directory kept:', gate.work)
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
