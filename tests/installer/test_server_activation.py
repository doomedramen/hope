"""Fault-injection tests for the real activation helper, SQLite and filesystem.

Only systemd and HTTP are stubbed. Runs on macOS too; real Linux/systemd gates remain necessary.
"""
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest

REPO = Path(__file__).resolve().parents[2]
HELPER = REPO / 'scripts/activate-server.py'

STUB = r'''#!@PYTHON@
import json, os, pathlib, sqlite3, sys, time
root = pathlib.Path(os.environ['ACTIVATION_TEST_ROOT'])
args = sys.argv[1:]
name = pathlib.Path(sys.argv[0]).name
with (root / 'events').open('a') as output:
    output.write(json.dumps([name, *args]) + '\n')
running = root / 'running'
enabled = root / 'enabled'
binary = root / os.environ.get('ACTIVATION_BINARY', 'opt/hope/bin/hope-server')
new = binary.exists() and binary.read_text().startswith('new')
if name == 'systemctl':
    if args[0] == 'is-active':
        sys.exit(0 if running.exists() else 3)
    if args[0] == 'is-enabled':
        sys.exit(0 if enabled.exists() else 1)
    if args[0] == 'show':
        if '--property=MainPID' in args:
            print('123' if running.exists() else '0')
        else:
            print('active' if running.exists() else 'inactive')
    if args[0] == 'stop':
        if (root / 'fail-stop').exists(): sys.exit(1)
        running.unlink(missing_ok=True)
    if args[0] == 'disable': enabled.unlink(missing_ok=True)
    if args[0] == 'enable': enabled.touch()
    if args[0] == 'start':
        if new:
            db = root / os.environ.get('ACTIVATION_DATA', 'var/lib/hope') / 'hope.db'
            db.parent.mkdir(parents=True, exist_ok=True)
            with sqlite3.connect(db) as connection:
                connection.executescript('CREATE TABLE IF NOT EXISTS migrated(value); INSERT INTO migrated VALUES(99); PRAGMA user_version=99;')
            if (root / 'fail-start').exists(): sys.exit(1)
        running.touch()
    sys.exit(0)
if name == 'curl':
    if new and (root / 'hold-health').exists():
        (root / 'entered-health').touch()
        while (root / 'hold-health').exists(): time.sleep(0.01)
    if new and (root / 'fail-health').exists():
        if (root / 'fail-recovery-stop').exists(): (root / 'fail-stop').touch()
        sys.exit(22)
    pid = 999 if new and (root / 'wrong-pid').exists() else 123
    print(json.dumps({'status': 'ok', 'pid': pid}))
    sys.exit(0)
sys.exit(1)
'''


