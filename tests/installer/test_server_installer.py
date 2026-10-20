"""Tests for scripts/install-hope.sh (the server installer) inside a temporary root.

Package managers, systemd, journald, account tools, setcap and curl are stubs on a private PATH;
the release archive comes from a local directory through the HOPE_INSTALL_RELEASE_URL override, so
the download, checksum verification and extraction paths are the real ones. The release also carries
hope-agents.tar.gz (fake agent binaries with the right headers) and the package carries the real
scripts/assemble-release.sh behind a wrapper that logs its call, so agent signing is the real one too.
The operating system and service manager are stubbed; the filesystem, activation helper and signing are real:
python3 -I -m unittest discover -s tests/installer -v
"""
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
INSTALLER = REPO / 'scripts' / 'install-hope.sh'
ASSEMBLE = REPO / 'scripts' / 'assemble-release.sh'
PLATFORMS = ['linux-x86_64']
# Logs the signing call, then runs the real script from the package (its repository check sees the package).
ASSEMBLE_WRAPPER = """#!/bin/sh
printf 'assemble-release HOPE_SIGNING_KEY=%s %s\\n' "$HOPE_SIGNING_KEY" "$*" >> "$HOPE_INSTALL_ROOT/commands.log"
exec sh "$(dirname "$0")/assemble-release-real.sh" "$@"
"""


def agent_header(platform):
    """Smallest ELF/Mach-O header that carries the machine/cputype assemble-release.sh checks."""
    if platform.startswith('linux'):
        machine = b'\x3e\x00' if platform.endswith('x86_64') else b'\xb7\x00'
        return b'\x7fELF\x02\x01\x01\x00' + bytes(8) + b'\x02\x00' + machine
    cpu = b'\x07\x00\x00\x01' if platform.endswith('x86_64') else b'\x0c\x00\x00\x01'
    return b'\xcf\xfa\xed\xfe' + cpu + bytes(12)

STUB = r'''#!@PYTHON@
import os, shutil, sys
root = os.environ['HOPE_INSTALL_ROOT']
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
def log(line):
    with open(os.path.join(root, 'commands.log'), 'a') as handle:
        handle.write(line + '\n')
log(name + ' ' + ' '.join(args))
if name == 'uname':
    print('Linux' if '-s' in args else 'x86_64'); sys.exit(0)
if name == 'id':
    print('0'); sys.exit(0)
if name in ('apt-get', 'dnf', 'pacman', 'setcap', 'useradd', 'chown'):
    if name == 'useradd':
        open(os.path.join(root, 'user-created'), 'w').close()
    sys.exit(0)
if name == 'getent':
    sys.exit(0 if os.path.exists(os.path.join(root, 'user-created')) else 2)
if name == 'systemctl':
    if args[0] == 'show':
        running = os.path.exists(os.path.join(root, 'hope-running'))
        print(('123' if running else '0') if '--property=MainPID' in args else ('active' if running else 'inactive'))
        sys.exit(0)
    if args[0] == 'stop':
        try: os.unlink(os.path.join(root, 'hope-running'))
        except FileNotFoundError: pass
    if args[0] == 'is-enabled': sys.exit(0)
    if args[:2] == ['is-active', '--quiet']:
        sys.exit(0 if os.path.exists(os.path.join(root, 'hope-running')) else 3)
    if args and args[0] in ('start', 'restart') and 'hope' in args:
        binary = os.path.join(root, 'opt/hope/bin/hope-server')
        if os.environ.get('FAIL_ACTIVATION_START') and os.path.exists(binary) and 'fake hope-server' in open(binary).read():
            import sqlite3
            db = os.path.join(root, 'var/lib/hope/hope.db')
            with sqlite3.connect(db) as connection:
                connection.execute('PRAGMA user_version=99')
            sys.exit(1)
        open(os.path.join(root, 'hope-running'), 'w').close()
    if args[:2] == ['enable', '--now'] and 'hope' in args:
        open(os.path.join(root, 'hope-running'), 'w').close()
    if 'caddy' in args and args[0] in ('enable', 'reload', 'restart'):
        ca = os.path.join(root, 'var/lib/caddy/.local/share/caddy/pki/authorities/local')
        os.makedirs(ca, exist_ok=True)
        with open(os.path.join(ca, 'root.crt'), 'w') as handle:
            handle.write('-----BEGIN CERTIFICATE-----\nFAKE\n-----END CERTIFICATE-----\n')
    sys.exit(0)
if name == 'journalctl':
    if os.path.exists(os.path.join(root, 'hope-running')):
        print('hope_server: H.O.P.E first-run setup token: tok3n-ABCdef_123456789012345678')
    sys.exit(0)
if name == 'curl':
    url = [a for a in args if '://' in a][-1]
    out = args[args.index('-o') + 1] if '-o' in args else None
    if url.startswith('file://'):
        source = url[len('file://'):]
        if not os.path.exists(source):
            sys.exit(22)
        if out:
            shutil.copyfile(source, out)
        else:
            sys.stdout.write(open(source).read())
        sys.exit(0)
    if url.endswith('/healthz'):
        print('{"status":"ok","pid":123}')
        sys.exit(0 if os.path.exists(os.path.join(root, 'hope-running')) else 7)
    sys.exit(6)
sys.exit(1)
'''


