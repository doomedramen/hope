"""scripts/assemble-release.sh policy tests with fake binaries and throwaway keys.
Run: python3 -I -m unittest discover -s tests/release -v"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / 'scripts' / 'assemble-release.sh'
PLATFORMS = ['linux-x86_64', 'linux-aarch64', 'macos-x86_64', 'macos-aarch64']
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

    def test_missing_platform_is_refused_unless_partial(self):
        self.stage_all(platforms=PLATFORMS[:3])
        refused = self.run_script()
        self.assertNotEqual(0, refused.returncode)
        self.assertIn('macos-aarch64', refused.stderr)
        self.assertEqual([], self.releases())
        self.assertEqual([], self.releases())
        partial = self.run_script('--allow-partial')
        self.assertEqual(0, partial.returncode, partial.stderr)
        self.assertTrue(self.manifest()['partial'])
        self.assertEqual(3, len(self.manifest()['artifacts']))

    def test_mixed_versions_are_refused(self):
        self.stage_all()
        (self.stage / 'hope-agent-macos-aarch64.info').write_text('version=0.9.0\nmin_os=x\n')
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

    def test_commits_must_agree_and_be_known(self):
        self.stage_all()
        (self.stage / 'hope-agent-macos-aarch64.info').write_text('version=1.0.0\ntarget=t\nmin_os=x\ncommit=111111111111\n')
        done = self.run_script()
        self.assertNotEqual(0, done.returncode)
        self.assertIn('different commits', done.stderr)
        self.stage_all(commit='unknown')
        self.assertNotEqual(0, self.run_script().returncode)
        self.assertEqual(0, self.run_script('--allow-partial').returncode)

    def test_mislabeled_binaries_are_refused(self):
        self.stage_all()
        shutil.copy(self.stage / 'hope-agent-linux-aarch64', self.stage / 'hope-agent-linux-x86_64')  # wrong machine
        done = self.run_script()
        self.assertNotEqual(0, done.returncode)
        self.assertIn('hope-agent-linux-x86_64 is not a linux-x86_64 executable', done.stderr)
        self.stage_all()
        (self.stage / 'hope-agent-macos-x86_64').write_bytes(b'#!/bin/sh\necho not a binary\n')
        self.assertNotEqual(0, self.run_script().returncode)

    def test_old_releases_are_pruned_but_current_and_previous_survive(self):
        for minor in range(5):
            version = f'1.{minor}.0'
            self.stage_all(version)
            self.assertEqual(0, self.run_script(version=version).returncode)
        self.assertEqual(3, len(self.releases()))
        self.assertEqual('1.4.0', self.manifest()['version'])
        self.assertEqual('1.3.0', self.manifest(self.out / 'previous')['version'])

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
