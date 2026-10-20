#!/usr/bin/env python3
"""Recoverable systemd server activation; shared by the host and Proxmox installers.

Only the Python standard library is required. The journal/backups are root-private and survive
SIGKILL or power loss. No installed file changes until the stopped database and files are saved.
"""
import argparse
from contextlib import closing
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time


class ActivationError(RuntimeError):
    pass


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def ensure_directory(path, mode=0o755):
    if not path.exists():
        ensure_directory(path.parent)
        path.mkdir(mode=mode)
        sync_directory(path.parent)


def sync_tree(path):
    if path.is_symlink():
        return
    if path.is_dir():
        for child in path.iterdir():
            sync_tree(child)
        sync_directory(path)
    else:
        with path.open('rb') as handle:
            os.fsync(handle.fileno())


def remove(path):
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink(missing_ok=True)


def copy(source, destination):
    source, destination = Path(source), Path(destination)
    if source.is_dir() and not source.is_symlink():
        shutil.copytree(source, destination, symlinks=True, copy_function=copy)
        shutil.copystat(source, destination)
    else:
        shutil.copy2(source, destination, follow_symlinks=False)
    stat = source.lstat()
    os.chown(destination, stat.st_uid, stat.st_gid, follow_symlinks=False)
    # chown can clear set-ID bits and file capabilities, including when IDs are unchanged.
    shutil.copystat(source, destination, follow_symlinks=False)
    return destination


def replace(source, destination):
    ensure_directory(destination.parent)
    temporary = Path(tempfile.mkdtemp(prefix='.hope-activation-', dir=destination.parent))
    try:
        staged = temporary / 'value'
        copy(source, staged)
        sync_tree(staged)
        # The service is stopped. Directory replacement is recoverable through the journal.
        if destination.is_dir() and not destination.is_symlink():
            shutil.rmtree(destination)
        os.replace(staged, destination)
        sync_directory(destination.parent)
    finally:
        shutil.rmtree(temporary)


def environment(path):
    values = {}
    if path.exists():
        for line in path.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith(('#', ';')):
                continue
            key, separator, value = line.partition('=')
            if not separator:
                raise ActivationError(f'Invalid environment line in {path}')
            # Parse values without executing shell substitutions or sourcing operator settings.
            if value.startswith(('"', "'")):
                parts = shlex.split(value)
                if len(parts) != 1:
                    raise ActivationError(f'Invalid quoted value for {key}')
                value = parts[0]
            values[key.strip()] = value
    return values


def command(*arguments, check=True, timeout=45):
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=timeout)
    if check and result.returncode:
        raise ActivationError(f'{arguments[0]} {arguments[1]} failed: {result.stderr.strip()}')
    return result