class ServerInstaller(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix='hope-server-install-'))
        self.addCleanup(shutil.rmtree, self.root, True)
        self.bin = self.root / 'stubs'
        self.bin.mkdir()
        stub = STUB.replace('@PYTHON@', os.path.realpath(__import__('sys').executable))
        for name in ('id', 'apt-get', 'dnf', 'pacman', 'setcap', 'useradd', 'chown', 'getent', 'systemctl', 'journalctl', 'curl', 'uname'):
            path = self.bin / name
            path.write_text(stub)
            path.chmod(path.stat().st_mode | stat.S_IXUSR)
        (self.root / 'etc').mkdir()
        (self.root / 'etc/os-release').write_text('ID=debian\nVERSION_ID="13"\n')
        self.release = self.root / 'release'
        self.release.mkdir()
        self.make_release()

    def make_release(self, corrupt=False, version='0.9.0', agents=True, agent_version=None, corrupt_agents=False, nested=False):
        arch = 'x86_64'
        tree = self.root / 'tree'
        shutil.rmtree(tree, ignore_errors=True)
        for path in self.release.iterdir():
            path.unlink()
        (tree / 'web').mkdir(parents=True)
        (tree / 'tools').mkdir()
        (tree / 'hope-server').write_text('#!/bin/sh\necho fake hope-server\n')
        (tree / 'hope-server').chmod(0o755)
        (tree / 'web/index.html').write_text('<html>console</html>')
        (tree / 'VERSION').write_text(version + '\n')
        (tree / 'tools/assemble-release.sh').write_text(ASSEMBLE_WRAPPER)
        shutil.copyfile(ASSEMBLE, tree / 'tools/assemble-release-real.sh')
        shutil.copyfile(REPO / 'scripts/activate-server.py', tree / 'tools/activate-server.py')
        name = f'hope-server-{arch}-linux.tar.gz'
        with tarfile.open(self.release / name, 'w:gz') as archive:
            for entry in ('hope-server', 'web', 'VERSION', 'tools'):
                archive.add(tree / entry, arcname=entry)
        digest = hashlib.sha256((self.release / name).read_bytes()).hexdigest()
        if corrupt:
            digest = '0' * 64
        sums = f'{digest}  {name}\n'
        if agents:
            built = agent_version or version
            staged = tree / 'agents' / f'hope-agents-{built}'
            staged.mkdir(parents=True)
            # The workflows archive the directory's contents (tar -C dist/agents ... .); nested is a top-level directory.
            for platform in PLATFORMS:
                (staged / f'hope-agent-{platform}').write_bytes(agent_header(platform) + f'agent {platform} {built}'.encode())
                (staged / f'hope-agent-{platform}.info').write_text(f'version={built}\ntarget=t-{platform}\nmin_os=Test OS 1.0\ncommit=abcdef012345\n')
            with tarfile.open(self.release / 'hope-agents.tar.gz', 'w:gz') as archive:
                archive.add(staged, arcname=staged.name if nested else '.')
            agent_digest = hashlib.sha256((self.release / 'hope-agents.tar.gz').read_bytes()).hexdigest()
            sums += f'{"f" * 64 if corrupt_agents else agent_digest}  hope-agents.tar.gz\n'
        (self.release / 'SHA256SUMS-server').write_text(sums)

    def run_installer(self, **extra):
        env = {
            'PATH': f'{self.bin}:{os.environ["PATH"]}',
            'HOME': str(self.root),
            'TMPDIR': str(self.root),
            'HOPE_INSTALL_ROOT': str(self.root),
            'HOPE_INSTALL_NONINTERACTIVE': '1',
            'HOPE_INSTALL_RELEASE_URL': f'file://{self.release}',
            'HOPE_INSTALL_URL': 'https://192.168.1.50',
            'NO_COLOR': '1',
        }
        env.update(extra)
        return subprocess.run(['sh', str(INSTALLER)], env=env, capture_output=True, text=True, timeout=120, stdin=subprocess.DEVNULL)

    def commands(self):
        return (self.root / 'commands.log').read_text().splitlines()

    @property
    def key(self):
        return self.root / 'etc/hope/release-signing.pem'

    @property
    def releases(self):
        return self.root / 'opt/hope/releases'

    def manifest(self):
        return json.loads((self.releases / 'current/manifest.json').read_text())

    def public_key_of(self, private):
        return subprocess.run(['openssl', 'pkey', '-in', str(private), '-pubout'], check=True, capture_output=True, text=True).stdout

    def test_fresh_install_with_caddy_internal_ca(self):
        done = self.run_installer()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        binary = self.root / 'opt/hope/bin/hope-server'
        self.assertTrue(binary.exists() and binary.stat().st_mode & 0o111)
        self.assertTrue((self.root / 'opt/hope/web/index.html').exists())
        unit = (self.root / 'etc/systemd/system/hope.service').read_text()
        for line in ('User=hope', 'AmbientCapabilities=CAP_NET_RAW', 'CapabilityBoundingSet=CAP_NET_RAW', 'ProtectSystem=strict',
                     'EnvironmentFile=/etc/hope/hope.env', 'ExecStart=/opt/hope/bin/hope-server'):
            self.assertIn(line, unit)
        env = (self.root / 'etc/hope/hope.env').read_text()
        self.assertIn('HOPE_SERVER_URL=https://192.168.1.50\n', env)
        self.assertIn('HOPE_BIND=127.0.0.1:8080\n', env)
        self.assertIn('HOPE_AGENT_CA=/etc/hope/ca.crt\n', env, 'the internal CA path is advertised to agents')
        self.assertEqual((self.root / 'etc/hope/hope.env').stat().st_mode & 0o777, 0o640)
        caddy = (self.root / 'etc/caddy/Caddyfile').read_text()
        self.assertIn('192.168.1.50 {', caddy)
        self.assertIn('tls internal', caddy)
        self.assertIn('reverse_proxy 127.0.0.1:8080', caddy)
        self.assertIn('FAKE', (self.root / 'etc/hope/ca.crt').read_text())
        commands = self.commands()
        self.assertTrue(any(c.startswith('useradd --system') and 'hope' in c for c in commands))
        self.assertTrue((self.root / 'etc/systemd/system/hope.service.d/20-hope-activation.conf').exists())
        self.assertIn('systemctl daemon-reload', commands)
        self.assertIn('systemctl enable hope', commands)
        self.assertIn('systemctl start hope', commands, 'the CA path is staged before the initial start')
        self.assertTrue(any(c.startswith('apt-get install') and 'caddy' in c for c in commands))
        self.assertIn('Setup token:    tok3n-ABCdef_123456789012345678', done.stdout)
        self.assertIn('copy /etc/hope/ca.crt', done.stdout)

    def test_rerun_upgrades_binary_and_keeps_settings(self):
        self.assertEqual(self.run_installer().returncode, 0)
        env_path = self.root / 'etc/hope/hope.env'
        env_path.write_text(env_path.read_text().replace('HOPE_TRUSTED_PROXIES=', 'HOPE_TRUSTED_PROXIES=10.0.0.5') + 'CUSTOM=1\n')
        (self.root / 'opt/hope/bin/hope-server').write_text('old')
        unit_path = self.root / 'etc/systemd/system/hope.service'
        unit_path.write_text(unit_path.read_text() + '# retained operator setting\n')
        (self.root / 'commands.log').unlink()
        done = self.run_installer(HOPE_INSTALL_URL='https://other.example')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        env = env_path.read_text()
        self.assertIn('HOPE_TRUSTED_PROXIES=10.0.0.5\n', env)
        self.assertIn('CUSTOM=1\n', env)
        self.assertIn('HOPE_SERVER_URL=https://192.168.1.50\n', env, 'an existing URL is never overwritten silently')
        self.assertIn('fake hope-server', (self.root / 'opt/hope/bin/hope-server').read_text())
        self.assertIn('# retained operator setting', unit_path.read_text())
        commands = self.commands()
        self.assertFalse(any(c.startswith('useradd') for c in commands))
        self.assertIn('systemctl stop hope', commands)
        self.assertIn('systemctl start hope', commands)
        self.assertFalse(any(c.startswith('systemctl enable --now hope') for c in commands))

    def test_failed_activation_restores_the_pre_upgrade_database_and_published_agents(self):
        self.assertEqual(self.run_installer(HOPE_INSTALL_PROXY='none').returncode, 0)
        binary = self.root / 'opt/hope/bin/hope-server'
        binary.write_text('old server')
        database = self.root / 'var/lib/hope/hope.db'
        with closing(sqlite3.connect(database)) as connection:
            connection.executescript('CREATE TABLE kept(value); INSERT INTO kept VALUES(7); PRAGMA user_version=14;')
        env = (self.root / 'etc/hope/hope.env').read_bytes()
        unit = (self.root / 'etc/systemd/system/hope.service').read_bytes()
        release = os.readlink(self.releases / 'current')
        (self.root / 'commands.log').unlink()
        self.make_release(version='0.9.1')
        done = self.run_installer(HOPE_INSTALL_PROXY='none', FAIL_ACTIVATION_START='1')
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Previous installation restored', done.stderr)
        self.assertEqual(binary.read_text(), 'old server')
        self.assertEqual((self.root / 'etc/hope/hope.env').read_bytes(), env)
        self.assertEqual((self.root / 'etc/systemd/system/hope.service').read_bytes(), unit)
        with closing(sqlite3.connect(database)) as connection:
            self.assertEqual(connection.execute('PRAGMA user_version').fetchone(), (14,))
            self.assertEqual(connection.execute('SELECT * FROM kept').fetchall(), [(7,)])
        self.assertTrue((self.root / 'hope-running').exists())
        self.assertEqual(os.readlink(self.releases / 'current'), release)
        self.assertFalse(any(line.startswith('assemble-release ') for line in self.commands()))
        self.assertNotIn('H.O.P.E is installed.', done.stdout)

    def test_failed_first_activation_does_not_claim_success_or_publish_agents(self):
        done = self.run_installer(HOPE_INSTALL_PROXY='none', FAIL_ACTIVATION_START='1')
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((self.root / 'opt/hope/bin/hope-server').exists())
        self.assertFalse((self.root / 'var/lib/hope/hope.db').exists())
        self.assertFalse((self.root / 'etc/hope/hope.env').exists())
        self.assertFalse(self.key.exists())
        self.assertNotIn('H.O.P.E is installed.', done.stdout)

    def test_public_domain_uses_a_public_certificate_and_no_proxy_is_respected(self):
        done = self.run_installer(HOPE_INSTALL_URL='https://hope.example.com')
        self.assertEqual(done.returncode, 0, done.stderr)
        caddy = (self.root / 'etc/caddy/Caddyfile').read_text()
        self.assertIn('hope.example.com {', caddy)
        self.assertNotIn('tls internal', caddy)
        self.assertNotIn('HOPE_AGENT_CA=/etc/hope/ca.crt', (self.root / 'etc/hope/hope.env').read_text())
        shutil.rmtree(self.root / 'etc/caddy')
        shutil.rmtree(self.root / 'opt')
        (self.root / 'etc/hope/hope.env').unlink()
        (self.root / 'commands.log').unlink()
        done = self.run_installer(HOPE_INSTALL_PROXY='none', HOPE_INSTALL_URL='https://hope.lan')
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertFalse((self.root / 'etc/caddy').exists())
        self.assertIn('Point your reverse proxy at http://127.0.0.1:8080', done.stdout)
        self.assertFalse(any('caddy' in c for c in self.commands()))

    def test_checksum_mismatch_and_bad_url_change_nothing(self):
        self.make_release(corrupt=True)
        done = self.run_installer()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Checksum mismatch', done.stderr)
        self.assertFalse((self.root / 'opt/hope/bin/hope-server').exists())
        self.assertFalse((self.root / 'etc/systemd/system/hope.service').exists())
        self.make_release()
        done = self.run_installer(HOPE_INSTALL_URL='http://hope.lan')
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('must start with https://', done.stderr)
        self.assertFalse((self.root / 'opt').exists())

    def test_agents_are_signed_with_a_new_server_key(self):
        done = self.run_installer()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.key.stat().st_mode & 0o777, 0o600, 'created under umask 077')
        text = subprocess.run(['openssl', 'rsa', '-in', str(self.key), '-noout', '-text'], check=True, capture_output=True, text=True).stdout
        self.assertRegex(text, r'Private-Key: \(3072 bit')
        signing = [c for c in self.commands() if c.startswith('assemble-release ')]
        self.assertEqual(list(self.root.glob('hope-install.*')), [], 'the staging directory is removed afterwards')
        self.assertEqual(len(signing), 1, signing)
        self.assertRegex(signing[0], rf'^assemble-release HOPE_SIGNING_KEY={self.key} --staging \S+/hope-install\.\w+/agents '
                                     rf'--version 0\.9\.0 --out {self.releases}$')
        manifest = self.manifest()
        self.assertEqual((manifest['version'], manifest['partial']), ('0.9.0', False))
        self.assertEqual(sorted(f"{a['platform']}-{a['arch']}" for a in manifest['artifacts']), sorted(PLATFORMS))
        current = self.releases / 'current'
        self.assertEqual((current / 'release-public.pem').read_text(), self.public_key_of(self.key))
        subprocess.run(['openssl', 'dgst', '-sha256', '-verify', str(current / 'release-public.pem'), '-signature',
                        str(current / 'manifest.json.sig'), str(current / 'manifest.json')], check=True, capture_output=True)
        # The hope service account reads the release: every directory 755, every file 644.
        for path in [self.releases, current.resolve()]:
            self.assertEqual(path.stat().st_mode & 0o777, 0o755, path)
        for path in current.resolve().iterdir():
            self.assertEqual(path.stat().st_mode & 0o777, 0o644, path)
        self.assertIn('Agent releases: /opt/hope/releases   (0.9.0, signed with /etc/hope/release-signing.pem', done.stdout)
        self.assertNotIn('publish a signed release', done.stdout)

    def test_rerun_resigns_with_the_existing_key(self):
        self.assertEqual(self.run_installer().returncode, 0)
        key = self.key.read_bytes()
        first = (self.releases / 'current').resolve().name
        (self.root / 'commands.log').unlink()
        self.make_release(version='0.9.1', nested=True)
        done = self.run_installer()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.key.read_bytes(), key, 'the signing key is never replaced')
        self.assertNotIn('Creating the agent release signing key', done.stderr)
        self.assertEqual(self.manifest()['version'], '0.9.1')
        self.assertEqual((self.releases / 'current/release-public.pem').read_text(), self.public_key_of(self.key))
        self.assertEqual(os.readlink(self.releases / 'previous'), first)
        signing = [c for c in self.commands() if c.startswith('assemble-release ')]
        self.assertEqual(len(signing), 1, signing)
        self.assertIn(f'HOPE_SIGNING_KEY={self.key} ', signing[0])
        self.assertIn('/agents/hope-agents-0.9.1 --version 0.9.1 ', signing[0])
        self.assertIn('(0.9.1, signed with', done.stdout)

    def test_signing_failure_keeps_the_previous_release(self):
        self.assertEqual(self.run_installer().returncode, 0)
        current = os.readlink(self.releases / 'current')
        key = self.key.read_bytes()
        self.make_release(version='0.9.1', agent_version='0.9.2')
        done = self.run_installer()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn('Signing the agent release failed', done.stderr)
        self.assertIn('mixed versions refused', done.stderr)
        self.assertEqual(os.readlink(self.releases / 'current'), current)
        self.assertEqual(self.manifest()['version'], '0.9.0')
        self.assertEqual(self.key.read_bytes(), key)
        self.assertIn('fake hope-server', (self.root / 'opt/hope/bin/hope-server').read_text(), 'the server is still upgraded')
        self.assertIn('(previous signed release kept)', done.stdout)

    def test_a_release_without_agents_installs_the_server_and_creates_no_key(self):
        self.make_release(agents=False)
        done = self.run_installer()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn('hope-agents.tar.gz', done.stderr)
        self.assertFalse(self.key.exists())
        self.assertFalse((self.releases / 'current').exists())
        self.assertFalse(any(c.startswith('assemble-release ') for c in self.commands()))
        self.assertIn('(none signed: publish a signed release before adding agents)', done.stdout)

    def test_a_hand_published_release_is_left_alone(self):
        # Published earlier with a key kept elsewhere: no second key, no refused publish attempt.
        (self.releases / '0.8.0-000000000000').mkdir(parents=True)
        os.symlink('0.8.0-000000000000', self.releases / 'current')
        done = self.run_installer()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn('signed with a key outside /etc/hope', done.stderr)
        self.assertFalse(self.key.exists())
        self.assertFalse(any(c.startswith('assemble-release ') for c in self.commands()))
        self.assertEqual(os.readlink(self.releases / 'current'), '0.8.0-000000000000')
        self.assertIn('(previous signed release kept)', done.stdout)

    def test_agent_checksum_mismatch_changes_nothing(self):
        self.make_release(corrupt_agents=True)
        done = self.run_installer()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Checksum mismatch for hope-agents.tar.gz', done.stderr)
        self.assertFalse((self.root / 'opt').exists())
        self.assertFalse(self.key.exists())

    def test_release_mode_without_an_artifact_fails_clearly(self):
        for path in self.release.iterdir():
            path.unlink()
        done = self.run_installer(HOPE_INSTALL_SOURCE='release')
        self.assertNotEqual(done.returncode, 0)
        self.assertRegex(done.stderr, 'No published server release|Download failed')
        self.assertFalse((self.root / 'opt').exists())





