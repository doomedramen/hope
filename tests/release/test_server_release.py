"""Verify the exact server download layout, for the supported Linux x86_64 artifacts."""
import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / 'scripts' / 'assemble-server-release.sh'


class ServerRelease(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.staging = self.root / 'downloaded'
        self.out = self.root / 'release'
        self.out.mkdir()
        (self.out / 'hope-agents.tar.gz').write_bytes(b'agent bundle')
        for arch in ('x86_64',):
            directory = self.staging / f'server-{arch}'
            directory.mkdir(parents=True)
            name = f'hope-server-{arch}-linux.tar.gz'
            content = f'server for {arch}'.encode()
            (directory / name).write_bytes(content)
            (directory / 'SHA256SUMS-server').write_text(f'{hashlib.sha256(content).hexdigest()}  {name}\n')

    def assemble(self):
        return subprocess.run(['sh', str(SCRIPT), '--staging', str(self.staging), '--out', str(self.out)], capture_output=True, text=True, timeout=10)

    def test_x86_64_server_and_agents_have_exactly_one_correct_checksum(self):
        # A stale overlaid checksum file must not determine what is published.
        (self.out / 'SHA256SUMS-server').write_text('old manifest\n')
        done = self.assemble()
        self.assertEqual(done.returncode, 0, done.stderr)
        rows = (self.out / 'SHA256SUMS-server').read_text().splitlines()
        expected = {'hope-server-x86_64-linux.tar.gz', 'hope-agents.tar.gz'}
        self.assertEqual(len(rows), 2)
        self.assertEqual({row.split()[1] for row in rows}, expected)
        for row in rows:
            digest, name = row.split()
            self.assertEqual(digest, hashlib.sha256((self.out / name).read_bytes()).hexdigest())

    def test_missing_x86_64_cannot_publish_a_partial_manifest(self):
        (self.staging / 'server-x86_64/hope-server-x86_64-linux.tar.gz').unlink()
        self.assertNotEqual(self.assemble().returncode, 0)
        self.assertFalse((self.out / 'SHA256SUMS-server').exists())

    def test_corrupt_or_duplicate_input_checksums_are_refused(self):
        manifest = self.staging / 'server-x86_64/SHA256SUMS-server'
        original = manifest.read_text()
        for content in ['0' * 64 + '  hope-server-x86_64-linux.tar.gz\n', original * 2]:
            manifest.write_text(content)
            self.assertNotEqual(self.assemble().returncode, 0)
            self.assertFalse((self.out / 'SHA256SUMS-server').exists())

    def test_missing_agent_bundle_is_refused(self):
        (self.out / 'hope-agents.tar.gz').unlink()
        self.assertNotEqual(self.assemble().returncode, 0)
        self.assertFalse((self.out / 'SHA256SUMS-server').exists())

    def test_unsupported_server_architecture_is_refused(self):
        directory = self.staging / 'server-aarch64'
        directory.mkdir()
        self.assertNotEqual(self.assemble().returncode, 0)
        self.assertFalse((self.out / 'SHA256SUMS-server').exists())
