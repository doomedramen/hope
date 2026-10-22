"""The /install starter (scripts/install-starter.sh): `curl -fsS <server>/install | sh -s -- <token>`.

It runs from a pipe, fetches the full installer through the harness's fake curl and runs it from a
file, so everything the installer does from a file (the updater copy, sudo re-runs) keeps working.
"""
import subprocess
import unittest

import harness
from harness import Env, TOKEN

STARTER = harness.REPO / 'scripts' / 'install-starter.sh'


class Starter(unittest.TestCase):
    def setUp(self):
        self.env = Env()
        self.addCleanup(self.env.close)

    def serve(self):
        """What the server serves at /agent/install and /agent/uninstall for the current release."""
        self.env.set('served/install', self.env.invocation_installer().read_text())
        self.env.set('served/uninstall', harness.INSTALLER.read_text().replace('@@HOPE_PUBLIC_KEY@@', ''))

    def starter(self, ca="''"):
        return STARTER.read_text().replace('@@HOPE_SERVER@@', "'" + harness.SERVER + "'").replace('@@HOPE_AGENT_CA@@', ca)

    def pipe(self, *arguments, text=None, uid=0, **extra):
        self.serve()
        environment = self.env._environment(uid, None, None, None, extra)
        done = subprocess.run(['sh', '-s', '--', *arguments], input=text if text is not None else self.starter(),
                              env=environment, capture_output=True, text=True, timeout=120)
        return harness.Result(done.returncode, done.stdout, done.stderr)

    def leftovers(self):
        return [path.name for path in self.env.tmp.iterdir()]

    def test_a_token_installs_the_agent_and_its_updater_from_a_file(self):
        result = self.pipe(TOKEN)
        self.assertEqual(0, result.code, result.err)
        self.assertIn('result=installed v1', result.err)
        # The updater keeps a copy of the installer, which needs it to have run from a file.
        self.assertTrue((self.env.root / 'var/lib/hope-agent-update/install-agent.sh').is_file())
        self.assertTrue((self.env.root / 'usr/local/sbin/hope-agent-update').is_file())
        self.assertEqual([], self.leftovers())
        self.assertEqual([], self.env.assert_no_secret())

    def test_upgrade_changes_nothing_unless_the_release_is_newer(self):
        self.assertEqual(0, self.pipe(TOKEN).code)
        same = self.pipe('upgrade')
        self.assertEqual(0, same.code, same.err)
        self.assertIn('result=current v1', same.err)
        self.env.publish('v2')
        newer = self.pipe('upgrade')
        self.assertEqual(0, newer.code, newer.err)
        self.assertIn('result=updated v2', newer.err)

    def test_uninstall_removes_the_agent(self):
        self.assertEqual(0, self.pipe(TOKEN).code)
        result = self.pipe('uninstall')
        self.assertEqual(0, result.code, result.err)
        self.assertFalse(self.env.paths('system')['binary'].exists())
        self.assertEqual([], self.leftovers())

    def test_a_download_cut_off_part_way_runs_nothing(self):
        text = self.starter()
        cut = text[:text.rindex('main "$@"')]
        result = self.pipe(TOKEN, text=cut)
        self.assertEqual(0, result.code)
        self.assertNotIn('curl', self.env.log())
        self.assertFalse(self.env.paths('system')['binary'].exists())

    def test_nothing_to_do_says_how_to_use_it(self):
        result = self.pipe()
        self.assertEqual(2, result.code)
        self.assertIn('Usage:', result.err)
        self.assertNotIn('curl', self.env.log())

    def test_a_custom_ca_is_used_for_the_download_and_passed_on(self):
        ca = self.env.base / 'ca.pem'
        ca.write_text('CA')
        result = self.pipe(TOKEN, text=self.starter(ca="'" + str(ca) + "'"))
        self.assertEqual(0, result.code, result.err)
        first = next(line for line in self.env.log().splitlines() if line.startswith('curl '))
        self.assertIn('--cacert ' + str(ca), first)
        self.assertEqual(str(self.env.root / 'var/lib/hope-agent-update/ca.pem'),
                         (self.env.root / 'var/lib/hope-agent-update/ca').read_text().strip())


if __name__ == '__main__':
    unittest.main()