class ServerActivation(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='hope-activation-test-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.bin = self.root / 'stubs'
        self.bin.mkdir()
        for name in ('systemctl', 'curl'):
            file = self.bin / name
            file.write_text(STUB.replace('@PYTHON@', sys.executable))
            file.chmod(0o755)
        self.env = {**os.environ, 'PATH': f'{self.bin}:{os.environ["PATH"]}', 'ACTIVATION_TEST_ROOT': str(self.root)}
        self.package = self.root / 'package'
        (self.package / 'web').mkdir(parents=True)
        (self.package / 'hope-server').write_text('new-binary')
        (self.package / 'hope-server').chmod(0o755)
        (self.package / 'web/index.html').write_text('new-console')
        (self.package / 'VERSION').write_text('new-version')
        (self.package / 'tools').mkdir()
        (self.package / 'tools/tool').write_text('new-tool')
        self.candidate_env = self.root / 'candidate.env'
        self.candidate_env.write_text('HOPE_DATA_DIR=/var/lib/hope\nHOPE_WEB_DIR=/opt/hope/web\nHOPE_BIND=127.0.0.1:8090\nCUSTOM=new\n')
        self.candidate_env.chmod(0o640)
        self.candidate_unit = self.root / 'candidate.service'
        self.candidate_unit.write_text('new-unit')
        self.binary = self.root / 'opt/hope/bin/hope-server'
        self.web = self.root / 'opt/hope/web/index.html'
        self.environment = self.root / 'etc/hope/hope.env'
        self.unit = self.root / 'etc/systemd/system/hope.service'
        self.database = self.root / 'var/lib/hope/hope.db'
        self.state = self.root / 'var/lib/hope-activation'
        for file, value in ((self.binary, 'old-binary'), (self.web, 'old-console'), (self.environment, 'HOPE_DATA_DIR=/var/lib/hope\nCUSTOM=old\n'), (self.unit, 'old-unit')):
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text(value)
        self.binary.chmod(0o6751)
        self.environment.chmod(0o600)
        self.database.parent.mkdir(parents=True)
        with closing(sqlite3.connect(self.database)) as connection:
            connection.executescript('CREATE TABLE original(value); INSERT INTO original VALUES(7); PRAGMA user_version=14;')
        (self.root / 'running').touch()
        (self.root / 'enabled').touch()

    def arguments(self, action='install'):
        arguments = [sys.executable, str(HELPER), action, '--root', str(self.root), '--health-timeout', '0']
        if action == 'install':
            arguments += ['--package', str(self.package), '--candidate-env', str(self.candidate_env), '--candidate-unit', str(self.candidate_unit)]
        if getattr(self, 'binary_target', None) and action == 'install':
            arguments += ['--binary', self.binary_target]
        return arguments

    def run_helper(self, action='install'):
        return subprocess.run(self.arguments(action), env=self.env, capture_output=True, text=True, timeout=15)

    def assert_original(self):
        for path, expected in ((self.binary, 'old-binary'), (self.web, 'old-console'), (self.unit, 'old-unit')):
            self.assertEqual(path.read_text(), expected)
        self.assertIn('CUSTOM=old', self.environment.read_text())
        self.assertEqual(self.environment.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.binary.stat().st_mode & 0o7777, 0o6751)
        with closing(sqlite3.connect(self.database)) as connection:
            self.assertEqual(connection.execute('PRAGMA user_version').fetchone(), (14,))
            self.assertEqual(connection.execute('SELECT * FROM original').fetchall(), [(7,)])
            self.assertEqual(connection.execute("SELECT name FROM sqlite_master WHERE name='migrated'").fetchall(), [])
        self.assertTrue((self.root / 'running').exists())
        self.assertFalse((self.state / 'transaction').exists())

    def hold_activation(self):
        (self.root / 'hold-health').touch()
        process = subprocess.Popen(self.arguments(), env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        def cleanup():
            (self.root / 'hold-health').unlink(missing_ok=True)
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate(timeout=5)
        self.addCleanup(cleanup)
        deadline = time.monotonic() + 10
        while not (self.root / 'entered-health').exists():
            if process.poll() is not None or time.monotonic() > deadline:
                self.fail(f'activation never reached health check: {process.communicate()}')
            time.sleep(0.01)
        return process

    def test_success_retains_complete_private_pre_migration_backup(self):
        done = self.run_helper()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.binary.read_text(), 'new-binary')
        self.assertEqual(self.web.read_text(), 'new-console')
        self.assertEqual((self.root / 'opt/hope/VERSION').read_text(), 'new-version')
        self.assertEqual((self.root / 'opt/hope/tools/tool').read_text(), 'new-tool')
        with closing(sqlite3.connect(self.database)) as connection:
            self.assertEqual(connection.execute('PRAGMA user_version').fetchone(), (99,))
        with closing(sqlite3.connect(self.state / 'previous/hope.db')) as connection:
            self.assertEqual(connection.execute('PRAGMA user_version').fetchone(), (14,))
            self.assertEqual(connection.execute('SELECT * FROM original').fetchall(), [(7,)])
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.state / 'previous/environment').read_bytes(), b'HOPE_DATA_DIR=/var/lib/hope\nCUSTOM=old\n')
        self.assertEqual(self.run_helper('guard').returncode, 0)

    def test_failed_start_restores_database_files_permissions_and_running_service(self):
        (self.root / 'fail-start').touch()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Previous installation restored', done.stderr)
        self.assert_original()

    def test_failed_health_or_another_process_cannot_confirm_activation(self):
        for marker in ('fail-health', 'wrong-pid'):
            with self.subTest(marker=marker):
                (self.root / marker).touch()
                done = self.run_helper()
                self.assertNotEqual(done.returncode, 0)
                self.assert_original()
                (self.root / marker).unlink()

    def test_failed_recovery_keeps_the_journal_and_guard_until_retry_succeeds(self):
        (self.root / 'fail-health').touch()
        (self.root / 'fail-recovery-stop').touch()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Automatic recovery incomplete', done.stderr)
        self.assertTrue((self.state / 'transaction/hope.db').is_file())
        self.assertNotEqual(self.run_helper('guard').returncode, 0)
        (self.root / 'fail-stop').unlink()
        recovered = self.run_helper('recover')
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_original()

    def test_failure_after_each_file_replacement_restores_the_complete_installation(self):
        for source, target in ((self.package / 'hope-server', 'opt/hope/bin/hope-server'), (self.package / 'web', 'opt/hope/web'),
                               (self.package / 'VERSION', 'opt/hope/VERSION'), (self.package / 'tools', 'opt/hope/tools'),
                               (self.candidate_env, 'etc/hope/hope.env'), (self.candidate_unit, 'etc/systemd/system/hope.service'),
                               (self.state / 'transaction/guard.new', 'etc/systemd/system/hope.service.d/20-hope-activation.conf')):
            with self.subTest(target=target):
                # Inject an I/O failure after the real replacement, without production test hooks.
                driver = self.root / 'fault.py'
                driver.write_text(f"""import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location('activation', {str(HELPER)!r})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
original = module.replace
failed = False
def fail_after_replace(source, destination):
    global failed
    original(source, destination)
    if not failed and pathlib.Path(source).resolve() == pathlib.Path({str(source)!r}).resolve() and pathlib.Path(destination).resolve() == pathlib.Path({str(self.root / target)!r}).resolve():
        failed = True
        raise OSError('simulated I/O failure after replacement')
module.replace = fail_after_replace
sys.argv = {self.arguments()[1:]!r}
module.main()
""")
                done = subprocess.run([sys.executable, str(driver)], env=self.env, capture_output=True, text=True, timeout=15)
                self.assertNotEqual(done.returncode, 0)
                self.assertIn('simulated I/O failure', done.stderr)
                self.assert_original()
                self.assertFalse((self.root / 'opt/hope/VERSION').exists())
                self.assertFalse((self.root / 'opt/hope/tools').exists())

    def test_stop_failure_cannot_replace_files_or_database(self):
        (self.root / 'fail-stop').touch()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assert_original()

    def test_corrupt_database_fails_before_any_installed_file_changes(self):
        self.database.write_bytes(b'not a database')
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertEqual(self.binary.read_text(), 'old-binary')
        self.assertEqual(self.database.read_bytes(), b'not a database')
        self.assertTrue((self.root / 'running').exists())

    def test_fresh_install_failure_removes_new_files_and_migrated_database(self):
        for path in (self.binary, self.environment, self.unit, self.database, self.root / 'running', self.root / 'enabled'):
            path.unlink()
        shutil.rmtree(self.web.parent)
        (self.root / 'fail-health').touch()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0, done.stderr)
        for path in (self.binary, self.web, self.environment, self.unit, self.database, self.root / 'running', self.root / 'enabled'):
            self.assertFalse(path.exists(), path)

    def test_sigterm_restores_the_previous_installation(self):
        process = self.hold_activation()
        process.terminate()
        _, stderr = process.communicate(timeout=10)
        self.assertNotEqual(process.returncode, 0)
        self.assertIn('Previous installation restored', stderr)
        self.assert_original()

    def test_sigkill_blocks_unattended_start_and_is_recoverable_before_retry(self):
        process = self.hold_activation()
        self.assertEqual(self.run_helper('guard').returncode, 0, 'live installer permits its own startup')
        process.kill()
        process.wait(timeout=5)
        done = self.run_helper('guard')
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Interrupted server activation', done.stderr)
        recovered = self.run_helper('recover')
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_original()
        (self.root / 'hold-health').unlink()
        retried = self.run_helper()
        self.assertEqual(retried.returncode, 0, retried.stderr)

    def test_concurrent_activation_is_rejected_before_changing_anything(self):
        process = self.hold_activation()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Another server activation is running', done.stderr)
        (self.root / 'hold-health').unlink()
        _, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 0, stderr)

    def test_backup_includes_committed_wal_data(self):
        connection = sqlite3.connect(self.database)
        self.addCleanup(connection.close)
        connection.execute('PRAGMA journal_mode=WAL')
        connection.execute('PRAGMA wal_autocheckpoint=0')
        connection.execute('INSERT INTO original VALUES(8)')
        connection.commit()
        self.assertTrue(Path(str(self.database) + '-wal').exists())
        done = self.run_helper()
        self.assertEqual(done.returncode, 0, done.stderr)
        with closing(sqlite3.connect(self.state / 'previous/hope.db')) as saved:
            self.assertEqual(saved.execute('SELECT * FROM original').fetchall(), [(7,), (8,)])

    def test_inactive_service_stays_inactive_after_failure(self):
        (self.root / 'running').unlink()
        (self.root / 'fail-health').touch()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((self.root / 'running').exists())
        self.assertEqual(self.binary.read_text(), 'old-binary')

    def test_custom_bind_and_quoted_paths_are_used_without_shell_execution(self):
        self.candidate_env.write_text('HOPE_DATA_DIR=/var/lib/hope\nHOPE_WEB_DIR="/srv/custom web"\nHOPE_BIND=127.0.0.1:9123\nCUSTOM="$(touch /tmp/never-execute)"\n')
        done = self.run_helper()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual((self.root / 'srv/custom web/index.html').read_text(), 'new-console')
        events = [json.loads(line) for line in (self.root / 'events').read_text().splitlines()]
        self.assertTrue(any(event[0] == 'curl' and event[-1] == 'http://127.0.0.1:9123/healthz' for event in events))

    def test_proxmox_binary_location_recovers_without_removing_the_legacy_checkout(self):
        self.binary_target = '/usr/local/bin/hope-server'
        self.env['ACTIVATION_BINARY'] = 'usr/local/bin/hope-server'
        (self.root / 'fail-health').touch()
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((self.root / 'usr/local/bin/hope-server').exists())
        self.assert_original()
        self.assertFalse((self.root / 'opt/hope/VERSION').exists())
        self.assertFalse((self.root / 'opt/hope/tools').exists())

    def test_database_relocation_is_refused_before_stopping_service(self):
        self.candidate_env.write_text('HOPE_DATA_DIR=/srv/different-data\n')
        done = self.run_helper()
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('Move HOPE_DATA_DIR separately', done.stderr)
        self.assert_original()
        self.assertNotIn('["systemctl", "stop"', (self.root / 'events').read_text() if (self.root / 'events').exists() else '')


if __name__ == '__main__':
    unittest.main()
