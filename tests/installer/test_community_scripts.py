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
                       'var_unprivileged="${var_unprivileged:-1}"',
                       'header_info "$APP"\nvariables\ncolor\ncatch_errors', 'start\nbuild_container\ndescription'):
            self.assertIn(needle, CT)

    def test_update_stages_a_pinned_release_and_commits_version_after_activation(self):
        self.assertIn('check_for_gh_release "hope" "doomedramen/hope"', CT)
        self.assertIn('release=$CHECK_UPDATE_RELEASE', CT)
        self.assertIn('fetch_and_deploy_gh_release "hope-staged" "doomedramen/hope" "prebuild" "$release" "$staged/server"', CT)
        activate = CT.index('python3 "$activation" install')
        self.assertLess(CT.index('fetch_and_deploy_gh_release "hope-staged"'), activate)
        self.assertGreater(CT.index('cp "$HOME/.hope-staged" "$HOME/.hope"'), activate)
        self.assertIn('--binary /usr/local/bin/hope-server', CT)
        self.assertIn('python3 /var/lib/hope-activation/recover.py recover', CT)
        for compiled in ('setup_rust', 'setup_nodejs', 'cargo build', 'npm ', 'check_for_gh_branch', 'fetch_and_deploy_gh_branch'):
            self.assertNotIn(compiled, CT, compiled)

    def test_proxmox_container_service_gets_raw_socket_capability(self):
        self.assertIn('AmbientCapabilities=CAP_NET_RAW', CT)
        self.assertIn('AmbientCapabilities=CAP_NET_RAW', INSTALL)
        # Startup failure, including capability refusal, is handled by the shared transaction.
        self.assertIn('--candidate-unit "$staged/hope.service"', CT)

    def test_update_preserves_the_earlier_source_build_and_toolchains(self):
        for destructive in ('rm -rf /opt/hope', '"$HOME/.cargo"', '"$HOME/.rustup"', 'apt purge', 'apt autoremove'):
            self.assertNotIn(destructive, CT)
        self.assertIn('cp -p /etc/hope/hope.env "$staged/hope.env"', CT)
        begin = CT.index('[Unit]')
        self.assertEqual(CT[begin:CT.index('EOF', begin)],
                         INSTALL[INSTALL.index('[Unit]'):INSTALL.index('EOF\nsystemctl enable -q --now hope')])

    def test_update_signs_agents_only_after_server_activation(self):
        activate = CT.index('python3 "$activation" install')
        deploy = CT.index('fetch_and_deploy_gh_release "hope-agents"')
        self.assertLess(activate, deploy)
        self.assertIn('"prebuild" "$release" "$staged/agents" "hope-agents.tar.gz"', CT)
        self.assertIn('ensure_dependencies python3 openssl', CT)
        self.assertIn('! -e "$release_dir/current"', CT)
        self.assertIn('--out "$release_dir"', CT)
        self.assertEqual(CT.count('openssl genrsa'), 1)
        self.assertIn('if FORCE_UPDATE=1 CLEAN_INSTALL=1 fetch_and_deploy_gh_release "hope-agents"', CT)

    def test_the_container_is_small(self):
        resources = META['install_methods'][0]['resources']
        self.assertLessEqual(resources['ram'], 512)
        self.assertLessEqual(resources['hdd'], 4)
        self.assertEqual(resources['cpu'], 1)

    def test_resources_match_the_metadata(self):
        resources = META['install_methods'][0]['resources']
        self.assertIn(f'var_cpu="${{var_cpu:-{resources["cpu"]}}}"', CT)
        self.assertIn(f'var_ram="${{var_ram:-{resources["ram"]}}}"', CT)
        self.assertIn(f'var_disk="${{var_disk:-{resources["hdd"]}}}"', CT)
        self.assertEqual(META['install_methods'][0]['script'], 'ct/hope.sh')
        self.assertEqual(META['architectures'], ['amd64'])


