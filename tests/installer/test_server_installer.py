"""Tests for scripts/install-hope.sh (the server installer) inside a temporary root.

Package managers, systemd, journald, account tools, setcap and curl are stubs on a private PATH;
the release archive comes from a local directory through the HOPE_INSTALL_RELEASE_URL override, so
the download, checksum verification and extraction paths are the real ones.
Run: python3 -I -m unittest discover -s tests/installer -v
"""
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
INSTALLER = REPO / 'scripts' / 'install-hope.sh'

STUB = r'''#!@PYTHON@
import os, shutil, sys
root = os.environ['HOPE_INSTALL_ROOT']
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
def log(line):
    with open(os.path.join(root, 'commands.log'), 'a') as handle:
        handle.write(line + '\n')
log(name + ' ' + ' '.join(args))
if name == 'id':
    print('0'); sys.exit(0)
if name in ('apt-get', 'dnf', 'pacman', 'setcap', 'useradd', 'chown'):
    if name == 'useradd':
        open(os.path.join(root, 'user-created'), 'w').close()
    sys.exit(0)
if name == 'getent':
    sys.exit(0 if os.path.exists(os.path.join(root, 'user-created')) else 2)
if name == 'systemctl':
    if args[:2] == ['is-active', '--quiet']:
        sys.exit(0 if os.path.exists(os.path.join(root, 'hope-running')) else 3)
    if args and args[0] in ('start', 'restart') and 'hope' in args:
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
        for name in ('id', 'apt-get', 'dnf', 'pacman', 'setcap', 'useradd', 'chown', 'getent', 'systemctl', 'journalctl', 'curl'):
            path = self.bin / name
            path.write_text(stub)
            path.chmod(path.stat().st_mode | stat.S_IXUSR)
        (self.root / 'etc').mkdir()
        (self.root / 'etc/os-release').write_text('ID=debian\nVERSION_ID="13"\n')
        self.release = self.root / 'release'
        self.release.mkdir()
        self.make_release()

    def make_release(self, corrupt=False):
        arch = {'x86_64': 'x86_64', 'amd64': 'x86_64', 'aarch64': 'aarch64', 'arm64': 'aarch64'}[os.uname().machine]
        tree = self.root / 'tree'
        shutil.rmtree(tree, ignore_errors=True)
        (tree / 'web').mkdir(parents=True)
        (tree / 'hope-server').write_text('#!/bin/sh\necho fake hope-server\n')
        (tree / 'hope-server').chmod(0o755)
        (tree / 'web/index.html').write_text('<html>console</html>')
        name = f'hope-server-{arch}-linux.tar.gz'
        with tarfile.open(self.release / name, 'w:gz') as archive:
            archive.add(tree / 'hope-server', arcname='hope-server')
            archive.add(tree / 'web', arcname='web')
        digest = hashlib.sha256((self.release / name).read_bytes()).hexdigest()
        if corrupt:
            digest = '0' * 64
        (self.release / 'SHA256SUMS-server').write_text(f'{digest}  {name}\n')

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
        self.assertTrue(any(c.startswith('setcap cap_net_raw=ep') for c in commands))
        self.assertIn('systemctl daemon-reload', commands)
        self.assertTrue(any(c.startswith('systemctl enable --now hope') for c in commands))
        self.assertIn('systemctl restart hope', commands, 'restarted after the CA path was set')
        self.assertTrue(any(c.startswith('apt-get install') and 'caddy' in c for c in commands))
        self.assertIn('Setup token:    tok3n-ABCdef_123456789012345678', done.stdout)
        self.assertIn('copy /etc/hope/ca.crt', done.stdout)

    def test_rerun_upgrades_binary_and_keeps_settings(self):
        self.assertEqual(self.run_installer().returncode, 0)
        env_path = self.root / 'etc/hope/hope.env'
        env_path.write_text(env_path.read_text().replace('HOPE_TRUSTED_PROXIES=', 'HOPE_TRUSTED_PROXIES=10.0.0.5') + 'CUSTOM=1\n')
        (self.root / 'opt/hope/bin/hope-server').write_text('old')
        (self.root / 'commands.log').unlink()
        done = self.run_installer(HOPE_INSTALL_URL='https://other.example')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        env = env_path.read_text()
        self.assertIn('HOPE_TRUSTED_PROXIES=10.0.0.5\n', env)
        self.assertIn('CUSTOM=1\n', env)
        self.assertIn('HOPE_SERVER_URL=https://192.168.1.50\n', env, 'an existing URL is never overwritten silently')
        self.assertIn('fake hope-server', (self.root / 'opt/hope/bin/hope-server').read_text())
        commands = self.commands()
        self.assertFalse(any(c.startswith('useradd') for c in commands))
        self.assertIn('systemctl restart hope', commands)
        self.assertFalse(any(c.startswith('systemctl enable --now hope') for c in commands))

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
