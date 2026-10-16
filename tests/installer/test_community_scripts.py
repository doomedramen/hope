"""Structural checks for the community-scripts pair (ct/hope.sh, install/hope-install.sh).

The engine (community-scripts/core) is not run here; these tests pin the conventions the
community-scripts work instructions enforce, so a change that would be rejected upstream, or
that would break `update` inside a container, fails locally first.
"""

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CT = (ROOT / 'ct' / 'hope.sh').read_text()
INSTALL = (ROOT / 'install' / 'hope-install.sh').read_text()
META = json.loads((ROOT / 'json' / 'hope.json').read_text())


def functions(script):
    return re.findall(r'^(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{', script, re.M)


class CtScript(unittest.TestCase):
    def test_bootstrap_names_this_repository_as_the_scripts_root(self):
        lines = CT.splitlines()
        self.assertEqual(lines[0], '#!/usr/bin/env bash')
        self.assertEqual(lines[1], '_CS_DEFAULT_URL="https://raw.githubusercontent.com/doomedramen/hope/main"')
        self.assertTrue(lines[2].startswith('_cs_boot='))
        self.assertIn('core/build.func', lines[3])

    def test_only_update_script_is_defined(self):
        self.assertEqual(functions(CT), ['update_script'])

    def test_variables_and_flow(self):
        for needle in ('APP="HOPE"', 'var_os="${var_os:-debian}"', 'var_version="${var_version:-13}"',
                       'var_unprivileged="${var_unprivileged:-1}"', 'var_arm64="${var_arm64:-yes}"',
                       'header_info "$APP"\nvariables\ncolor\ncatch_errors', 'start\nbuild_container\ndescription'):
            self.assertIn(needle, CT)

    def test_update_tracks_the_branch_and_rebuilds(self):
        self.assertIn('check_for_gh_branch "hope" "doomedramen/hope" "main"', CT)
        self.assertIn('fetch_and_deploy_gh_branch "hope" "doomedramen/hope" "main"', CT)
        self.assertNotRegex(CT, r'git (clone|pull)')
        for command in ('systemctl stop hope', 'npm ci', 'npm run build', 'cargo build --release --locked -p hope-server',
                        'install -m 755 target/release/hope-server /usr/local/bin/hope-server', 'systemctl start hope'):
            self.assertIn(command, CT)
        body = CT[CT.index('function update_script'):]
        self.assertRegex(body, r'\n  exit\n\}')

    def test_resources_match_the_metadata(self):
        resources = META['install_methods'][0]['resources']
        self.assertIn(f'var_cpu="${{var_cpu:-{resources["cpu"]}}}"', CT)
        self.assertIn(f'var_ram="${{var_ram:-{resources["ram"]}}}"', CT)
        self.assertIn(f'var_disk="${{var_disk:-{resources["hdd"]}}}"', CT)
        self.assertEqual(META['install_methods'][0]['script'], 'ct/hope.sh')
        self.assertEqual(META['architectures'], ['amd64', 'arm64'])


class InstallScript(unittest.TestCase):
    def test_prologue_and_footer(self):
        self.assertIn('source /dev/stdin <<<"$FUNCTIONS_FILE_PATH"\ncolor\nverb_ip6\ncatch_errors\nsetting_up_container\nnetwork_check\nupdate_os', INSTALL)
        self.assertTrue(INSTALL.rstrip().endswith('motd_ssh\ncustomize\ncleanup_lxc'))
        self.assertEqual(functions(INSTALL), [])

    def test_uses_engine_helpers_not_hand_rolled_equivalents(self):
        self.assertIn('setup_rust', INSTALL)
        self.assertIn('NODE_VERSION="22" setup_nodejs', INSTALL)
        self.assertIn('fetch_and_deploy_gh_branch "hope" "doomedramen/hope" "main"', INSTALL)
        self.assertIn('random_password', INSTALL)
        for banned in ('apt-get', 'git clone', 'rustup.rs', 'nodesource', 'useradd', 'adduser', 'sudo ', '${APPLICATION}', '$APP'):
            self.assertNotIn(banned, INSTALL, banned)

    def test_every_block_is_closed_and_commands_are_quiet(self):
        self.assertEqual(INSTALL.count('msg_info '), INSTALL.count('msg_ok '))
        for line in INSTALL.splitlines():
            if re.match(r'\s*(apt|npm|cargo) ', line):
                self.fail(f'not wrapped in $STD: {line}')

    def test_server_layout_survives_a_branch_update(self):
        # fetch_and_deploy_gh_branch resets /opt/hope to the branch and removes untracked files, so
        # nothing the service needs may live there except build outputs the repository ignores.
        ignored = (ROOT / '.gitignore').read_text().split()
        self.assertIn('/target/', ignored)
        self.assertIn('web/out/', ignored)
        self.assertIn('HOPE_WEB_DIR=/opt/hope/web/out', INSTALL)
        self.assertIn('HOPE_DATA_DIR=/var/lib/hope', INSTALL)
        self.assertIn('HOPE_RELEASE_DIR=/var/lib/hope/releases', INSTALL)
        self.assertIn('ExecStart=/usr/local/bin/hope-server', INSTALL)
        self.assertNotRegex(INSTALL, r'/opt/hope/(bin|release|src|releases)\b')
        self.assertEqual(META['install_methods'][0]['config_path'], '/etc/hope/hope.env')

    def test_tls_and_setup_token(self):
        self.assertIn('tls internal', INSTALL)
        self.assertIn('HOPE_AGENT_CA=/etc/hope/ca.crt', INSTALL)
        self.assertIn('/etc/hope/ca.crt', INSTALL)
        self.assertIn('HOPE_SETUP_TOKEN=$(random_password 32)', INSTALL)
        self.assertIn('chmod 600 /etc/hope/hope.env', INSTALL)
        self.assertLess(INSTALL.index('Configuring Caddy'), INSTALL.index('Creating Service'))


if __name__ == '__main__':
    unittest.main()