class Activation:
    def __init__(self, root, state_dir, health_timeout):
        self.root = Path(root) if root else Path('/')
        self.state = self.path(state_dir)
        ensure_directory(self.state, mode=0o700)
        if self.state.is_symlink() or self.state.stat().st_uid != os.geteuid():
            raise ActivationError('The activation state directory must not be a symlink')
        self.state.chmod(0o700)
        self.lock = (self.state / 'lock').open('a')
        try:
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ActivationError('Another server activation is running') from error
        self.transaction = self.state / 'transaction'
        self.journal = self.transaction / 'journal.json'
        self.health_timeout = health_timeout

    def path(self, value):
        path = Path(value)
        if not path.is_absolute() or '..' in path.parts:
            raise ActivationError(f'An absolute normalized path is required: {value}')
        return self.root / str(path).lstrip('/')

    def record(self, plan):
        temporary = self.transaction / 'journal.new'
        with temporary.open('w') as handle:
            json.dump(plan, handle)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, self.journal)
        sync_directory(self.transaction)

    def active(self):
        return command('systemctl', 'is-active', '--quiet', 'hope', check=False).returncode == 0

    def stop(self):
        command('systemctl', 'stop', 'hope')
        state = command('systemctl', 'show', '--property=ActiveState', '--value', 'hope').stdout.strip()
        pid = command('systemctl', 'show', '--property=MainPID', '--value', 'hope').stdout.strip()
        if state not in ('inactive', 'failed') or pid != '0':
            raise ActivationError('hope.service has not stopped; refusing to copy or replace its database')

    def healthy(self, values, require_pid):
        bind = values.get('HOPE_BIND', '127.0.0.1:8080')
        host, separator, port = bind.rpartition(':')
        if not separator or not port.isdecimal():
            raise ActivationError(f'Unsupported HOPE_BIND: {bind}')
        if host in ('0.0.0.0', ''):
            host = '127.0.0.1'
        if host in ('::', '[::]'):
            host = '[::1]'
        url = f'http://{host}:{port}/healthz'
        deadline = time.monotonic() + self.health_timeout
        while True:
            pid = command('systemctl', 'show', '--property=MainPID', '--value', 'hope').stdout.strip()
            if self.active() and pid.isdecimal() and int(pid) > 0:
                result = command('curl', '--fail', '--silent', '--show-error', '--max-time', '2', '--noproxy', '*', url, check=False, timeout=3)
                try:
                    body = json.loads(result.stdout) if result.returncode == 0 else {}
                except ValueError:
                    body = {}
                if (isinstance(body, dict) and body.get('status') == 'ok'
                        and (body.get('pid') == int(pid) or (not require_pid and 'pid' not in body))
                        and command('systemctl', 'show', '--property=MainPID', '--value', 'hope').stdout.strip() == pid
                        and self.active()):
                    return
            if time.monotonic() >= deadline:
                raise ActivationError(f'hope.service did not become healthy at {url}')
            time.sleep(0.25)

    def diagnostics(self):
        for arguments in (("systemctl", "status", "hope", "--no-pager"),
                          ("journalctl", "-u", "hope", "-n", "40", "--no-pager")):
            try:
                result = command(*arguments, check=False, timeout=3)
            except (OSError, subprocess.SubprocessError) as error:
                print("Activation diagnostics unavailable (" + " ".join(arguments[:2]) + "): " + str(error),
                      file=sys.stderr)
                continue
            output = (result.stdout + result.stderr).strip()
            output = re.sub(r"first-run setup token: [A-Za-z0-9_-]+", "first-run setup token: [redacted]", output)
            if output:
                print("Activation diagnostics (" + " ".join(arguments[:2]) + "):\n" + output, file=sys.stderr)

    def finish(self):
        previous = self.state / 'previous'
        remove(previous)
        os.replace(self.transaction, previous)
        sync_directory(self.state)

    def recover(self):
        if not self.journal.exists():
            # An interrupted initial snapshot cannot have modified installed files.
            remove(self.transaction)
            return
        plan = json.loads(self.journal.read_text())
        if plan.get('version') != 1:
            raise ActivationError('Unknown activation journal version; retain backups for manual recovery')
        if plan['phase'] == 'committed':
            self.finish()
            return
        print('Restoring the previous server installation', file=sys.stderr)
        # An additional TERM/INT must not interrupt restoration. SIGKILL remains recoverable.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if plan['phase'] == 'ready':
            self.stop()
            for name, target in plan['targets'].items():
                if name == 'guard':
                    continue
                destination = Path(target)
                if plan['existed'][name]:
                    replace(self.transaction / name, destination)
                else:
                    remove(destination)
                    sync_directory(destination.parent)
            database = Path(plan['database'])
            for suffix in ('-wal', '-shm', '-journal'):
                Path(str(database) + suffix).unlink(missing_ok=True)
            if plan['database_existed']:
                replace(self.transaction / 'hope.db', database)
            else:
                database.unlink(missing_ok=True)
                sync_directory(database.parent)
            guard = Path(plan['targets']['guard'])
            if plan['existed']['guard']:
                replace(self.transaction / 'guard', guard)
            else:
                guard.unlink(missing_ok=True)
                sync_directory(guard.parent)
            command('systemctl', 'daemon-reload')
            if not plan['was_enabled']:
                command('systemctl', 'disable', 'hope', check=False)
        if plan['was_active']:
            command('systemctl', 'start', 'hope')
            self.healthy(plan['old_environment'], require_pid=False)
        if plan['phase'] == 'preparing':
            # No installed file was changed and this snapshot may be incomplete.
            remove(self.transaction)
            sync_directory(self.state)
            return
        # Leave a complete backup until the next proven activation replaces it.
        plan['phase'] = 'committed'
        self.record(plan)
        self.finish()
        print('Previous installation restored', file=sys.stderr)

    def install(self, package, candidate_env, candidate_unit, binary, env_file, unit_file, metadata_dir):
        self.recover()
        # Signals are converted into ordinary failures so the same recovery path runs.
        def interrupted(signum, _frame):
            raise ActivationError(f'Activation interrupted by signal {signum}')
        signal.signal(signal.SIGTERM, interrupted)
        signal.signal(signal.SIGINT, interrupted)
        package = Path(package).resolve()
        candidate_env = Path(candidate_env).resolve()
        candidate_unit = Path(candidate_unit).resolve()
        values = environment(candidate_env)
        old_values = environment(self.path(env_file))
        data = values.get('HOPE_DATA_DIR', '/var/lib/hope')
        if old_values and data != old_values.get('HOPE_DATA_DIR', '/var/lib/hope'):
            raise ActivationError('Move HOPE_DATA_DIR separately; an upgrade cannot also relocate the database')
        database = self.path(data) / 'hope.db'
        targets = {'binary': self.path(binary), 'web': self.path(values.get('HOPE_WEB_DIR', '/opt/hope/web')),
                   'environment': self.path(env_file), 'unit': self.path(unit_file)}
        targets['guard'] = targets['unit'].parent / (targets['unit'].name + '.d') / '20-hope-activation.conf'
        sources = {'binary': package / 'hope-server', 'web': package / 'web',
                   'environment': candidate_env, 'unit': candidate_unit}
        for name, member in (('version', 'VERSION'), ('tools', 'tools')):
            if (package / member).exists():
                targets[name] = self.path(metadata_dir) / member
                sources[name] = package / member
        if not os.access(sources['binary'], os.X_OK) or not (sources['web'] / 'index.html').is_file():
            raise ActivationError('Incomplete server package; installed files were not changed')
        if not candidate_env.is_file() or not candidate_unit.is_file():
            raise ActivationError('Candidate environment and unit are required')
        paths = [*targets.values(), database, self.state]
        for index, path in enumerate(paths):
            for other in paths[index + 1:]:
                if path == other or path in other.parents or other in path.parents:
                    raise ActivationError(f'Activation paths overlap: {path} and {other}')
        if database.is_symlink():
            raise ActivationError('The database must not be a symlink')
        ensure_directory(database.parent)
        ensure_directory(self.transaction, mode=0o700)
        # Keep the recovery entry point even when download staging is cleaned up or lost.
        replace(Path(__file__).resolve(), self.state / 'recover.py')
        guard_source = self.transaction / 'guard.new'
        guard_source.write_text('[Service]\nExecStartPre=+/usr/bin/python3 /' + str(self.state.relative_to(self.root)) + '/recover.py guard\n')
        sources['guard'] = guard_source
        plan = {'version': 1, 'phase': 'preparing', 'targets': {name: str(path) for name, path in targets.items()},
                'existed': {name: path.exists() or path.is_symlink() for name, path in targets.items()},
                'database': str(database), 'database_existed': database.exists(), 'old_environment': old_values,
                'was_active': self.active(),
                'was_enabled': command('systemctl', 'is-enabled', '--quiet', 'hope', check=False).returncode == 0}
        self.record(plan)
        try:
            if plan['existed']['unit'] or plan['was_active']:
                self.stop()
            plan['database_existed'] = database.exists()
            for name, path in targets.items():
                if plan['existed'][name]:
                    copy(path, self.transaction / name)
            if database.exists():
                with closing(sqlite3.connect(f'{database.as_uri()}?mode=rw', uri=True, timeout=10)) as source:
                    with closing(sqlite3.connect(self.transaction / 'hope.db')) as destination:
                        source.backup(destination)
                        if destination.execute('PRAGMA integrity_check').fetchall() != [('ok',)]:
                            raise ActivationError('The pre-upgrade database backup failed its integrity check')
                shutil.copystat(database, self.transaction / 'hope.db')
                stat = database.stat()
                os.chown(self.transaction / 'hope.db', stat.st_uid, stat.st_gid)
            elif any(Path(str(database) + suffix).exists() for suffix in ('-wal', '-journal')):
                raise ActivationError('Orphaned database journal found; refusing activation')
            sync_tree(self.transaction)
            plan['phase'] = 'ready'
            self.record(plan)
            replace(sources['guard'], targets['guard'])
            command('systemctl', 'daemon-reload')
            for name, path in targets.items():
                if name != 'guard':
                    replace(sources[name], path)
            command('systemctl', 'daemon-reload')
            command('systemctl', 'start', 'hope')
            self.healthy(values, require_pid=True)
            if not plan['existed']['unit']:
                command('systemctl', 'enable', 'hope')
            plan['phase'] = 'committed'
            self.record(plan)
            self.finish()
            print('Server activation verified; previous files and database retained in ' + str(self.state / 'previous'))
        except BaseException as error:
            interrupted = isinstance(error, ActivationError) and str(error).startswith('Activation interrupted by signal ')
            if plan['phase'] == 'ready' and not interrupted:
                try:
                    self.diagnostics()
                except BaseException as diagnostic_error:
                    print(f'Activation diagnostics failed: {diagnostic_error}', file=sys.stderr)
            try:
                self.recover()
            except BaseException as error:
                print(f'Automatic recovery incomplete: {error}. Backups retained in {self.transaction}. '
                      f'Run python3 {self.state / "recover.py"} recover before starting the server.', file=sys.stderr)
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('install', 'recover', 'guard', 'value'))
    parser.add_argument('--root', default='')
    parser.add_argument('--health-timeout', type=float, default=30)
    parser.add_argument('--package')
    parser.add_argument('--candidate-env')
    parser.add_argument('--candidate-unit')
    parser.add_argument('--metadata-dir', default='/opt/hope')
    parser.add_argument('--binary', default='/opt/hope/bin/hope-server')
    parser.add_argument('--env', default='/etc/hope/hope.env')
    parser.add_argument('--unit', default='/etc/systemd/system/hope.service')
    parser.add_argument('--key')
    args = parser.parse_args()
    if args.action == 'value':
        print(environment(Path(args.env)).get(args.key, ''))
        return
    if args.action == 'guard':
        state = Path(args.root or '/') / 'var/lib/hope-activation'
        journal = state / 'transaction/journal.json'
        if journal.exists() and json.loads(journal.read_text()).get('phase') != 'committed':
            # The service's filesystem sandbox can read this directory but cannot write it.
            with (state / 'lock').open('rb') as lock:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    return  # A live installer/recovery owns the transaction.
            raise ActivationError(f'Interrupted server activation: run python3 {state / "recover.py"} recover')
        return
    activation = Activation(args.root, '/var/lib/hope-activation', args.health_timeout)
    if args.action == 'recover':
        activation.recover()
    else:
        if not all((args.package, args.candidate_env, args.candidate_unit)):
            parser.error('install requires --package, --candidate-env and --candidate-unit')
        activation.install(args.package, args.candidate_env, args.candidate_unit, args.binary, args.env, args.unit, args.metadata_dir)


if __name__ == '__main__':
    try:
        main()
    except (ActivationError, OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
        print(f'Server activation failed: {error}', file=sys.stderr)
        sys.exit(1)
