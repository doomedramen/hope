"""Test harness for scripts/install-agent.sh.

Everything runs inside a temporary root. Service managers, sudo, curl, account tools and the
agent binary are fakes on a stub PATH; openssl is real and signatures are verified with real RSA
keys generated per run (package-agent.sh signs with `openssl dgst -sha256 -sign`, as done here).
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]
INSTALLER = REPO / 'scripts' / 'install-agent.sh'
SERVER = 'https://hope.test:8443'
TOKEN = 'TOKEN-SECRET-9f3a71c2'
CREDENTIAL = 'CREDENTIAL-SECRET-55d0'

_keys = None


def keys():
    """Return {'good': (private, public_pem_text), 'other': (...)} generated once per process."""
    global _keys
    if _keys is None:
        directory = Path(tempfile.mkdtemp(prefix='hope-installer-keys-'))
        _keys = {}
        for name in ('good', 'other'):
            private = directory / f'{name}.pem'
            public = directory / f'{name}-public.pem'
            subprocess.run(['openssl', 'genrsa', '-out', str(private), '2048'], check=True, capture_output=True)
            subprocess.run(['openssl', 'pkey', '-in', str(private), '-pubout', '-out', str(public)], check=True, capture_output=True)
            _keys[name] = (private, public.read_text().strip())
    return _keys


def cleanup_keys():
    global _keys
    if _keys:
        shutil.rmtree(next(iter(_keys.values()))[0].parent, ignore_errors=True)
    _keys = None


# One python program implements every stubbed command; it dispatches on its own file name.
STUB = r'''#!@PYTHON@
import json, os, re, signal, sys
R = os.environ['HOPE_TEST_ROOT']
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
INST = os.environ.get('HOPE_INSTALL_ROOT', '')
def read(path, default=''):
    try:
        return open(os.path.join(R, path)).read().strip()
    except OSError:
        return default
def log(line):
    with open(os.path.join(R, 'fake', 'commands.log'), 'a') as handle:
        handle.write(line + '\n')
def flag(n):
    return os.path.exists(os.path.join(R, 'fault', n))
def hook(n):
    stall = os.path.join(R, 'fault', 'stall-at-' + n)
    if os.path.exists(stall):
        # Park here so a test can close the installer's output pipe while it is mid-transaction.
        os.unlink(stall)
        open(os.path.join(R, 'fake', 'stalled'), 'w').close()
        import time
        for _ in range(200):
            if os.path.exists(os.path.join(R, 'fault', 'release')):
                break
            time.sleep(0.1)
    for kind, sig in (('signal-at-', signal.SIGTERM), ('sigkill-at-', signal.SIGKILL)):
        path = os.path.join(R, 'fault', kind + n)
        if os.path.exists(path):
            os.unlink(path)
            os.kill(int(os.environ['HOPE_INSTALL_PID']), sig)
    path = os.path.join(R, 'fault', 'delete-at-' + n)
    if os.path.exists(path):
        target = open(path).read().strip()
        os.unlink(path)
        if os.path.exists(target):
            os.unlink(target)
def next_pid():
    path = os.path.join(R, 'fake', 'pid')
    value = int(read('fake/pid', '4000')) + 1
    open(path, 'w').write(str(value))
    return value
def accounts():
    try:
        return json.load(open(os.path.join(R, 'fake', 'accounts.json')))
    except OSError:
        return {'users': {}, 'groups': {}}
def save_accounts(data):
    json.dump(data, open(os.path.join(R, 'fake', 'accounts.json'), 'w'))
def activation_from(path):
    try:
        text = open(path).read()
    except OSError:
        return '-'
    match = re.search(r'HOPE_ACTIVATION_ID(?:=|</key><string>)([0-9a-f-]{36})', text)
    return match.group(1) if match else '-'

if name == 'uname':
    print(read('fake/os', 'Linux') if args == ['-s'] else read('fake/arch', 'x86_64'))
elif name == 'id':
    print(os.environ.get('HOPE_TEST_UID') or read('fake/uid', '1000'))
elif name == 'sudo':
    log('sudo ' + ' '.join(args))
    if flag('sudo-fail') or (flag('sudo-no-preserve') and any(a.startswith('--preserve-env') for a in args)):
        sys.exit(1)
    rest = list(args)
    while rest and rest[0].startswith('-'):
        rest = rest[2:] if rest[0] == '-u' else rest[1:]
    if rest == ['true']:
        sys.exit(0)
    env = dict(os.environ, HOPE_TEST_UID='0')
    os.execvpe(rest[0], rest, env)
elif name == 'loginctl':
    log('loginctl ' + ' '.join(args))
    state = read('fake/linger', 'no')
    if state == 'missing':
        sys.exit(1)
    print(state)
elif name == 'chown':
    log('chown ' + ' '.join(args))
elif name == 'useradd':
    log('useradd ' + ' '.join(args))
    data = accounts()
    home = args[args.index('--home-dir') + 1]
    shell = args[args.index('--shell') + 1]
    uid = str(900 + len(data['users']))
    data['users'][args[-1]] = {'UniqueID': uid, 'PrimaryGroupID': uid, 'UserShell': shell, 'NFSHomeDirectory': home}
    data['groups'][args[-1]] = {'PrimaryGroupID': uid, 'GroupMembership': ''}
    save_accounts(data)
elif name == 'getent' and args[0] == 'group':
    for who, fields in accounts()['groups'].items():
        if args[1] in (who, fields.get('PrimaryGroupID')):
            print('%s:x:%s:%s' % (who, fields['PrimaryGroupID'], fields.get('GroupMembership', '')))
            sys.exit(0)
    sys.exit(2)
elif name == 'getent':
    user = accounts()['users'].get(args[1])
    if not user:
        sys.exit(2)
    print('%s:x:%s:%s::%s:%s' % (args[1], user['UniqueID'], user['PrimaryGroupID'], user['NFSHomeDirectory'], user['UserShell']))
elif name == 'dscl':
    log('dscl ' + ' '.join(args))
    data = accounts()
    action = args[1]
    if action == '-read':
        kind, _, who = args[2].strip('/').partition('/')
        table = data['users' if kind == 'Users' else 'groups']
        if who not in table or args[3] not in table[who]:
            sys.exit(56)
        if args[3] == 'GroupMembership' and not table[who][args[3]]:
            sys.exit(56)
        print('%s: %s' % (args[3], table[who][args[3]]))
    elif action == '-list':
        table = data['users' if args[2] == '/Users' else 'groups']
        for who, fields in table.items():
            if args[3] in fields:
                print('%-30s %s' % (who, fields[args[3]]))
    elif action == '-create':
        kind, _, who = args[2].strip('/').partition('/')
        table = data['users' if kind == 'Users' else 'groups']
        table.setdefault(who, {})
        if len(args) > 4:
            table[who][args[3]] = args[4]
        save_accounts(data)
elif name == 'systemctl':
    scope = 'user' if '--user' in args else 'system'
    rest = [a for a in args if a != '--user']
    log('systemctl[%s] %s' % (scope, ' '.join(rest)))
    if any('hope-agent-update' in a for a in rest):
        if flag('updater-units-fail') and rest[0] == 'enable':
            sys.exit(1)
        sys.exit(0)
    marker = os.path.join(R, 'service', scope + '.active')
    unit = (INST + '/etc/systemd/system/hope-agent.service') if scope == 'system' else (os.environ['HOME'] + '/.config/systemd/user/hope-agent.service')
    command = rest[0]
    if command == 'is-system-running':
        state = read('fake/manager-' + scope, 'running')
        print(state)
        sys.exit(0 if state == 'running' else 1)
    elif command == 'is-active':
        sys.exit(0 if os.path.exists(marker) else 3)
    elif command == 'daemon-reload':
        hook('write-unit')
    elif command == 'enable':
        hook('activate')
        if flag('enable-fail'):
            print('Failed to enable unit', file=sys.stderr)
            sys.exit(1)
        open(marker, 'w').write('%s %d' % (activation_from(unit), next_pid()))
    elif command == 'start':
        hook('start')
        if flag('start-fail'):
            sys.exit(1)
        open(marker, 'w').write('%s %d' % (activation_from(unit), next_pid()))
    elif command == 'stop':
        hook('stop')
        if os.path.exists(marker):
            os.unlink(marker)
    elif command == 'show':
        content = read('service/' + scope + '.active', '- 0')
        print(content.split()[1] if '--value' in rest else 'MainPID=' + content.split()[1])
elif name == 'launchctl':
    log('launchctl ' + ' '.join(args))
    if any('io.hope.agent.update' in a for a in args):
        sys.exit(1 if flag('updater-units-fail') and args[0] == 'bootstrap' else 0)
    marker = os.path.join(R, 'service', 'launchd.active')
    if args[0] == 'print' and '/io.hope.agent' in args[1]:
        if not os.path.exists(marker):
            sys.exit(113)
        print('\tstate = ' + ('waiting' if flag('launchd-no-pid') else 'running'))
        if not flag('launchd-no-pid'):
            print('\tpid = ' + read('service/launchd.active', '- 0').split()[1])
    elif args[0] == 'print':
        sys.exit(0 if not flag('no-' + args[1].split('/')[0]) else 113)
    elif args[0] == 'bootstrap':
        hook('activate')
        if flag('enable-fail'):
            os.unlink(os.path.join(R, 'fault', 'enable-fail'))
            sys.exit(5)
        open(marker, 'w').write('%s %d' % (activation_from(args[2]), next_pid()))
    elif args[0] == 'bootout':
        hook('stop')
        if os.path.exists(marker):
            os.unlink(marker)
elif name == 'flock':
    import fcntl
    try:
        fcntl.flock(int(args[-1]), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit(1)
elif name == 'curl':
    log('curl ' + ' '.join(args))
    leaked = [v for v in ('CURL_CA_BUNDLE', 'SSL_CERT_FILE', 'SSL_CERT_DIR') if v in os.environ]
    if leaked:
        log('curl-env-leak ' + ','.join(leaked))
    if args[0] != '-q':
        print('curl: user config not disabled', file=sys.stderr)
        sys.exit(99)
    if '--proto' not in args or args[args.index('--proto') + 1] != '=https':
        print('curl: insecure invocation', file=sys.stderr)
        sys.exit(99)
    url = [a for a in args if a.startswith('https://')][-1]
    head = '--head' in args
    output = args[args.index('-o') + 1] if '-o' in args else None
    prefix = 'https://hope.test:8443/agent/releases/'
    if flag('curl-unreachable'):
        sys.exit(6)
    if not url.startswith(prefix):
        sys.exit(22)
    artifact = url[len(prefix):]
    path = os.path.join(R, 'release', artifact)
    status = read('fault/head-status', '') if head else ''
    missing = not os.path.isfile(path) or (not head and flag('curl-fail-' + artifact))
    if status or missing:
        if '--write-out' in args:
            sys.stdout.write(status or '404')
        print('curl: (22) The requested URL returned an error', file=sys.stderr)
        sys.exit(22)
    if '--write-out' in args:
        sys.stdout.write('200')
    if output and not head:
        import shutil
        shutil.copyfile(path, output)
    sys.exit(0)
'''

AGENT = r'''#!/bin/sh
VERSION=@VERSION@
R=$HOPE_TEST_ROOT
printf 'agent[%s] %s\n' "$VERSION" "$*" >> "$R/fake/commands.log"
hook() {
    if [ -e "$R/fault/stall-at-$1" ]; then
        rm -f "$R/fault/stall-at-$1"; : > "$R/fake/stalled"; i=0
        while [ ! -e "$R/fault/release" ] && [ "$i" -lt 200 ]; do sleep 0.1; i=$((i+1)); done
    fi
    if [ -e "$R/fault/signal-at-$1" ]; then rm -f "$R/fault/signal-at-$1"; kill -TERM "$HOPE_INSTALL_PID"; fi
    if [ -e "$R/fault/sigkill-at-$1" ]; then rm -f "$R/fault/sigkill-at-$1"; kill -KILL "$HOPE_INSTALL_PID"; fi
}
cfg=${HOPE_AGENT_DIR:-/nonexistent}/config.json
case "${1:-}" in
--version)
    [ -e "$R/fault/version-fail" ] && exit 126
    # place-binary fires only inside a transaction; the stage-phase version probe must not trigger it.
    case "$0" in */usr/local/bin/hope-agent|*/.local/bin/hope-agent)
        [ -n "$(find "$R/root" "$R/home" -name journal 2>/dev/null | head -n 1)" ] && hook place-binary;; esac
    echo "hope-agent $VERSION";;
check-config)
    [ -f "$cfg" ] || { echo 'Agent is not enrolled' >&2; exit 1; }
    server=$(sed -n 's/^server=//p' "$cfg")
    if [ -n "${HOPE_SERVER:-}" ] && [ "${HOPE_SERVER%/}" != "$server" ]; then echo 'Existing agent belongs to another server' >&2; exit 1; fi
    echo 'Agent configuration is valid';;
configure)
    hook configure
    if [ -e "$R/fault/configure" ]; then echo 'enrollment rejected: token expired or already used' >&2; exit 1; fi
    if [ ! -f "$cfg" ]; then
        [ -n "${HOPE_ENROLLMENT_TOKEN:-}" ] || { echo 'token required' >&2; exit 1; }
        mkdir -p "$HOPE_AGENT_DIR"
        printf 'agent_id=agent-1\nserver=%s\ncredential=@CREDENTIAL@\nsettings=none\n' "$HOPE_SERVER" > "$cfg"
    fi
    grep -v '^settings=' "$cfg" > "$cfg.tmp"; echo "settings=$VERSION" >> "$cfg.tmp"; mv "$cfg.tmp" "$cfg"
    echo 'Agent configuration ready';;
restore-config)
    [ -f "$cfg" ] && [ -f "$2" ] || exit 1
    { grep -v '^credential=' "$2"; grep '^credential=' "$cfg"; } > "$cfg.tmp"; mv "$cfg.tmp" "$cfg"
    echo 'Previous settings restored';;
status)
    shift
    while [ $# -gt 0 ]; do
        case "$1" in --activation-id) activation=$2; shift;; --service-pid) servicepid=$2; shift;; esac
        shift
    done
    hook wait
    [ -e "$R/fault/no-report" ] && exit 1
    for marker in "$R"/service/*.active; do
        [ -f "$marker" ] || continue
        read -r act pid < "$marker"
        if [ "$act" = "${activation:-}" ] && [ "$pid" = "${servicepid:-}" ]; then echo '{"report_id":"r"}'; exit 0; fi
    done
    exit 1;;
*) exit 2;;
esac
'''


class Result:
    def __init__(self, code, out, err):
        self.code, self.out, self.err = code, out, err

    @property
    def phases(self):
        return [line.split('phase=')[1] for line in self.err.splitlines() if line.startswith('hope-install: phase=')]

    @property
    def errors(self):
        return [line.split('error=')[1].split()[0] for line in self.err.splitlines() if line.startswith('hope-install: error=')]


class Env:
    def __init__(self, os_name='Linux', arch='x86_64'):
        self.base = Path(tempfile.mkdtemp(prefix='hope-installer-')).resolve()
        self.root = self.base / 'root'
        self.home = self.base / 'home'
        self.bin = self.base / 'bin'
        self.tmp = self.base / 'tmp'
        self.rel = self.base / 'release'
        for name in ('fake', 'fault', 'service'):
            (self.base / name).mkdir()
        for path in (self.root, self.home, self.bin, self.tmp, self.rel):
            path.mkdir()
        text = STUB.replace('@PYTHON@', sys.executable)
        for command in ('uname', 'id', 'sudo', 'chown', 'useradd', 'getent', 'dscl', 'systemctl', 'loginctl', 'launchctl', 'curl', 'flock'):
            (self.bin / command).write_text(text)
            (self.bin / command).chmod(0o755)
        self.runs = []
        self.set('fake/os', os_name)
        self.set('fake/arch', arch)
        self.os_name = os_name
        self.platform = 'linux' if os_name == 'Linux' else 'macos'
        self.arch_name = 'aarch64' if arch in ('arm64', 'aarch64') else 'x86_64'
        self.entries = {}
        self.manifest_version = 'v1'
        self.publish('v1')
        installer = INSTALLER.read_text().replace('@@HOPE_PUBLIC_KEY@@', keys()['good'][1])
        self.installer = self.base / 'install-agent.sh'
        self.installer.write_text(installer)

    # --- fixtures -------------------------------------------------------------------------------
    def set(self, relative, content=''):
        path = self.base / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def fault(self, name):
        (self.base / 'fault' / name).write_text('')

    def publish(self, version, key='good', platform=None, arch=None, manifest_key='good', manifest_version=None, tamper=None):
        """Write a signed artifact and re-issue the signed manifest listing every published artifact.

        key signs the artifact, manifest_key the manifest; manifest_version lets a test issue a
        manifest whose version differs from the artifact's (a mixed release). tamper(entry) may
        alter a manifest entry before the manifest is signed.
        """
        platform, arch = platform or self.platform, arch or self.arch_name
        artifact = self.rel / f'hope-agent-{platform}-{arch}'
        artifact.write_text(AGENT.replace('@VERSION@', version).replace('@CREDENTIAL@', CREDENTIAL))
        subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(keys()[key][0]), '-out', str(artifact) + '.sig', str(artifact)], check=True, capture_output=True)
        data = artifact.read_bytes()
        entry = dict(platform=platform, arch=arch, file=artifact.name, sha256=hashlib.sha256(data).hexdigest(), size=len(data), min_os='test')
        if tamper:
            tamper(entry)
        self.entries[(platform, arch)] = entry
        self.write_manifest(manifest_version or version, manifest_key)

    def write_manifest(self, version, key='good'):
        """Same layout as scripts/assemble-release.sh: one top-level key per line, one artifact per line."""
        self.manifest_version = version
        lines = ['{', '  "format": 1,', f'  "version": "{version}",', '  "created": "2026-10-14T00:00:00Z",', '  "key_id": "test",', '  "partial": true,', '  "artifacts": [']
        rows = [json.dumps(entry, separators=(',', ':')) for entry in self.entries.values()]
        lines += ['    ' + row + (',' if index < len(rows) - 1 else '') for index, row in enumerate(rows)]
        lines += ['  ]', '}']
        manifest = self.rel / 'manifest.json'
        manifest.write_text('\n'.join(lines) + '\n')
        subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(keys()[key][0]), '-out', str(manifest) + '.sig', str(manifest)], check=True, capture_output=True)

    def replace_artifact(self, version, platform=None, arch=None):
        """Swap the served binary for another version without touching the manifest."""
        artifact = self.rel / f'hope-agent-{platform or self.platform}-{arch or self.arch_name}'
        artifact.write_text(AGENT.replace('@VERSION@', version).replace('@CREDENTIAL@', CREDENTIAL))
        subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(keys()['good'][0]), '-out', str(artifact) + '.sig', str(artifact)], check=True, capture_output=True)

    def unpublish(self, platform=None, arch=None):
        platform, arch = platform or self.platform, arch or self.arch_name
        (self.rel / f'hope-agent-{platform}-{arch}').unlink()
        self.entries.pop((platform, arch), None)
        self.write_manifest(self.manifest_version)

    def txn(self, mode='system'):
        if mode == 'system':
            if self.platform == 'linux':
                return self.root / 'var/lib/hope-agent-install'
            return self.root / 'Library/Application Support/hope-agent-install'
        return self.paths('user')['state'] / '.install-transaction'

    def paths(self, mode):
        if mode == 'system':
            if self.platform == 'linux':
                return dict(binary=self.root / 'usr/local/bin/hope-agent', unit=self.root / 'etc/systemd/system/hope-agent.service', state=self.root / 'var/lib/hope-agent')
            return dict(binary=self.root / 'usr/local/bin/hope-agent', unit=self.root / 'Library/LaunchDaemons/io.hope.agent.plist', state=self.root / 'var/lib/hope-agent')
        if self.platform == 'linux':
            return dict(binary=self.home / '.local/bin/hope-agent', unit=self.home / '.config/systemd/user/hope-agent.service', state=self.home / '.local/state/hope-agent')
        return dict(binary=self.home / '.local/bin/hope-agent', unit=self.home / 'Library/LaunchAgents/io.hope.agent.plist', state=self.home / '.local/state/hope-agent')

    def preinstall(self, mode='system', version='v1', active=True, server=SERVER):
        paths = self.paths(mode)
        for key in ('binary', 'unit'):
            paths[key].parent.mkdir(parents=True, exist_ok=True)
        paths['state'].mkdir(parents=True, exist_ok=True)
        paths['binary'].write_text(AGENT.replace('@VERSION@', version).replace('@CREDENTIAL@', CREDENTIAL))
        paths['binary'].chmod(0o755)
        paths['unit'].write_text('OLD-UNIT\nEnvironment="HOPE_ACTIVATION_ID=00000000-0000-4000-8000-000000000000"\n')
        (paths['state'] / 'config.json').write_text(f'agent_id=agent-1\nserver={server}\ncredential={CREDENTIAL}\nsettings=old-{version}\n')
        if active:
            scope = 'launchd' if self.platform == 'macos' else ('system' if mode == 'system' else 'user')
            self.set(f'service/{scope}.active', '00000000-0000-4000-8000-000000000000 111')
        return paths

    # --- execution ------------------------------------------------------------------------------
    def _environment(self, uid, mode, token, server, extra):
        env = {
            'PATH': f'{self.bin}:' + os.environ['PATH'],
            'HOME': str(self.home),
            'TMPDIR': str(self.tmp),
            'HOPE_TEST_ROOT': str(self.base),
            'HOPE_INSTALL_ROOT': str(self.root),
            'HOPE_POLL_INTERVAL': '0.2',
            'HOPE_FIRST_REPORT_TIMEOUT': '10',
            'LC_ALL': 'C',
            'PYTHONDONTWRITEBYTECODE': '1',
        }
        if server is not None:
            env['HOPE_SERVER'] = server
        if token is not None:
            env['HOPE_ENROLLMENT_TOKEN'] = token
        if mode:
            env['HOPE_INSTALL_MODE'] = mode
        env.update({key: str(value) for key, value in extra.items()})
        self.set('fake/uid', str(uid))
        return env

    def run(self, operation='install', uid=0, mode=None, token=TOKEN, server=SERVER, timeout=90, **extra):
        env = self._environment(uid, mode, token, server, extra)
        done = subprocess.run([os.environ.get('HOPE_TEST_SHELL', 'sh'), str(self.installer), operation], env=env, capture_output=True, text=True, timeout=timeout, stdin=subprocess.DEVNULL)
        result = Result(done.returncode, done.stdout, done.stderr)
        self.runs.append(result)
        return result

    def updater(self, **extra):
        """Run the hope-agent-update the installer left behind, as root."""
        wrapper = self.root / 'usr/local/sbin/hope-agent-update'
        env = self._environment(0, None, None, None, extra)
        done = subprocess.run([os.environ.get('HOPE_TEST_SHELL', 'sh'), str(wrapper)], env=env, capture_output=True, text=True, timeout=120, stdin=subprocess.DEVNULL)
        result = Result(done.returncode, done.stdout, done.stderr)
        self.runs.append(result)
        return result

    def run_with_closed_output(self, operation='install', uid=0, mode=None, token=TOKEN, server=SERVER, timeout=90, **extra):
        """Run the installer, wait until a stub parks at a stall-at-<phase> fault, close the reader of the
        installer's stderr (a dropped SSH session or a closed pipe), then let it continue."""
        env = self._environment(uid, mode, token, server, extra)
        process = subprocess.Popen([os.environ.get('HOPE_TEST_SHELL', 'sh'), str(self.installer), operation], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, stdin=subprocess.DEVNULL)
        stalled = self.base / 'fake' / 'stalled'
        deadline = time.time() + 30
        while not stalled.exists() and process.poll() is None and time.time() < deadline:
            time.sleep(0.05)
        process.stderr.close()
        (self.base / 'fault' / 'release').write_text('')
        code = process.wait(timeout=timeout)
        result = Result(code, '', '')
        self.runs.append(result)
        return result

    def log(self):
        try:
            return (self.base / 'fake/commands.log').read_text()
        except OSError:
            return ''

    def snapshot(self, directory):
        found = {}
        for path in sorted(Path(directory).rglob('*')):
            if path.relative_to(directory).parts[:2] == ('Library', 'Caches'):
                continue
            found[str(path.relative_to(directory))] = path.read_bytes() if path.is_file() else None
        return found

    def assert_no_secret(self):
        problems = []
        for result in self.runs:
            for stream in (result.out, result.err):
                if TOKEN in stream or CREDENTIAL in stream:
                    problems.append('secret in installer output')
        for path in self.base.rglob('*'):
            if path.is_file() and not path.is_symlink() and path.parent.name != 'release':
                try:
                    data = path.read_bytes()
                except OSError:
                    continue
                if TOKEN.encode() in data:
                    problems.append(f'token written to {path}')
                if (path.name == 'commands.log' or path.suffix in ('.service', '.plist')) and CREDENTIAL.encode() in data:
                    problems.append(f'credential in {path.name}')
        return problems

    def close(self):
        shutil.rmtree(self.base, ignore_errors=True)
