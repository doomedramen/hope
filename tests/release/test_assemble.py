"""scripts/assemble-release.sh policy tests with fake binaries and throwaway keys.
Run: python3 -I -m unittest discover -s tests/release -v"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / 'scripts' / 'assemble-release.sh'
PLATFORMS = ['linux-x86_64']
COMMIT = 'abcdef012345'


def header(platform):
    """Smallest header that carries the machine/cputype the script checks."""
    if platform.startswith('linux'):
        machine = b'\x3e\x00' if platform.endswith('x86_64') else b'\xb7\x00'
        return b'\x7fELF\x02\x01\x01\x00' + bytes(8) + b'\x02\x00' + machine
    cpu = b'\x07\x00\x00\x01' if platform.endswith('x86_64') else b'\x0c\x00\x00\x01'
    return b'\xcf\xfa\xed\xfe' + cpu + bytes(12)


class Assemble(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.keys = Path(tempfile.mkdtemp(prefix='hope-release-keys-'))
        for name in ('a', 'b'):
            subprocess.run(['openssl', 'genrsa', '-out', str(cls.keys / f'{name}.pem'), '2048'], check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.keys, ignore_errors=True)

    def setUp(self):
        self.base = Path(tempfile.mkdtemp(prefix='hope-release-'))
        self.addCleanup(shutil.rmtree, self.base, True)
        self.stage = self.base / 'stage'
        self.out = self.base / 'releases'
        self.stage.mkdir()

    def stage_all(self, version='1.0.0', platforms=PLATFORMS, commit=COMMIT):
        for platform in platforms:
            (self.stage / f'hope-agent-{platform}').write_bytes(header(platform) + f'binary {platform} {version}'.encode())
            (self.stage / f'hope-agent-{platform}.info').write_text(f'version={version}\ntarget=t-{platform}\nmin_os=Test OS 1.0\ncommit={commit}\n')

    def run_script(self, *args, key='a', version='1.0.0', stage=None, cwd=None):
        env = dict(os.environ, HOPE_SIGNING_KEY=str(self.keys / f'{key}.pem'))
        return subprocess.run(['sh', str(SCRIPT), '--staging', str(stage or self.stage), '--version', version, '--out', str(self.out), *args],
                              env=env, capture_output=True, text=True, timeout=120, cwd=cwd)

    def manifest(self, directory=None):
        return json.loads((directory or self.out / 'current').joinpath('manifest.json').read_text())

    def releases(self):
        if not self.out.exists():
            return []
        return sorted(p.name for p in self.out.iterdir() if p.is_dir() and not p.is_symlink())

    def test_complete_release_is_signed_and_verifiable(self):
        self.stage_all()
        done = self.run_script()
        self.assertEqual(0, done.returncode, done.stderr)
        manifest = self.manifest()
        self.assertEqual((1, '1.0.0', False), (manifest['format'], manifest['version'], manifest['partial']))
        self.assertEqual(PLATFORMS, [f"{a['platform']}-{a['arch']}" for a in manifest['artifacts']])
        self.assertTrue((self.out / 'current').is_symlink())
        self.assertEqual(COMMIT, manifest['commit'])
        self.assertEqual('t-linux-x86_64', manifest['artifacts'][0]['target'])
        pem = self.out / 'current' / 'release-public.pem'
        for name in ['manifest.json'] + [a['file'] for a in manifest['artifacts']]:
            subprocess.run(['openssl', 'dgst', '-sha256', '-verify', str(pem), '-signature', str(self.out / 'current' / (name + '.sig')), str(self.out / 'current' / name)], check=True, capture_output=True)
        self.assertEqual('Test OS 1.0', manifest['artifacts'][0]['min_os'])
        self.assertEqual([p.name for p in self.out.iterdir() if p.name.startswith('.')], [])

    def test_the_installer_template_beside_the_script_is_shipped_and_signed(self):
        self.stage_all()
        done = self.run_script()
        self.assertEqual(0, done.returncode, done.stderr)
        current = self.out / 'current'
        shipped = current / 'install-agent.sh'
        self.assertEqual((REPO / 'scripts/install-agent.sh').read_bytes(), shipped.read_bytes())
        subprocess.run(['openssl', 'dgst', '-sha256', '-verify', str(current / 'release-public.pem'), '-signature', str(shipped) + '.sig', str(shipped)], check=True, capture_output=True)

    def test_a_copy_without_an_installer_template_still_publishes(self):
        tools = self.base / 'tools'
        tools.mkdir()
        shutil.copyfile(SCRIPT, tools / 'assemble-release.sh')
        self.stage_all()
        env = dict(os.environ, HOPE_SIGNING_KEY=str(self.keys / 'a.pem'))
        done = subprocess.run(['sh', str(tools / 'assemble-release.sh'), '--staging', str(self.stage), '--version', '1.0.0', '--out', str(self.out)],
                              env=env, capture_output=True, text=True, timeout=120)
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn('installed updaters keep the installer they have', done.stdout)
        self.assertFalse((self.out / 'current' / 'install-agent.sh').exists())

    def test_missing_supported_target_is_refused(self):
        refused = self.run_script()
        self.assertNotEqual(0, refused.returncode)
        self.assertIn('linux-x86_64', refused.stderr)
        self.assertEqual([], self.releases())
        partial = self.run_script('--allow-partial')
        self.assertNotEqual(0, partial.returncode)
        self.assertEqual([], self.releases())

    def test_unsupported_architecture_artifacts_are_refused(self):
        self.stage_all()
        platform = 'linux-aarch64'
        (self.stage / f'hope-agent-{platform}').write_bytes(header(platform) + b'unsupported')
        (self.stage / f'hope-agent-{platform}.info').write_text(f'version=1.0.0\ntarget=t-{platform}\nmin_os=Test OS 1.0\ncommit={COMMIT}\n')
        done = self.run_script()
        self.assertNotEqual(0, done.returncode)
        self.assertIn('only Linux x86_64 is supported', done.stderr)

    def test_mixed_versions_are_refused(self):
        self.stage_all()
        (self.stage / 'hope-agent-linux-x86_64.info').write_text('version=0.9.0\nmin_os=x\n')
        done = self.run_script()
        self.assertNotEqual(0, done.returncode)
        self.assertIn('mixed versions', done.stderr)

    def in_repo_key(self, name):
        inside = REPO / 'dist' / name
        inside.parent.mkdir(exist_ok=True)
        self.addCleanup(lambda: next(inside.parent.iterdir(), None) or inside.parent.rmdir())
        shutil.copy(self.keys / 'a.pem', inside)
        self.addCleanup(lambda: inside.exists() and inside.unlink())
        return inside

    def test_key_inside_the_repository_is_refused(self):
        self.stage_all()
        inside = self.in_repo_key('release-test-key.pem')
        for spelling in (inside, Path(str(inside).upper().replace('/USERS', '/Users', 1))):
            env = dict(os.environ, HOPE_SIGNING_KEY=str(spelling))
            done = subprocess.run(['sh', str(SCRIPT), '--staging', str(self.stage), '--version', '1.0.0', '--out', str(self.out)], env=env, capture_output=True, text=True)
            # The upper-cased alias reaches the key only on a case-insensitive filesystem (macOS);
            # elsewhere it names no file, and the script rightly reports that instead.
            if spelling == inside or spelling.exists():
                self.assertNotEqual(0, done.returncode)
                self.assertIn('outside the repository', done.stderr)

    def test_hard_link_to_a_key_inside_the_repository_is_refused(self):
        self.stage_all()
        inside = self.in_repo_key('release-test-key2.pem')
        link = self.base / 'innocent.pem'
        os.link(inside, link)
        env = dict(os.environ, HOPE_SIGNING_KEY=str(link))
        done = subprocess.run(['sh', str(SCRIPT), '--staging', str(self.stage), '--version', '1.0.0', '--out', str(self.out)], env=env, capture_output=True, text=True)
        self.assertNotEqual(0, done.returncode)
        self.assertIn('inside the repository', done.stderr)

    def test_relative_key_and_paths_resolve_against_the_callers_directory(self):
        self.stage_all()
        shutil.copy(self.keys / 'a.pem', self.base / 'key.pem')
        decoy = REPO / 'target' / 'key.pem'  # the script cd's to the repo root; a relative path must not find this
        decoy.parent.mkdir(exist_ok=True)
        shutil.copy(self.keys / 'b.pem', decoy)
        self.addCleanup(decoy.unlink)
        env = dict(os.environ, HOPE_SIGNING_KEY='key.pem')
        done = subprocess.run(['sh', str(SCRIPT), '--staging', 'stage', '--version', '1.0.0', '--out', 'releases'], env=env, capture_output=True, text=True, cwd=self.base)
        self.assertEqual(0, done.returncode, done.stderr)
        expected = subprocess.run(['openssl', 'pkey', '-in', str(self.keys / 'a.pem'), '-pubout'], capture_output=True, text=True, check=True).stdout
        self.assertEqual(expected, (self.out / 'current' / 'release-public.pem').read_text())

    def test_versions_must_be_plain_semver(self):
        self.stage_all('1.0.0')
        for bad in ('v1.0.0', '1.0', 'latest', '1.0.0+x'):
            done = self.run_script(version=bad)
            self.assertNotEqual(0, done.returncode, bad)
            self.assertIn('must look like', done.stderr)
        self.assertEqual(0, self.run_script(version='1.0.0').returncode)
        self.stage_all('1.1.0-rc.1')
        self.assertEqual(0, self.run_script(version='1.1.0-rc.1').returncode)

    def test_release_commit_must_be_known_and_valid(self):
        self.stage_all(commit='unknown')
        self.assertNotEqual(0, self.run_script().returncode)
        self.assertEqual(0, self.run_script('--allow-partial').returncode)
        self.stage_all(commit='not-a-commit')
        self.assertNotEqual(0, self.run_script().returncode)
        self.assertNotEqual(0, self.run_script('--allow-partial').returncode)

    def test_mislabeled_binaries_are_refused(self):
        self.stage_all()
        wrong_arch = b'\x7fELF\x02\x01\x01\x00' + bytes(8) + b'\x02\x00\xb7\x00'
        (self.stage / 'hope-agent-linux-x86_64').write_bytes(wrong_arch)
        done = self.run_script()
        self.assertNotEqual(0, done.returncode)
        self.assertIn('hope-agent-linux-x86_64 is not a linux-x86_64 executable', done.stderr)
        self.stage_all()

    def test_old_releases_are_pruned_but_current_and_previous_survive(self):
        for minor in range(5):
            version = f'1.{minor}.0'
            self.stage_all(version)
            self.assertEqual(0, self.run_script(version=version).returncode)
        self.assertEqual(5, len(self.releases()), 'rapid publication must preserve the installation window')
        self.assertEqual('1.4.0', self.manifest()['version'])
        self.assertEqual('1.3.0', self.manifest(self.out / 'previous')['version'])

    def test_expired_releases_are_pruned_but_an_old_current_gets_a_new_retention_window(self):
        for minor in range(5):
            version = f'1.{minor}.0'
            self.stage_all(version)
            self.assertEqual(0, self.run_script(version=version).returncode)
        old_current = os.readlink(self.out / 'current')
        for index, name in enumerate(self.releases()):
            timestamp = time.time() - 48 * 3600 + index
            os.utime(self.out / name, (timestamp, timestamp))
        self.stage_all('1.5.0')
        done = self.run_script(version='1.5.0')
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertEqual(3, len(self.releases()))
        self.assertEqual(old_current, os.readlink(self.out / 'previous'))
        self.assertLess(time.time() - (self.out / old_current).stat().st_mtime, 60)

    def test_current_is_swapped_by_rename_never_missing(self):
        self.stage_all('1.0.0')
        self.assertEqual(0, self.run_script(version='1.0.0').returncode)
        first = os.readlink(self.out / 'current')
        self.stage_all('1.0.1')
        self.assertEqual(0, self.run_script(version='1.0.1').returncode)
        self.assertNotEqual(first, os.readlink(self.out / 'current'))
        self.assertTrue((self.out / first).is_dir())
        self.assertEqual([], [p.name for p in self.out.iterdir() if p.name.startswith('.')])

    def test_a_different_key_needs_explicit_rotation_and_never_mixes(self):
        self.stage_all()
        self.assertEqual(0, self.run_script().returncode)
        before = self.manifest()
        refused = self.run_script(key='b')
        self.assertNotEqual(0, refused.returncode)
        self.assertIn('--rotate-key', refused.stderr)
        self.assertEqual(before, self.manifest())  # published release untouched
        rotated = self.run_script('--rotate-key', key='b')
        self.assertEqual(0, rotated.returncode, rotated.stderr)
        self.assertNotEqual(before['key_id'], self.manifest()['key_id'])
        pem = self.out / 'current' / 'release-public.pem'
        for artifact in self.manifest()['artifacts']:  # every artifact now verifies under the new key
            subprocess.run(['openssl', 'dgst', '-sha256', '-verify', str(pem), '-signature', str(self.out / 'current' / (artifact['file'] + '.sig')), str(self.out / 'current' / artifact['file'])], check=True, capture_output=True)
        self.assertEqual(before['key_id'], self.manifest(self.out / 'previous')['key_id'])
        self.assertNotEqual(0, self.run_script('--rotate-key', '--allow-partial', key='a').returncode)

    def test_downgrade_is_refused_unless_explicit_and_previous_release_is_kept(self):
        self.stage_all('1.1.0')
        self.assertEqual(0, self.run_script(version='1.1.0').returncode)
        self.stage_all('1.0.0')
        self.assertNotEqual(0, self.run_script().returncode)
        self.assertEqual('1.1.0', self.manifest()['version'])
        self.assertEqual(0, self.run_script('--allow-downgrade').returncode)
        self.assertEqual('1.0.0', self.manifest()['version'])
        self.assertEqual('1.1.0', self.manifest(self.out / 'previous')['version'])


if __name__ == '__main__':
    unittest.main()