class ProxmoxHandover(unittest.TestCase):
    """On a Proxmox host the installer runs ct/hope.sh (the community-scripts engine), nothing else."""

    def test_proxmox_host_runs_the_ct_script_and_creates_nothing_itself(self):
        root = Path(tempfile.mkdtemp(prefix='hope-proxmox-'))
        self.addCleanup(shutil.rmtree, root, True)
        bin_dir = root / 'stubs'
        bin_dir.mkdir()
        python = os.path.realpath(__import__('sys').executable)
        for name in ('pct', 'pvesh', 'id'):
            path = bin_dir / name
            path.write_text(f"#!{python}\nimport sys\nprint('0')\n")
            path.chmod(0o755)
        curl = bin_dir / 'curl'
        curl.write_text(f"#!{python}\nimport sys\nurl=[a for a in sys.argv if '://' in a][-1]\nopen({str(root / 'fetched.log')!r}, 'a').write(url + chr(10))\nprint('echo CT_SCRIPT_RAN')\n")
        curl.chmod(0o755)
        env = {'PATH': f'{bin_dir}:{os.environ["PATH"]}', 'HOME': str(root), 'HOPE_INSTALL_NONINTERACTIVE': '1', 'NO_COLOR': '1'}
        done = subprocess.run(['sh', str(INSTALLER)], env=env, capture_output=True, text=True, timeout=60, stdin=subprocess.DEVNULL)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn('CT_SCRIPT_RAN', done.stdout)
        self.assertEqual((root / 'fetched.log').read_text().strip(), 'https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh')
        self.assertNotIn('pct create', done.stdout + done.stderr)


if __name__ == '__main__':
    unittest.main()