class InstallScript(unittest.TestCase):
    def test_prologue_and_footer(self):
        self.assertIn('source /dev/stdin <<<"$FUNCTIONS_FILE_PATH"\ncolor\nverb_ip6\ncatch_errors\nsetting_up_container\nnetwork_check\nupdate_os', INSTALL)
        self.assertTrue(INSTALL.rstrip().endswith('motd_ssh\ncustomize\ncleanup_lxc'))
        self.assertEqual(functions(INSTALL), [])

    def test_uses_engine_helpers_not_hand_rolled_equivalents(self):
        self.assertIn('fetch_and_deploy_gh_release "hope" "doomedramen/hope" "prebuild" "latest" "/opt/hope" '
                      '"hope-server-x86_64-linux.tar.gz"', INSTALL)
        self.assertIn('random_password', INSTALL)
        for banned in ('apt-get', 'git clone', 'rustup.rs', 'nodesource', 'useradd', 'adduser', 'sudo ', '${APPLICATION}', '$APP',
                       'setup_rust', 'setup_nodejs', 'cargo ', 'npm ', 'curl '):
            self.assertNotIn(banned, INSTALL, banned)

    def test_every_block_is_closed_and_commands_are_quiet(self):
        self.assertEqual(INSTALL.count('msg_info '), INSTALL.count('msg_ok '))
        for line in INSTALL.splitlines():
            if re.match(r'\s*(apt|npm|cargo) ', line):
                self.fail(f'not wrapped in $STD: {line}')

    def test_server_layout_matches_the_release_package(self):
        # The package (scripts/package-server.sh) holds hope-server and web/ at its top level.
        package = (ROOT / 'scripts' / 'package-server.sh').read_text()
        self.assertIn('"$staging/hope-server-$version/hope-server"', package)
        self.assertIn('"$staging/hope-server-$version/web"', package)
        self.assertIn('HOPE_WEB_DIR=/opt/hope/web', INSTALL)
        self.assertIn('HOPE_DATA_DIR=/var/lib/hope', INSTALL)
        self.assertIn('HOPE_RELEASE_DIR=/var/lib/hope/releases', INSTALL)
        self.assertIn('ExecStart=/usr/local/bin/hope-server', INSTALL)
        self.assertNotRegex(INSTALL, r'/opt/hope/(bin|release|src|releases)\b')
        self.assertEqual(META['install_methods'][0]['config_path'], '/etc/hope/hope.env')
        workflow = (ROOT / '.github' / 'workflows' / 'server-build.yml').read_text()
        self.assertIn('--latest', workflow)
        self.assertIn('hope-server-*-linux.tar.gz', workflow)

    def test_agents_are_signed_with_a_new_server_key(self):
        self.assertIn('fetch_and_deploy_gh_release "hope-agents" "doomedramen/hope" "prebuild" "latest" "/opt/hope-agents" '
                      '"hope-agents.tar.gz"', INSTALL)
        self.assertIn('apt install -y caddy openssl', INSTALL)
        self.assertIn('(umask 077 && openssl genrsa -out /etc/hope/release-signing.pem 3072 2>/dev/null)', INSTALL)
        self.assertIn('$STD env HOPE_SIGNING_KEY=/etc/hope/release-signing.pem sh /opt/hope/tools/assemble-release.sh '
                      '--staging /opt/hope-agents --version "$(cat /opt/hope/VERSION)" --out /var/lib/hope/releases', INSTALL)
        self.assertLess(INSTALL.index('Configuring H.O.P.E'), INSTALL.index('Signing Agent Release'))
        self.assertLess(INSTALL.index('Signing Agent Release'), INSTALL.index('Creating Service'))
        # The package carries what the pair runs.
        package = (ROOT / 'scripts' / 'package-server.sh').read_text()
        self.assertIn('scripts/assemble-release.sh "$staging/hope-server-$version/tools/assemble-release.sh"', package)
        self.assertIn('scripts/activate-server.py "$staging/hope-server-$version/tools/activate-server.py"', package)
        self.assertIn('> "$staging/hope-server-$version/VERSION"', package)

    def test_releases_carry_the_unsigned_agents(self):
        for name in ('server-build.yml', 'release.yml'):
            workflow = (ROOT / '.github' / 'workflows' / name).read_text()
            self.assertIn('-czf dist/server/hope-agents.tar.gz .', workflow, name)
            self.assertIn('scripts/assemble-server-release.sh --staging dist/server-builds --out dist/server', workflow, name)
            self.assertIn('dist/server/hope-agents.tar.gz dist/server/SHA256SUMS-server', workflow, name)
        workflow = (ROOT / '.github' / 'workflows' / 'server-build.yml').read_text()
        self.assertIn('uses: ./.github/workflows/build-agent.yml', workflow)
        self.assertIn('needs: [version, build, agents]', workflow)
        self.assertNotIn('secrets.', workflow, 'server builds hold no signing key')

    def test_tls_and_setup_token(self):
        self.assertIn('tls internal', INSTALL)
        self.assertIn('HOPE_AGENT_CA=/etc/hope/ca.crt', INSTALL)
        self.assertIn('/etc/hope/ca.crt', INSTALL)
        self.assertIn('HOPE_SETUP_TOKEN=$(random_password 32)', INSTALL)
        self.assertIn('chmod 600 /etc/hope/hope.env', INSTALL)
        self.assertLess(INSTALL.index('Configuring Caddy'), INSTALL.index('Creating Service'))


    def test_builds_have_their_own_version(self):
        workflow = (ROOT / '.github' / 'workflows' / 'server-build.yml').read_text()
        self.assertIn('echo "version=${crate%.*}.${RUN}"', workflow)
        self.assertEqual(workflow.count('HOPE_BUILD_VERSION: ${{ needs.version.outputs.version }}'), 1)
        self.assertIn('version: ${{ needs.version.outputs.version }}', workflow)
        self.assertIn('gh release create "v${VERSION}"', workflow)
        agent = (ROOT / '.github' / 'workflows' / 'build-agent.yml').read_text()
        self.assertIn('HOPE_BUILD_VERSION: ${{ inputs.version }}', agent)
        main = (ROOT / 'crates' / 'hope-agent' / 'src' / 'main.rs').read_text()
        self.assertIn('option_env!("HOPE_BUILD_VERSION")', main)


if __name__ == '__main__':
    unittest.main()
