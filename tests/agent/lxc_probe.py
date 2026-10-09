#!/usr/bin/env python3
"""Start a dedicated, signed reporting-service fixture on an authorized LXC."""
import argparse
import http.cookiejar
import ipaddress
import json
import os
from pathlib import Path
import shlex
import ssl
import subprocess
import urllib.request
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--host', required=True)
parser.add_argument('--known-hosts', required=True)
parser.add_argument('--directory', default='target/lxc-validation')
parser.add_argument('--server', default='https://localhost:9443')
parser.add_argument('--enrollment-token-env', help='Read an already issued token from this environment variable')
parser.add_argument('--visible-site', action='store_true', help='Install persistent service without starting a benchmark')
args = parser.parse_args()
ipaddress.ip_address(args.host)
directory = Path(args.directory).resolve()
server = args.server
context = ssl.create_default_context(cafile=str(directory / 'keys/ca.pem'))
client = urllib.request.build_opener(urllib.request.HTTPSHandler(context=context), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

def api(path, body=None):
    request = urllib.request.Request(server + '/api/v1' + path, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type': 'application/json', 'Origin': server})
    return json.load(client.open(request, timeout=15))

if args.enrollment_token_env:
    token = os.environ[args.enrollment_token_env]
else:
    if args.visible_site:
        parser.error('--visible-site requires --enrollment-token-env')
    status = api('/auth/status')
    api('/auth/setup' if status['setup_required'] else '/auth/login', {'password':'hope-lxc-fixture-password','setup_token':'hope-lxc-validation-setup-token'})
    token = api('/enrollment-tokens', {})['token']
activation = str(uuid.uuid4())
environment = dict(os.environ)
environment['SSHPASS'] = environment.pop('HOPE_TEST_SSH_PASSWORD')
options = ['-o', f'UserKnownHostsFile={args.known_hosts}', '-o', 'StrictHostKeyChecking=yes', '-o', 'PubkeyAuthentication=no', '-o', 'NumberOfPasswordPrompts=1', '-o', 'ConnectTimeout=5']
remote = f'root@{args.host}'
subprocess.run(['sshpass','-e','scp', *options, str(directory / 'keys/ca.pem'), f'{remote}:/root/hope-test-ca.pem'], env=environment, check=True, capture_output=True, timeout=20)
subprocess.run(['sshpass','-e','scp', *options, str(Path(__file__).with_name('resource_sample.py')), f'{remote}:/root/hope-resource-sample.py'], env=environment, check=True, capture_output=True, timeout=20)
script = r'''set -eu
set +x
umask 077
state=/var/lib/hope-agent-test
binary=/usr/local/lib/hope-agent-test/hope-agent
if id hope-agent-test >/dev/null 2>&1; then
    uid=$(id -u hope-agent-test)
    [ "$uid" -gt 0 ] && [ "$uid" -lt 1000 ]
    [ "$(getent passwd hope-agent-test | cut -d: -f6-7)" = "$state:/usr/sbin/nologin" ]
else
    useradd --system --user-group --no-create-home --home-dir "$state" --shell /usr/sbin/nologin hope-agent-test
fi
install -d -m 0755 /usr/local/lib/hope-agent-test
install -d -m 0700 -o hope-agent-test -g hope-agent-test "$state"
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
for name in hope-agent-linux-x86_64 hope-agent-linux-x86_64.sig release-public.pem; do
    curl --fail --silent --show-error --max-time 120 --proto '=https' --cacert /root/hope-test-ca.pem "$HOPE_SERVER/agent/releases/$name" -o "$staging/$name"
done
openssl dgst -sha256 -verify "$staging/release-public.pem" -signature "$staging/hope-agent-linux-x86_64.sig" "$staging/hope-agent-linux-x86_64" >/dev/null
install -m 0755 "$staging/hope-agent-linux-x86_64" "$binary"
install -m 0600 -o hope-agent-test -g hope-agent-test /root/hope-test-ca.pem "$state/bootstrap-ca.pem"
runuser -u hope-agent-test -- env HOPE_AGENT_DIR="$state" HOPE_SERVER="$HOPE_SERVER" HOPE_CA_CERT="$state/bootstrap-ca.pem" HOPE_ENROLLMENT_TOKEN="$HOPE_ENROLLMENT_TOKEN" "$binary" configure
rm -f "$state/bootstrap-ca.pem"
unset HOPE_ENROLLMENT_TOKEN
systemd-run --quiet --unit=hope-agent-test --property=User=hope-agent-test --property=Group=hope-agent-test --property=NoNewPrivileges=yes --property=ProtectSystem=strict --property=ProtectHome=yes --property=PrivateTmp=yes --property=ReadWritePaths="$state" --property=CapabilityBoundingSet= --property=CPUAccounting=yes --property=MemoryAccounting=yes --setenv=HOPE_AGENT_DIR="$state" --setenv=HOPE_ACTIVATION_ID="$HOPE_ACTIVATION_ID" "$binary" run
attempt=0
while [ "$attempt" -lt 60 ]; do
    pid=$(systemctl show hope-agent-test -p MainPID --value)
    if [ "$pid" -gt 0 ] && runuser -u hope-agent-test -- env HOPE_AGENT_DIR="$state" "$binary" status --activation-id "$HOPE_ACTIVATION_ID" --service-pid "$pid" --json 2>/dev/null; then
        python3 /root/hope-resource-sample.py --pid "$pid" --binary "$binary" --output /root/hope-agent-resource.json --seconds 1800 --warmup 60 >/root/hope-resource-sample.log 2>&1 </dev/null &
        echo "Resource measurement started for process $pid"
        exit 0
    fi
    attempt=$((attempt + 1))
    sleep 1
done
echo 'The fixture service has not sent an accepted report' >&2
exit 1
'''
if args.visible_site:
    script = script.replace('hope-agent-test', 'hope-agent')
    script = script.replace('set -eu\n', 'set -eu\nif [ -e /var/lib/hope-agent/config.json ] || systemctl cat hope-agent.service >/dev/null 2>&1; then echo "Existing installation found; use the upgrade workflow" >&2; exit 1; fi\n', 1)
    start = script.index('systemd-run --quiet')
    end = script.index('attempt=0', start)
    script = script[:start] + r'''cat > /etc/systemd/system/hope-agent.service <<EOF
[Unit]
Description=H.O.P.E host reporting agent
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
User=hope-agent
Group=hope-agent
ExecStart=$binary run
Environment=HOPE_AGENT_DIR=$state
Environment=HOPE_ACTIVATION_ID=$HOPE_ACTIVATION_ID
Restart=on-failure
RestartSec=30
UMask=0077
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=$state
CapabilityBoundingSet=
[Install]
WantedBy=multi-user.target
EOF
chmod 0644 /etc/systemd/system/hope-agent.service
systemctl daemon-reload
systemctl enable --now hope-agent.service
''' + script[end:]
    start = script.index('        python3 /root/hope-resource-sample.py')
    end = script.index('        exit 0', start)
    script = script[:start] + '        echo "Persistent service report accepted"\n' + script[end:]
variables = {'HOPE_SERVER':server,'HOPE_ENROLLMENT_TOKEN':token,'HOPE_ACTIVATION_ID':activation}
script = 'set +x\n' + ''.join(f'{name}={shlex.quote(value)}\nexport {name}\n' for name,value in variables.items()) + script
result = subprocess.run(['sshpass','-e','ssh', '-T', *options, remote, 'sh -s'], input=script, text=True, env=environment, capture_output=True, timeout=180)
if result.returncode:
    # These commands emit only fixed diagnostics; never print the sent script/token.
    print(result.stderr[-4096:])
    raise SystemExit(result.returncode)
print(result.stdout)
(directory / ('visible-probe.json' if args.visible_site else 'probe.json')).write_text(json.dumps({'host':args.host,'activation_id':activation,'binary_fixture':'hope-agent' if args.visible_site else 'hope-agent-test','measurement_seconds':0 if args.visible_site else 1800},indent=2)+'\n')
