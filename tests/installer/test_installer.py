"""Installer behaviour tests. Run: python3 -I -m unittest discover -s tests/installer -v"""
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import harness  # noqa: E402
from harness import Env, TOKEN  # noqa: E402

PHASES = ['preflight', 'stage', 'verify', 'stop', 'place-binary', 'configure', 'write-unit', 'activate', 'wait-report', 'complete']


def setUpModule():
    harness.keys()


def tearDownModule():
    harness.cleanup_keys()


class Base(unittest.TestCase):
    def new_env(self, *args, **kwargs):
        env = Env(*args, **kwargs)
        self.addCleanup(env.close)
        self.addCleanup(lambda: self.assertEqual([], env.assert_no_secret()))
        return env

    def setUp(self):
        self.env = self.new_env()

    def ok(self, result):
        self.assertEqual(0, result.code, result.err)

    def failed(self, result, code):
        self.assertNotEqual(0, result.code, result.err)
        self.assertIn(code, result.errors, result.err)

    def untouched(self, env):
        """No service-manager mutation and nothing installed."""
        log = env.log()
        for word in ('stop', 'enable', 'disable', 'start', 'bootstrap', 'bootout'):
            self.assertNotRegex(log, r'(systemctl\[\w+\]|launchctl) %s\b' % word)
        self.assertEqual({}, env.snapshot(env.root))

    def version(self, path):
        return path.read_text().split('VERSION=')[1].split()[0]


class Linux(Base):
    def test_proxmox_system_install_adds_a_root_owned_collector_timer(self):
        pvesh = self.env.bin / 'pvesh'
        pvesh.write_text('#!/bin/sh\nexit 0\n')
        pvesh.chmod(0o755)
        result = self.env.run()
        self.ok(result)
        service = self.env.root / 'etc/systemd/system/hope-agent-proxmox-collect.service'
        timer = self.env.root / 'etc/systemd/system/hope-agent-proxmox-collect.timer'
        self.assertTrue(service.exists())
        self.assertTrue(timer.exists())
        unit = service.read_text()
        self.assertIn('User=root', unit)
        self.assertIn('collect-proxmox', unit)
        self.assertIn('ReadWritePaths=/run/hope-agent /etc/hope-agent', unit)
        self.assertIn('/etc/pve/priv/hope-agent /etc/pve/priv/lock', unit)
        self.assertIn('TimeoutStartSec=45s', unit)
        self.assertIn('RuntimeDirectory=hope-agent', unit)
        self.assertEqual((self.env.root / 'etc/hope-agent').stat().st_mode & 0o777, 0o750)
        self.assertIn('OnUnitActiveSec=30s', timer.read_text())
        self.assertIn('systemctl[system] enable --now hope-agent-proxmox-collect.timer', self.env.log())

    def test_root_install(self):
        result = self.env.run()
        self.ok(result)
        self.assertEqual(PHASES, [p for p in result.phases if p in PHASES])
        paths = self.env.paths('system')
        self.assertEqual('v1', self.version(paths['binary']))
        unit = paths['unit'].read_text()
        self.assertRegex(unit, r'Environment="HOPE_ACTIVATION_ID=[0-9a-f-]{36}"')
        self.assertIn('User=hope-agent', unit)
        self.assertIn('ProtectSystem=strict', unit)
        self.assertTrue((paths['state'] / 'config.json').exists())
        self.assertFalse(self.env.txn().exists())
        self.assertIn('accepted report', result.out)
        self.assertIn('systemctl[system] enable --now hope-agent', self.env.log())
        self.assertIn('useradd --system', self.env.log())

    def test_nonroot_with_working_sudo_reexecs_for_system_service(self):
        result = self.env.run(uid=1000)
        self.ok(result)
        self.assertIn('sudo -n --preserve-env=', self.env.log())
        self.assertIn('HOPE_PURGE_IDENTITY', self.env.log())
        self.assertTrue(self.env.paths('system')['unit'].exists())
        self.assertFalse(self.env.paths('user')['unit'].exists())

    def test_sudo_unavailable_auto_falls_back_to_user_service(self):
        self.env.fault('sudo-fail')
        result = self.env.run(uid=1000)
        self.ok(result)
        self.assertIn('sudo is unavailable', result.err)
        self.assertIn('lingering', result.err)
        self.assertTrue(self.env.paths('user')['unit'].exists())
        self.assertEqual({}, self.env.snapshot(self.env.root))
        self.assertIn('systemctl[user] enable --now', self.env.log())
        self.assertNotIn('systemctl[system]', self.env.log().replace('systemctl[system] is-system-running', ''))

    def test_sudo_unavailable_with_explicit_system_mode(self):
        self.env.fault('sudo-fail')
        result = self.env.run(uid=1000, mode='system')
        self.failed(result, 'SUDO_UNAVAILABLE')
        self.untouched(self.env)

    def test_user_service_on_systemd(self):
        result = self.env.run(uid=1000, mode='user')
        self.ok(result)
        paths = self.env.paths('user')
        self.assertTrue(paths['unit'].exists())
        self.assertNotIn('User=', paths['unit'].read_text())
        self.assertNotIn('sudo', self.env.log())
        self.assertEqual({}, self.env.snapshot(self.env.root))

    def test_ssh_style_user_service_without_lingering_is_refused_before_any_change(self):
        for linger in ('no', 'missing'):
            self.env.set('fake/linger', linger)
            result = self.env.run(uid=1000, mode='user', HOPE_REQUIRE_PERSISTENT_SERVICE=1)
            self.failed(result, 'LINGER_REQUIRED')
            self.assertIn('enable-linger', result.err)
            paths = self.env.paths('user')
            self.assertFalse(paths['binary'].exists() or paths['unit'].exists() or paths['state'].exists())
            self.assertEqual({}, self.env.snapshot(self.env.root))
            self.assertNotIn('systemctl', self.env.log())
            self.assertNotIn('curl', self.env.log())

    def test_ssh_style_user_service_with_lingering_installs(self):
        self.env.set('fake/linger', 'yes')
        result = self.env.run(uid=1000, mode='user', HOPE_REQUIRE_PERSISTENT_SERVICE=1)
        self.ok(result)
        self.assertNotIn('USER_SERVICE_NOT_PERSISTENT', result.err)
        self.assertTrue(self.env.paths('user')['unit'].exists())

    def test_linger_marker_file_counts_as_persistent(self):
        self.env.set('fake/linger', 'missing')
        self.env.set('root/var/lib/systemd/linger/1000')
        result = self.env.run(uid=1000, mode='user', HOPE_REQUIRE_PERSISTENT_SERVICE=1)
        self.ok(result)

    def test_manual_user_service_without_lingering_installs_with_notice(self):
        self.env.set('fake/linger', 'no')
        result = self.env.run(uid=1000, mode='user')
        self.ok(result)
        self.assertIn('hope-install: notice=USER_SERVICE_NOT_PERSISTENT', result.err)
        self.assertTrue(self.env.paths('user')['unit'].exists())

    def test_require_persistent_does_not_affect_system_service(self):
        self.env.set('fake/linger', 'no')
        self.ok(self.env.run(HOPE_REQUIRE_PERSISTENT_SERVICE=1))

    def test_no_running_user_manager(self):
        self.env.set('fake/manager-user', 'offline')
        result = self.env.run(uid=1000, mode='user')
        self.failed(result, 'NO_SERVICE_MANAGER')
        self.assertEqual({}, self.env.snapshot(self.env.home))

    def test_system_manager_must_be_running_not_just_installed(self):
        self.env.set('fake/manager-system', 'offline')
        self.failed(self.env.run(), 'NO_SERVICE_MANAGER')
        self.untouched(self.env)
        self.env.set('fake/manager-system', 'degraded')
        self.ok(self.env.run())

    def test_unsupported_architecture(self):
        for architecture in ('aarch64', 'arm64', 'riscv64'):
            with self.subTest(architecture=architecture):
                self.env.set('fake/arch', architecture)
                self.failed(self.env.run(), 'UNSUPPORTED_ARCH')
                self.untouched(self.env)

    def test_unsupported_platform(self):
        self.env.set('fake/os', 'FreeBSD')
        self.failed(self.env.run(), 'UNSUPPORTED_PLATFORM')

    def test_macos_install_is_rejected_but_legacy_install_can_be_removed(self):
        install_env = self.new_env('Darwin', 'arm64')
        self.failed(install_env.run(), 'UNSUPPORTED_PLATFORM')
        self.assertEqual({}, install_env.snapshot(install_env.root))
        legacy_env = self.new_env('Darwin', 'arm64')
        paths = legacy_env.preinstall()
        self.ok(legacy_env.run('uninstall', token=None, server=None))
        self.assertFalse(paths['binary'].exists())

    def test_existing_identity_for_another_server_is_rejected_before_stopping(self):
        paths = self.env.preinstall(server='https://other.test')
        result = self.env.run()
        self.failed(result, 'EXISTING_IDENTITY_MISMATCH')
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertTrue((self.env.base / 'service/system.active').exists())
        self.assertNotIn('systemctl[system] stop', self.env.log())

    def test_malformed_and_insecure_urls(self):
        cases = {
            'http://hope.test': 'INSECURE_URL',
            'hope.test': 'MALFORMED_URL',
            'https://': 'MALFORMED_URL',
            'https://:8443': 'MALFORMED_URL',
            'https://hope test': 'MALFORMED_URL',
            "https://ho'pe.test": 'MALFORMED_URL',
            'https://u:p@hope.test': 'MALFORMED_URL',
        }
        for url, code in cases.items():
            with self.subTest(url=url):
                self.failed(self.env.run(server=url), code)
        self.untouched(self.env)

    def test_missing_ca_file(self):
        self.failed(self.env.run(HOPE_CA_CERT=self.env.base / 'missing.pem'), 'CA_UNREADABLE')
        self.untouched(self.env)

    def test_tls_verification_flags_are_kept(self):
        ca = self.env.base / 'ca.pem'
        ca.write_text('CA')
        self.ok(self.env.run(HOPE_CA_CERT=ca))
        curls = [line for line in self.env.log().splitlines() if line.startswith('curl ')]
        self.assertTrue(curls)
        for line in curls:
            self.assertIn(f"--cacert {ca}", line)
            self.assertIn("--proto =https --proto-redir =https", line)
            self.assertNotIn('--insecure', line)
            self.assertNotIn(' -k ', line)

    def test_bad_signature_never_executes_download(self):
        self.env.publish('v1', key='other')
        result = self.env.run()
        self.failed(result, 'BAD_SIGNATURE')
        self.assertNotIn('agent[', self.env.log())
        self.untouched(self.env)

    def test_placeholder_key_is_rejected(self):
        self.env.installer.write_text(harness.INSTALLER.read_text())
        self.failed(self.env.run(), 'BAD_SIGNATURE')
        self.assertNotIn('agent[', self.env.log())

    def test_download_failures(self):
        for name in ('hope-agent-linux-x86_64', 'hope-agent-linux-x86_64.sig'):
            with self.subTest(name=name):
                self.env.fault('curl-fail-' + name)
                self.failed(self.env.run(), 'DOWNLOAD_FAILED')
                (self.env.base / 'fault' / ('curl-fail-' + name)).unlink()
        self.untouched(self.env)

    def test_unreachable_server(self):
        self.env.fault('curl-unreachable')
        self.failed(self.env.run(), 'DOWNLOAD_FAILED')
        self.untouched(self.env)

    def test_release_not_packaged_for_platform(self):
        paths = self.env.preinstall()
        self.env.unpublish()
        self.failed(self.env.run(), 'RELEASE_UNAVAILABLE')
        self.assertNotIn('systemctl[system] stop', self.env.log())
        self.assertTrue(paths['binary'].exists())

    def test_token_expired_or_reused_rolls_back(self):
        self.env.fault('configure')
        result = self.env.run()
        self.failed(result, 'CONFIGURE_FAILED')
        self.assertIn('ROLLED_BACK', result.errors)
        paths = self.env.paths('system')
        self.assertFalse(paths['binary'].exists())
        self.assertFalse(paths['unit'].exists())
        self.assertFalse(self.env.txn().exists())

    def test_failed_service_start_restores_previous_and_restarts_it(self):
        paths = self.env.preinstall()
        old_unit = paths['unit'].read_text()
        old_config = (paths['state'] / 'config.json').read_text()
        self.env.publish('v2')
        self.env.fault('enable-fail')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'SERVICE_START_FAILED')
        self.assertIn('ROLLED_BACK', result.errors)
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertEqual(old_unit, paths['unit'].read_text())
        self.assertIn('settings=old-v1', (paths['state'] / 'config.json').read_text())
        self.assertIn('credential=' + harness.CREDENTIAL, (paths['state'] / 'config.json').read_text())
        self.assertIn('agent_id=agent-1', old_config)
        self.assertIn('systemctl[system] start hope-agent', self.env.log())
        self.assertIn('00000000-0000-4000-8000-000000000000', (self.env.base / 'service/system.active').read_text())
        self.assertFalse(self.env.txn().exists())

    def test_failed_first_report_rolls_back(self):
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.env.fault('no-report')
        result = self.env.run('upgrade', token=None, HOPE_FIRST_REPORT_TIMEOUT=3, HOPE_POLL_INTERVAL=1)
        self.failed(result, 'FIRST_REPORT_TIMEOUT')
        self.assertIn('ROLLED_BACK', result.errors)
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertIn('OLD-UNIT', paths['unit'].read_text())
        self.assertIn('systemctl[system] start hope-agent', self.env.log())
        self.assertNotIn('complete', result.phases)

    def test_fresh_failed_first_report_leaves_no_service(self):
        self.env.fault('no-report')
        result = self.env.run(HOPE_FIRST_REPORT_TIMEOUT=2, HOPE_POLL_INTERVAL=1)
        self.failed(result, 'FIRST_REPORT_TIMEOUT')
        paths = self.env.paths('system')
        self.assertFalse(paths['binary'].exists())
        self.assertFalse(paths['unit'].exists())
        self.assertFalse((self.env.base / 'service/system.active').exists())

    def test_reinstall_same_version_converges(self):
        self.ok(self.env.run())
        paths = self.env.paths('system')
        config = (paths['state'] / 'config.json').read_text()
        self.ok(self.env.run(token=None))
        self.assertEqual(config, (paths['state'] / 'config.json').read_text())
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertEqual([], [p.name for p in paths['binary'].parent.iterdir() if p.name != 'hope-agent'])
        self.assertFalse(self.env.txn().exists())

    def test_upgrade_keeps_identity_and_discards_backup(self):
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.ok(self.env.run('upgrade', token=None))
        self.assertEqual('v2', self.version(paths['binary']))
        config = (paths['state'] / 'config.json').read_text()
        self.assertIn('agent_id=agent-1', config)
        self.assertIn('credential=' + harness.CREDENTIAL, config)
        self.assertIn('settings=v2', config)
        self.assertNotIn('OLD-UNIT', paths['unit'].read_text())
        self.assertFalse(self.env.txn().exists())
        log = self.env.log()
        self.assertLess(log.index('systemctl[system] stop'), log.index('systemctl[system] enable --now'))

    def test_uninstall_keeps_identity(self):
        self.ok(self.env.run())
        result = self.env.run('uninstall', token=None, server=None)
        self.ok(result)
        paths = self.env.paths('system')
        self.assertFalse(paths['binary'].exists())
        self.assertFalse(paths['unit'].exists())
        self.assertTrue((paths['state'] / 'config.json').exists())
        self.assertIn('Identity retained', result.out)

    def test_uninstall_purge_removes_identity(self):
        self.ok(self.env.run())
        result = self.env.run('uninstall', token=None, server=None, HOPE_PURGE_IDENTITY=1)
        self.ok(result)
        self.assertFalse(self.env.paths('system')['state'].exists())
        self.assertIn('Revoke', result.out)

    def test_root_cannot_target_user_mode_and_user_mode_leaves_system_alone(self):
        system = self.env.preinstall('system')
        before = self.env.snapshot(self.env.root)
        self.failed(self.env.run(mode='user'), 'INVALID_MODE')
        self.failed(self.env.run('uninstall', mode='user', token=None, server=None), 'INVALID_MODE')
        self.assertEqual(before, self.env.snapshot(self.env.root))
        self.ok(self.env.run(uid=1000, mode='user'))
        self.assertEqual(before, self.env.snapshot(self.env.root))
        self.assertTrue(self.env.paths('user')['unit'].exists())
        self.assertTrue((self.env.base / 'service/system.active').exists())
        self.assertNotRegex(self.env.log(), r'systemctl\[system\] (stop|enable|disable|start|daemon-reload)')
        self.ok(self.env.run('uninstall', uid=1000, mode='user', token=None, server=None))
        self.assertEqual(before, self.env.snapshot(self.env.root))
        self.assertTrue(system['binary'].exists())
        self.assertFalse(self.env.paths('user')['unit'].exists())

    def test_auto_mode_detects_existing_install_and_refuses_ambiguity(self):
        self.env.preinstall('user', active=False)
        self.ok(self.env.run('uninstall', uid=1000, token=None, server=None))
        self.assertFalse(self.env.paths('user')['binary'].exists())

    def test_auto_mode_refuses_when_both_modes_are_installed(self):
        self.env.preinstall('user', active=False)
        self.env.preinstall('system', active=False)
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'EXISTING_INSTALL_MODE')
        self.untouched_services()

    def untouched_services(self):
        self.assertNotRegex(self.env.log(), r'systemctl\[\w+\] (stop|enable|disable)')

    def test_signals_during_each_mutation_phase_roll_back_and_rerun_converges(self):
        for phase in ('stop', 'place-binary', 'configure', 'write-unit', 'activate', 'wait'):
            with self.subTest(phase=phase):
                env = self.new_env()
                paths = env.preinstall()
                old_unit = paths['unit'].read_text()
                env.publish('v2')
                env.fault('signal-at-' + phase)
                result = env.run('upgrade', token=None)
                self.assertNotEqual(0, result.code, result.err)
                self.assertIn('INTERRUPTED', result.errors, result.err)
                self.assertIn('ROLLED_BACK', result.errors, result.err)
                self.assertEqual('v1', self.version(paths['binary']))
                self.assertEqual(old_unit, paths['unit'].read_text())
                config = (paths['state'] / 'config.json').read_text()
                self.assertIn('settings=old-v1', config)
                self.assertIn('credential=' + harness.CREDENTIAL, config)
                self.assertIn('00000000-0000-4000-8000-000000000000', (env.base / 'service/system.active').read_text())
                self.assertIn('systemctl[system] start hope-agent', env.log())
                self.assertFalse(env.txn().exists())
                again = env.run('upgrade', token=None)
                self.ok(again)
                self.assertEqual('v2', self.version(paths['binary']))
                self.assertFalse(env.txn().exists())

    def test_closed_output_pipe_mid_transaction_rolls_back(self):
        # A dropped SSH session (or a closed pipe) must not strand the host with the old service stopped:
        # SIGPIPE on the next progress line used to kill the installer without running its rollback.
        paths = self.env.preinstall()
        old_unit = paths['unit'].read_text()
        self.env.publish('v2')
        self.env.fault('stall-at-configure')
        result = self.env.run_with_closed_output('upgrade', token=None)
        self.assertNotEqual(0, result.code)
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertEqual(old_unit, paths['unit'].read_text())
        self.assertIn('00000000-0000-4000-8000-000000000000', (self.env.base / 'service/system.active').read_text())
        self.assertFalse(self.env.txn().exists())
        self.ok(self.env.run('upgrade', token=None))
        self.assertEqual('v2', self.version(paths['binary']))

    def test_signal_during_fresh_install_leaves_nothing_running(self):
        self.env.fault('signal-at-activate')
        result = self.env.run()
        self.assertIn('INTERRUPTED', result.errors)
        paths = self.env.paths('system')
        self.assertFalse(paths['binary'].exists())
        self.assertFalse(paths['unit'].exists())
        self.assertFalse((self.env.base / 'service/system.active').exists())
        self.ok(self.env.run(token=None))

    def test_leftover_transaction_after_hard_kill_is_rolled_back_first(self):
        for phase in ('stop', 'place-binary', 'configure', 'write-unit', 'activate', 'wait'):
            with self.subTest(phase=phase):
                env = self.new_env()
                paths = env.preinstall()
                env.publish('v2')
                env.fault('sigkill-at-' + phase)
                killed = env.run('upgrade', token=None)
                self.assertNotEqual(0, killed.code)
                self.assertTrue((env.txn() / 'journal').exists())
                rerun = env.run('upgrade', token=None)
                self.ok(rerun)
                self.assertIn('recover', rerun.phases)
                self.assertIn('interrupted earlier run was rolled back', rerun.err)
                self.assertLess(rerun.phases.index('recover'), rerun.phases.index('stop'))
                self.assertEqual('v2', self.version(paths['binary']))
                self.assertIn('settings=v2', (paths['state'] / 'config.json').read_text())
                self.assertFalse(env.txn().exists())
                self.assertNotIn('00000000-0000-4000', (env.base / 'service/system.active').read_text())

    def test_service_account_must_be_valid(self):
        (self.env.base / 'fake/accounts.json').write_text(json.dumps({'users': {'hope-agent': {'UniqueID': '0', 'PrimaryGroupID': '0', 'UserShell': '/bin/bash', 'NFSHomeDirectory': '/root'}}, 'groups': {}}))
        self.failed(self.env.run(), 'SERVICE_ACCOUNT_INVALID')
        self.untouched_services()

    def good_account(self, members='', group='hope-agent'):
        state = str(self.env.paths('system')['state'])
        (self.env.base / 'fake/accounts.json').write_text(json.dumps({
            'users': {'hope-agent': {'UniqueID': '900', 'PrimaryGroupID': '900', 'UserShell': '/usr/sbin/nologin', 'NFSHomeDirectory': state}},
            'groups': {group: {'PrimaryGroupID': '900', 'GroupMembership': members}}}))

    def test_service_account_group_must_be_dedicated(self):
        self.good_account()
        self.ok(self.env.run())
        for kwargs in ({'members': 'hope-agent,mallory'}, {'group': 'staff'}):
            with self.subTest(**kwargs):
                env = self.new_env()
                self.env = env
                self.good_account(**kwargs)
                self.failed(env.run(), 'SERVICE_ACCOUNT_INVALID')
                self.assertFalse(env.paths('system')['binary'].exists())

    def test_created_directories_are_world_searchable(self):
        self.ok(self.env.run())
        for relative in ('usr/local/bin', 'etc/systemd/system', 'var/lib'):
            self.assertEqual(0o755, (self.env.root / relative).stat().st_mode & 0o777, relative)
        self.assertEqual(0o700, self.env.paths('system')['state'].stat().st_mode & 0o777)
        user = self.new_env()
        self.ok(user.run(uid=1000, mode='user'))
        for relative in ('.local/bin', '.config/systemd/user'):
            self.assertEqual(0o755, (user.home / relative).stat().st_mode & 0o777, relative)

    def test_transaction_is_outside_service_writable_state(self):
        self.env.fault('no-report')
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.env.fault('sigkill-at-activate')
        self.env.run('upgrade', token=None)
        self.assertTrue(self.env.txn().is_dir())
        self.assertNotIn(paths['state'], self.env.txn().parents)
        self.assertNotIn(str(self.env.txn()), ' '.join(l for l in self.env.log().splitlines() if l.startswith('chown')))

    def test_symlinked_state_or_config_is_refused(self):
        paths = self.env.preinstall()
        target = self.env.base / 'elsewhere.json'
        target.write_text('x')
        (paths['state'] / 'config.json').unlink()
        (paths['state'] / 'config.json').symlink_to(target)
        self.failed(self.env.run('upgrade', token=None), 'PATH_UNWRITABLE')
        self.assertNotIn('agent[', self.env.log())
        self.failed(self.env.run('uninstall', token=None, server=None, HOPE_PURGE_IDENTITY=1), 'PATH_UNWRITABLE')

    def test_sudo_probe_uses_preserve_env(self):
        self.env.fault('sudo-no-preserve')
        self.ok(self.env.run(uid=1000))
        self.assertTrue(self.env.paths('user')['unit'].exists())
        self.assertIn('sudo -n --preserve-env=', self.env.log().splitlines()[0] + ''.join(l for l in self.env.log().splitlines() if l.startswith('sudo')))
        env = self.new_env()
        env.fault('sudo-no-preserve')
        self.failed(env.run(uid=1000, mode='system'), 'SUDO_UNAVAILABLE')

    def test_purge_removes_only_agent_files(self):
        custom = self.env.base / 'shared-dir'
        custom.mkdir()
        (custom / 'unrelated.txt').write_text('keep')
        self.ok(self.env.run(HOPE_AGENT_DIR=custom))
        self.assertTrue((custom / 'config.json').exists())
        self.ok(self.env.run('uninstall', token=None, server=None, HOPE_AGENT_DIR=custom, HOPE_PURGE_IDENTITY=1))
        self.assertEqual(['unrelated.txt'], [p.name for p in custom.iterdir()])

    def test_uninstall_keeps_files_without_running_service_manager(self):
        self.ok(self.env.run())
        self.env.set('fake/manager-system', 'offline')
        result = self.env.run('uninstall', token=None, server=None)
        self.failed(result, 'NO_SERVICE_MANAGER')
        self.assertTrue(self.env.paths('system')['binary'].exists())
        self.assertTrue(self.env.paths('system')['unit'].exists())

    def test_uninstall_does_not_remove_a_service_that_failed_to_stop(self):
        self.ok(self.env.run())
        self.env.fault('stop-fail')
        self.failed(self.env.run('uninstall', token=None, server=None), 'UNINSTALL_STOP_FAILED')
        self.assertTrue(self.env.paths('system')['binary'].exists())
        self.assertTrue(self.env.paths('system')['unit'].exists())

    def test_uninstall_refuses_while_an_updater_holds_the_install_lock(self):
        import fcntl
        self.ok(self.env.run())
        with open(self.env.root / 'var/lib/hope-agent-update/update.lock', 'a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.failed(self.env.run('uninstall', token=None, server=None), 'INSTALL_BUSY')
        self.assertTrue(self.env.paths('system')['binary'].exists())
        self.assertTrue(self.env.paths('system')['unit'].exists())

    def test_curl_ignores_user_config_and_ambient_ca_variables(self):
        self.ok(self.env.run(CURL_CA_BUNDLE='/evil', SSL_CERT_FILE='/evil', SSL_CERT_DIR='/evil'))
        self.assertNotIn('curl-env-leak', self.env.log())
        for line in self.env.log().splitlines():
            if line.startswith('curl '):
                self.assertTrue(line.startswith('curl -q '), line)

    def test_only_http_404_means_release_unavailable(self):
        self.env.set('fault/head-status', '403')
        self.failed(self.env.run(), 'DOWNLOAD_FAILED')
        self.env.set('fault/head-status', '404')
        self.failed(self.env.run(), 'RELEASE_UNAVAILABLE')

    def test_unrunnable_verified_binary_is_not_reported_as_bad_signature(self):
        self.env.fault('version-fail')
        result = self.env.run()
        self.failed(result, 'BINARY_NOT_EXECUTABLE')
        self.assertNotIn('BAD_SIGNATURE', result.errors)
        self.untouched(self.env)

    def test_leftover_transaction_in_other_mode_is_reported(self):
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.env.fault('sigkill-at-activate')
        self.env.run('upgrade', token=None)
        result = self.env.run(uid=1000, mode='user')
        self.failed(result, 'EXISTING_INSTALL_MODE')
        self.assertTrue(self.env.txn().exists())

    def test_signal_during_rollback_cannot_interrupt_it(self):
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.env.fault('enable-fail')
        self.env.fault('signal-at-start')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'SERVICE_START_FAILED')
        self.assertIn('ROLLED_BACK', result.errors)
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertFalse(self.env.txn().exists())

    def test_broken_backup_keeps_transaction_for_rerun(self):
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.env.fault('enable-fail')
        self.env.set('fault/delete-at-activate', str(self.env.txn() / 'binary.prev'))
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'ROLLBACK_INCOMPLETE')
        self.assertNotIn('ROLLED_BACK', result.errors)
        self.assertTrue((self.env.txn() / 'journal').exists())

    def test_no_secret_in_any_argv_or_service_definition(self):
        self.ok(self.env.run())
        self.assertNotIn(TOKEN, self.env.log())
        self.assertNotIn(harness.CREDENTIAL, self.env.paths('system')['unit'].read_text())


class UninstallIdentity(Base):
    def test_ssh_uninstall_accepts_only_the_selected_identity_in_all_service_modes(self):
        identity = '00000000-0000-4000-8000-000000000001'
        for platform, arch in [('Linux', 'x86_64'), ('Darwin', 'arm64')]:
            for mode, uid in [('system', 0), ('system', 1000), ('user', 1000)]:
                with self.subTest(platform=platform, mode=mode, uid=uid):
                    env = self.new_env(platform, arch)
                    paths = env.preinstall(mode)
                    (paths['state'] / 'config.json').write_text(json.dumps({'agent_id': identity}, indent=2))
                    result = env.run('uninstall', uid=uid, mode=mode, token=None, server=None, HOPE_EXPECT_AGENT_ID=identity)
                    self.ok(result)
                    self.assertIn('uninstall', result.phases)
                    self.assertIn('complete', result.phases)
                    self.assertFalse(paths['binary'].exists())
                    self.assertTrue((paths['state'] / 'config.json').exists())

    def test_ssh_uninstall_rejects_other_missing_or_oversized_identity_before_service_changes(self):
        identity = '00000000-0000-4000-8000-000000000001'
        for config in [json.dumps({'agent_id': '00000000-0000-4000-8000-000000000002'}), '{}', 'broken', 'x' * 131073]:
            with self.subTest(config=config[:80]):
                env = self.new_env()
                paths = env.preinstall()
                (paths['state'] / 'config.json').write_text(config)
                result = env.run('uninstall', token=None, server=None, HOPE_EXPECT_AGENT_ID=identity)
                self.failed(result, 'EXISTING_IDENTITY_MISMATCH')
                self.assertNotIn('systemctl[system] stop', env.log())
                self.assertTrue(paths['binary'].exists())
                self.assertTrue(paths['unit'].exists())


class Manifest(Base):
    """The signed manifest binds version, size and checksum to each artifact."""

    def refused(self, code):
        result = self.env.run()
        self.failed(result, code)
        self.assertNotIn('agent[', self.env.log())  # nothing downloaded was executed
        self.untouched(self.env)
        return result

    def test_manifest_is_fetched_and_verified_before_the_artifact(self):
        self.ok(self.env.run())
        curls = [line for line in self.env.log().splitlines() if line.startswith('curl ') and '-o ' in line and '--head' not in line]
        self.assertIn('manifest.json', curls[0])
        self.assertTrue(any('manifest.json.sig' in line for line in curls))

    def test_manifest_signed_by_another_key_is_rejected(self):
        self.env.publish('v1', manifest_key='other')
        self.refused('MANIFEST_INVALID')

    def test_tampered_manifest_is_rejected(self):
        manifest = self.env.rel / 'manifest.json'
        manifest.write_text(manifest.read_text().replace('"size":', '"size": ', 1))
        self.refused('MANIFEST_INVALID')

    def test_missing_manifest_is_unavailable(self):
        (self.env.rel / 'manifest.json').unlink()
        self.failed(self.env.run(), 'RELEASE_UNAVAILABLE')
        self.untouched(self.env)

    def test_checksum_mismatch(self):
        self.env.publish('v1', tamper=lambda entry: entry.update(sha256='0' * 64))
        self.refused('CHECKSUM_MISMATCH')

    def test_size_mismatch(self):
        self.env.publish('v1', tamper=lambda entry: entry.update(size=entry['size'] + 1))
        self.refused('CHECKSUM_MISMATCH')

    def test_artifact_replaced_after_manifest_is_a_checksum_mismatch(self):
        self.env.replace_artifact('v2')  # validly signed, but not the artifact the manifest describes
        self.refused('CHECKSUM_MISMATCH')

    def test_manifest_without_this_platform(self):
        paths = self.env.preinstall()
        self.env.publish('v1', platform='macos', arch='aarch64')
        self.env.entries.pop(('linux', 'x86_64'))
        self.env.write_manifest('v1')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'RELEASE_UNAVAILABLE')
        self.assertIn('linux/x86_64', result.err)
        self.assertTrue(paths['binary'].exists())
        self.assertNotIn('systemctl[system] stop', self.env.log())

    def test_artifact_version_must_match_manifest_version(self):
        self.env.publish('v1', manifest_version='v2')  # v1 binary, correct checksum, manifest says v2
        self.refused_after_run('MANIFEST_INVALID')

    def refused_after_run(self, code):
        # the binary is run once (--version) from the temporary directory, but nothing is installed
        result = self.env.run()
        self.failed(result, code)
        log = self.env.log()
        self.assertEqual(['agent[v1] --version'], [line for line in log.splitlines() if line.startswith('agent[')])
        self.untouched(self.env)

    def test_malformed_entry_is_rejected(self):
        self.env.publish('v1', tamper=lambda entry: entry.update(file='hope-agent-linux-aarch64'))
        self.refused('MANIFEST_INVALID')

    def test_duplicate_entry_is_rejected(self):
        manifest = self.env.rel / 'manifest.json'
        lines = manifest.read_text().splitlines()
        row = next(line for line in lines if '"platform":"linux"' in line)
        lines.insert(lines.index(row) + 1, row)
        manifest.write_text('\n'.join(lines) + '\n')
        import subprocess
        subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(harness.keys()['good'][0]), '-out', str(manifest) + '.sig', str(manifest)], check=True, capture_output=True)
        self.refused('MANIFEST_INVALID')

    def test_downgrade_is_refused_on_upgrade(self):
        paths = self.env.preinstall(version='v2')
        self.env.publish('v1')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'DOWNGRADE_REFUSED')
        self.assertEqual('v2', self.version(paths['binary']))
        self.assertNotIn('systemctl[system] stop', self.env.log())
        self.assertNotIn('agent[v1]', self.env.log())

    def test_downgrade_is_allowed_explicitly(self):
        paths = self.env.preinstall(version='v2')
        self.env.publish('v1')
        result = self.env.run('upgrade', token=None, HOPE_ALLOW_DOWNGRADE=1)
        self.ok(result)
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertIn('Downgrading', result.err)

    def test_same_and_newer_versions_install(self):
        paths = self.env.preinstall(version='v1')
        self.ok(self.env.run('upgrade', token=None))
        self.env.publish('v10')
        self.ok(self.env.run('upgrade', token=None))
        self.assertEqual('v10', self.version(paths['binary']))

    def test_version_comparison_is_numeric_not_lexical(self):
        paths = self.env.preinstall(version='0.9.0')
        self.env.publish('0.10.0')
        self.ok(self.env.run('upgrade', token=None))
        self.assertEqual('0.10.0', self.version(paths['binary']))
        self.env.publish('0.9.5')
        self.failed(self.env.run('upgrade', token=None), 'DOWNGRADE_REFUSED')


@unittest.skip('H.O.P.E currently ships agents for Linux x86_64 only.')
class MacOS(Base):
    def test_uninstall_does_not_remove_a_service_that_failed_to_stop(self):
        self.ok(self.env.run())
        self.env.fault('stop-fail')
        self.failed(self.env.run('uninstall', token=None, server=None), 'UNINSTALL_STOP_FAILED')
        self.assertTrue(self.env.paths('system')['binary'].exists())
        self.assertTrue(self.env.paths('system')['unit'].exists())

    def setUp(self):
        self.env = self.new_env('Darwin', 'arm64')

    def test_root_install_uses_dedicated_user_and_group(self):
        result = self.env.run()
        self.ok(result)
        paths = self.env.paths('system')
        plist = paths['unit'].read_text()
        self.assertIn('<key>UserName</key><string>_hopeagent</string>', plist)
        self.assertIn('<key>GroupName</key><string>_hopeagent</string>', plist)
        self.assertRegex(plist, r'HOPE_ACTIVATION_ID</key><string>[0-9a-f-]{36}')
        accounts = json.loads((self.env.base / 'fake/accounts.json').read_text())
        user, group = accounts['users']['_hopeagent'], accounts['groups']['_hopeagent']
        self.assertTrue(400 <= int(user['UniqueID']) <= 499)
        self.assertEqual(user['PrimaryGroupID'], group['PrimaryGroupID'])
        self.assertNotEqual('20', user['PrimaryGroupID'])
        self.assertEqual('/usr/bin/false', user['UserShell'])
        self.assertIn('launchctl bootstrap system', self.env.log())
        self.assertIn('chown -R _hopeagent:_hopeagent', self.env.log())

    def test_user_install_uses_gui_domain(self):
        result = self.env.run(uid=501, mode='user')
        self.ok(result)
        self.assertTrue(self.env.paths('user')['unit'].exists())
        self.assertIn('launchctl bootstrap gui/501', self.env.log())
        self.assertIn('GUI', result.err + result.out)
        self.assertEqual({}, self.env.snapshot(self.env.root))

    def test_user_install_without_gui_session(self):
        self.env.fault('no-gui')
        self.failed(self.env.run(uid=501, mode='user'), 'NO_SERVICE_MANAGER')
        self.assertEqual({}, self.env.snapshot(self.env.home))

    def test_preexisting_account_in_staff_group_is_rejected(self):
        state = str(self.env.paths('system')['state'])
        (self.env.base / 'fake/accounts.json').write_text(json.dumps({
            'users': {'_hopeagent': {'UniqueID': '401', 'PrimaryGroupID': '20', 'UserShell': '/usr/bin/false', 'NFSHomeDirectory': state}},
            'groups': {'_hopeagent': {'PrimaryGroupID': '20'}}}))
        result = self.env.run()
        self.failed(result, 'SERVICE_ACCOUNT_INVALID')
        self.assertFalse(self.env.paths('system')['binary'].exists())
        self.assertNotIn('launchctl bootstrap', self.env.log())

    def test_preexisting_account_with_wrong_shell_is_rejected(self):
        state = str(self.env.paths('system')['state'])
        (self.env.base / 'fake/accounts.json').write_text(json.dumps({
            'users': {'_hopeagent': {'UniqueID': '401', 'PrimaryGroupID': '401', 'UserShell': '/bin/zsh', 'NFSHomeDirectory': state}},
            'groups': {'_hopeagent': {'PrimaryGroupID': '401'}}}))
        self.failed(self.env.run(), 'SERVICE_ACCOUNT_INVALID')

    def test_valid_preexisting_account_is_reused(self):
        state = str(self.env.paths('system')['state'])
        (self.env.base / 'fake/accounts.json').write_text(json.dumps({
            'users': {'_hopeagent': {'UniqueID': '401', 'PrimaryGroupID': '401', 'UserShell': '/usr/bin/false', 'NFSHomeDirectory': state}},
            'groups': {'_hopeagent': {'PrimaryGroupID': '401'}}}))
        self.ok(self.env.run())
        self.assertNotIn('dscl . -create', self.env.log())

    def test_loaded_but_not_running_service_is_not_restarted_on_rollback(self):
        self.env.preinstall()
        self.env.fault('launchd-no-pid')
        self.env.publish('v2')
        self.env.fault('enable-fail')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'SERVICE_START_FAILED')
        self.assertEqual(1, self.env.log().count('launchctl bootstrap'))

    def test_account_with_uid_collision_or_extra_members_is_rejected(self):
        state = str(self.env.paths('system')['state'])
        base = {'users': {'_hopeagent': {'UniqueID': '401', 'PrimaryGroupID': '401', 'UserShell': '/usr/bin/false', 'NFSHomeDirectory': state}},
                'groups': {'_hopeagent': {'PrimaryGroupID': '401', 'GroupMembership': ''}}}
        collide = json.loads(json.dumps(base))
        collide['users']['other'] = {'UniqueID': '401', 'PrimaryGroupID': '501', 'UserShell': '/bin/zsh', 'NFSHomeDirectory': '/Users/o'}
        members = json.loads(json.dumps(base))
        members['groups']['_hopeagent']['GroupMembership'] = '_hopeagent martin'
        for name, data in (('collision', collide), ('members', members)):
            with self.subTest(name):
                env = self.new_env('Darwin', 'arm64')
                (env.base / 'fake/accounts.json').write_text(json.dumps(data))
                self.failed(env.run(), 'SERVICE_ACCOUNT_INVALID')
                self.assertFalse(env.paths('system')['binary'].exists())

    def test_macos_rollback_restores_previous_daemon(self):
        paths = self.env.preinstall()
        self.env.publish('v2')
        self.env.fault('enable-fail')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'SERVICE_START_FAILED')
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertIn('OLD-UNIT', paths['unit'].read_text())
        self.assertTrue((self.env.base / 'service/launchd.active').exists())


class ImmutableRelease(Base):
    def test_install_stays_pinned_when_publication_changes_between_downloads(self):
        artifact = 'hope-agent-linux-x86_64'
        files = ['manifest.json', 'manifest.json.sig', artifact, artifact + '.sig']
        for phase in ['before-downloads'] + ['head-' + name for name in files] + files:
            with self.subTest(phase=phase):
                env = self.new_env()
                selected = env.capture_release()
                env.installer.write_text(env.installer.read_text().replace('@@HOPE_RELEASE_ID@@', selected))
                # The already fetched installer must still install v1 after a key rotation.
                env.publish('v2', key='other', manifest_key='other')
                replacement = env.capture_release()
                if phase != 'before-downloads':
                    env.set('release/current-id', selected)
                    env.set('fault/publish-after-' + phase, replacement)
                self.ok(env.run())
                self.assertEqual('v1', self.version(env.paths('system')['binary']))
                requests = [line for line in env.log().splitlines() if line.startswith('curl ')]
                self.assertEqual(8, len(requests))
                self.assertTrue(all('/by-id/' + selected + '/' in line for line in requests))
                if phase != 'before-downloads':
                    self.assertFalse((env.base / 'fault' / ('publish-after-' + phase)).exists())

    def test_saved_updater_selects_once_then_ignores_later_publication(self):
        self.ok(self.env.run())
        self.env.publish('v3')
        later = self.env.capture_release()
        self.env.publish('v2')
        self.env.set('fault/publish-after-current-id', later)
        before = len(self.env.log())
        self.ok(self.env.updater())
        self.assertEqual('v2', self.version(self.env.paths('system')['binary']))
        requests = [line for line in self.env.log()[before:].splitlines() if line.startswith('curl ')]
        self.assertIn('/current-id', requests[0])
        selected = requests[1].split('/by-id/')[1].split('/')[0]
        self.assertNotEqual(later, selected)
        self.assertTrue(all('/by-id/' + selected + '/' in line for line in requests[1:]))

    def test_malformed_release_selector_never_touches_the_running_agent(self):
        self.ok(self.env.run())
        self.env.publish('v2')
        paths = self.env.paths('system')
        valid = '1.0.0-0123456789ab'
        for selector in ['../' + valid, valid + '/private', valid + '\nextra', valid + ' ', valid + ';id', 'current']:
            with self.subTest(selector=selector):
                self.env.set('fault/selector-body', selector)
                before = len(self.env.log())
                self.failed(self.env.updater(), 'RELEASE_UNAVAILABLE')
                self.assertEqual('v1', self.version(paths['binary']))
                attempt = self.env.log()[before:]
                self.assertNotIn('systemctl[system] stop', attempt)
                self.assertNotIn('/by-id/', attempt)


class Updater(Base):
    """The root-owned updater that lets the console update an installed agent without a new token."""

    def installed(self, env=None):
        env = env or self.env
        self.ok(env.run())
        return env.paths('system')

    def request(self, env=None):
        """What the agent does when the server asks for an update: leave a request file."""
        env = env or self.env
        queue = env.paths('system')['state'] / 'update-queue'
        (queue / 'request').write_text('v2')
        return queue / 'request'

    def result_of(self, env=None):
        env = env or self.env
        text = (env.paths('system')['state'] / 'update-result').read_text()
        return dict(line.split('=', 1) for line in text.splitlines())

    def test_a_system_install_keeps_a_pinned_installer_and_an_updater(self):
        paths = self.installed()
        root = self.env.root
        wrapper = root / 'usr/local/sbin/hope-agent-update'
        self.assertTrue(wrapper.stat().st_mode & 0o111)
        directory = root / 'var/lib/hope-agent-update'
        pinned = (directory / 'install-agent.sh').read_text()
        self.assertIn(harness.keys()['good'][1], pinned)
        self.assertNotIn('@@HOPE_PUBLIC_KEY@@', pinned)
        self.assertEqual(harness.SERVER, (directory / 'server').read_text().strip())
        self.assertTrue((paths['state'] / 'update-queue').is_dir())
        path_unit = (root / 'etc/systemd/system/hope-agent-update.path').read_text()
        self.assertIn(f"PathExists={paths['state']}/update-queue/request", path_unit)
        service_unit = (root / 'etc/systemd/system/hope-agent-update.service').read_text()
        self.assertIn('Type=oneshot', service_unit)
        self.assertIn(f'ExecStart="{wrapper}"', service_unit)
        self.assertIn('Restart=on-failure', service_unit)
        self.assertIn('RestartSec=5min', service_unit)
        self.assertIn('RestartPreventExitStatus=1', service_unit)
        self.assertIn('exit_rc=75', wrapper.read_text())
        self.assertIn('systemctl[system] enable --now hope-agent-update.path', self.env.log())

    def test_the_updater_holds_no_secret(self):
        self.installed()
        for path in (self.env.root / 'var/lib/hope-agent-update').rglob('*'):
            if path.is_file():
                self.assertNotIn(harness.TOKEN, path.read_text())
                self.assertNotIn(harness.CREDENTIAL, path.read_text())
        self.assertNotIn(harness.TOKEN, (self.env.root / 'usr/local/sbin/hope-agent-update').read_text())

    def test_a_user_service_gets_no_updater(self):
        self.ok(self.env.run(uid=1000, mode='user'))
        self.assertFalse((self.env.root / 'usr/local/sbin/hope-agent-update').exists())
        self.assertNotIn('hope-agent-update', self.env.log())
        self.assertFalse((self.env.paths('user')['state'] / 'update-queue').exists())

    def test_a_request_updates_the_agent_without_a_token_and_reports_the_outcome(self):
        paths = self.installed()
        self.env.publish('v2')
        request = self.request()
        result = self.env.updater()
        self.ok(result)
        self.assertEqual('v2', self.version(paths['binary']))
        self.assertFalse(request.exists())
        self.assertEqual({'status': 'updated', 'version': 'v2'}, {k: v for k, v in self.result_of().items() if k != 'at'})
        config = (paths['state'] / 'config.json').read_text()
        self.assertIn('credential=' + harness.CREDENTIAL, config)
        self.assertIn('hope-install: result=updated v2', result.err)
        self.assertFalse(self.env.txn().exists())
        self.assertEqual('644', oct((paths['state'] / 'update-result').stat().st_mode)[-3:])

    def test_nothing_changes_when_the_agent_is_already_current(self):
        paths = self.installed()
        before = self.env.log()
        self.ok(self.env.updater())
        self.assertEqual('current', self.result_of()['status'])
        self.assertEqual('v1', self.result_of()['version'])
        added = self.env.log()[len(before):]
        self.assertNotIn('systemctl[system] stop', added)
        self.assertEqual('v1', self.version(paths['binary']))

    def test_an_older_offer_is_ignored_rather_than_refused(self):
        paths = self.installed()
        self.env.publish('v0')
        self.ok(self.env.updater())
        self.assertEqual('current', self.result_of()['status'])
        self.assertEqual('v1', self.version(paths['binary']))

    def test_a_server_that_changes_its_signing_key_cannot_push_an_update(self):
        paths = self.installed()
        self.env.publish('v2', key='other', manifest_key='other')
        before = len(self.env.log())
        result = self.env.updater()
        self.assertNotEqual(0, result.code)
        self.assertEqual({'status': 'failed', 'code': 'MANIFEST_INVALID'}, {k: v for k, v in self.result_of().items() if k != 'at'})
        self.assertEqual('v1', self.version(paths['binary']))
        attempt = self.env.log()[before:]
        self.assertNotIn('systemctl[system] stop', attempt)
        self.assertNotIn('hope-agent.sig', attempt)

    def test_a_tampered_binary_is_never_installed(self):
        paths = self.installed()
        self.env.publish('v2')
        self.env.replace_artifact('v3')
        result = self.env.updater()
        self.assertNotEqual(0, result.code)
        self.assertEqual('failed', self.result_of()['status'])
        self.assertIn(self.result_of()['code'], ('CHECKSUM_MISMATCH', 'BAD_SIGNATURE'))
        self.assertEqual('v1', self.version(paths['binary']))

    def test_a_failed_update_rolls_back_and_says_why(self):
        paths = self.installed()
        self.env.publish('v2')
        self.env.fault('no-report')
        result = self.env.updater(HOPE_FIRST_REPORT_TIMEOUT=2)
        self.assertNotEqual(0, result.code)
        self.assertEqual({'status': 'failed', 'code': 'FIRST_REPORT_TIMEOUT'}, {k: v for k, v in self.result_of().items() if k != 'at'})
        self.assertEqual('v1', self.version(paths['binary']))
        self.assertTrue((self.env.base / 'service/system.active').exists())

    def test_the_request_file_is_removed_before_the_update_so_it_cannot_loop(self):
        self.installed()
        self.env.publish('v2')
        self.env.fault('no-report')
        request = self.request()
        self.env.updater(HOPE_FIRST_REPORT_TIMEOUT=2)
        self.assertFalse(request.exists())

    def test_a_link_in_the_state_directory_stops_the_update_before_anything_runs_as_root(self):
        paths = self.installed()
        self.env.publish('v2')
        victim = self.env.base / 'victim'
        victim.mkdir()
        (victim / 'request').write_text('keep me')
        before = len(self.env.log())
        queue = paths['state'] / 'update-queue'
        queue.rmdir()
        queue.symlink_to(victim)
        result = self.env.updater()
        self.assertNotEqual(0, result.code)
        self.assertIn('symbolic link', result.err)
        self.assertEqual('keep me', (victim / 'request').read_text())
        self.assertEqual('v1', self.version(paths['binary']))
        attempt = self.env.log()[before:]
        self.assertNotIn('curl', attempt)
        self.assertNotIn('agent[', attempt)
        self.assertEqual({'status': 'failed', 'code': 'STATE_UNSAFE'}, {k: v for k, v in self.result_of().items() if k != 'at'})

    def test_a_link_left_at_the_result_path_is_refused_and_replaced_not_followed(self):
        paths = self.installed()
        victim = self.env.base / 'victim.txt'
        victim.write_text('keep me')
        (paths['state'] / 'update-result').symlink_to(victim)
        self.assertNotEqual(0, self.env.updater().code)
        self.assertEqual('keep me', victim.read_text())
        self.assertFalse((paths['state'] / 'update-result').is_symlink())
        self.assertEqual('STATE_UNSAFE', self.result_of()['code'])

    def test_a_link_at_the_request_path_is_only_deleted_never_followed(self):
        paths = self.installed()
        victim = self.env.base / 'victim.txt'
        victim.write_text('keep me')
        (paths['state'] / 'update-queue' / 'request').symlink_to(victim)
        self.ok(self.env.updater())
        self.assertEqual('keep me', victim.read_text())
        self.assertFalse((paths['state'] / 'update-queue' / 'request').exists())

    def test_a_deeper_link_is_found_too(self):
        paths = self.installed()
        (paths['state'] / 'update-queue' / 'other').symlink_to(self.env.base / 'elsewhere')
        result = self.env.updater()
        self.assertNotEqual(0, result.code)
        self.assertEqual('STATE_UNSAFE', self.result_of()['code'])

    def test_a_linked_state_directory_gets_no_result(self):
        paths = self.installed()
        real = self.env.base / 'elsewhere'
        paths['state'].rename(real)
        paths['state'].symlink_to(real)
        self.assertNotEqual(0, self.env.updater().code)
        self.assertEqual([], [p.name for p in real.iterdir() if p.name == 'update-result'])

    def test_two_updates_do_not_run_at_once(self):
        self.installed()
        self.env.publish('v2')
        import fcntl
        lock = open(self.env.root / 'var/lib/hope-agent-update/update.lock', 'a')
        self.addCleanup(lock.close)
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        request = self.request()
        result = self.env.updater()
        self.assertEqual(75, result.code)
        self.assertTrue(request.exists(), 'a busy updater must retain the pending request')
        self.assertIn('already running', result.err)
        self.assertEqual('v1', self.version(self.env.paths('system')['binary']))

    def test_the_updater_uses_the_pinned_server_and_ca(self):
        ca = self.env.base / 'ca.pem'
        ca.write_text('CA')
        self.ok(self.env.run(HOPE_CA_CERT=ca))
        self.assertEqual(str(ca), (self.env.root / 'var/lib/hope-agent-update/ca').read_text().strip())
        self.env.publish('v2')
        before = len(self.env.log())
        self.ok(self.env.updater())
        curls = [line for line in self.env.log()[before:].splitlines() if line.startswith('curl ')]
        self.assertTrue(curls)
        for line in curls:
            self.assertIn('--cacert ' + str(ca), line)
            self.assertIn(harness.SERVER, line)

    def test_an_update_does_not_replace_the_pinned_installer(self):
        self.installed()
        pinned = self.env.root / 'var/lib/hope-agent-update/install-agent.sh'
        before = pinned.read_text()
        self.env.publish('v2')
        self.ok(self.env.updater())
        self.assertEqual(before, pinned.read_text())

    def test_upgrade_command_repairs_updater_when_binary_is_already_current(self):
        self.installed()
        wrapper = self.env.root / 'usr/local/sbin/hope-agent-update'
        wrapper.write_text('old updater')
        result = self.env.run('upgrade', token=None, HOPE_ONLY_NEWER=1)
        self.ok(result)
        self.assertIn('result=current v1', result.err)
        self.assertIn('flock -n 9', wrapper.read_text())
        self.assertNotIn('phase=stop', result.err)
        unit = (self.env.root / 'etc/systemd/system/hope-agent-update.path').read_text()
        self.assertIn(f'PathExists={self.env.txn()}', unit)

    def test_orphaned_kernel_lock_file_does_not_prevent_recovery(self):
        self.installed()
        (self.env.root / 'var/lib/hope-agent-update/update.lock').write_text('')
        self.env.publish('v2')
        result = self.env.updater()
        self.ok(result)
        self.assertEqual('updated', self.result_of()['status'])

    def test_manual_install_does_not_recover_an_active_update_transaction(self):
        import fcntl
        self.installed()
        lock = open(self.env.root / 'var/lib/hope-agent-update/update.lock', 'a')
        self.addCleanup(lock.close)
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.env.publish('v2')
        result = self.env.run('upgrade', token=None)
        self.failed(result, 'INSTALL_BUSY')
        self.assertNotIn('phase=stop', result.err)
        self.assertTrue((self.env.base / 'service/system.active').exists())

    def test_an_interrupted_update_can_recover_without_an_agent_request(self):
        self.installed()
        self.env.publish('v2')
        self.env.fault('sigkill-at-place-binary')
        interrupted = self.env.updater()
        self.assertNotEqual(0, interrupted.code)
        self.assertTrue(self.env.txn().exists())
        self.assertFalse((self.env.base / 'service/system.active').exists())
        # The transaction watcher invokes the updater; the agent is stopped and cannot queue work.
        recovered = self.env.updater()
        self.ok(recovered)
        self.assertFalse(self.env.txn().exists())
        self.assertTrue((self.env.base / 'service/system.active').exists())
        self.assertEqual('updated', self.result_of()['status'])

    def test_a_full_installation_again_refreshes_the_pinned_key(self):
        self.installed()
        pinned = self.env.root / 'var/lib/hope-agent-update/install-agent.sh'
        newer = harness.keys()['other'][1]
        self.env.installer.write_text(harness.INSTALLER.read_text().replace('@@HOPE_PUBLIC_KEY@@', newer))
        self.env.publish('v2', key='other', manifest_key='other')
        self.ok(self.env.run())
        self.assertIn(newer, pinned.read_text())

    def test_only_newer_needs_an_installed_agent(self):
        result = self.env.run('upgrade', token=None, HOPE_ONLY_NEWER=1)
        self.failed(result, 'NOT_INSTALLED')

    def test_updater_setup_failure_is_a_warning_not_a_failed_install(self):
        self.env.fault('updater-units-fail')
        result = self.env.run()
        self.ok(result)
        self.assertIn('warning=UPDATER_NOT_INSTALLED', result.err)
        self.assertTrue(self.env.paths('system')['binary'].exists())
        self.assertIn('accepted report', result.out)

    def test_an_update_by_the_updater_leaves_the_updater_alone(self):
        self.installed()
        self.env.publish('v2')
        before = self.env.log()
        self.ok(self.env.updater())
        self.assertNotIn('enable --now hope-agent-update', self.env.log()[len(before):])

    def test_uninstall_removes_the_updater(self):
        paths = self.installed()
        result = self.env.run('uninstall', token=None, server=None)
        self.ok(result)
        root = self.env.root
        for gone in ('usr/local/sbin/hope-agent-update', 'var/lib/hope-agent-update',
                     'etc/systemd/system/hope-agent-update.path', 'etc/systemd/system/hope-agent-update.service'):
            self.assertFalse((root / gone).exists(), gone)
        self.assertIn('systemctl[system] disable --now hope-agent-update.path', self.env.log())
        self.assertTrue((paths['state'] / 'config.json').exists())

    def test_purge_removes_the_queue_and_the_result(self):
        paths = self.installed()
        self.env.publish('v2')
        self.ok(self.env.updater())
        self.ok(self.env.run('uninstall', token=None, server=None, HOPE_PURGE_IDENTITY=1))
        self.assertFalse(paths['state'].exists())


@unittest.skip('H.O.P.E currently ships agents for Linux x86_64 only.')
class UpdaterMacOS(Base):
    def setUp(self):
        self.env = self.new_env('Darwin', 'arm64')

    def test_a_system_daemon_watches_a_queue_directory(self):
        self.ok(self.env.run())
        root = self.env.root
        plist = (root / 'Library/LaunchDaemons/io.hope.agent.update.plist').read_text()
        state = self.env.paths('system')['state']
        self.assertIn('<key>Label</key><string>io.hope.agent.update</string>', plist)
        self.assertIn(f'<key>QueueDirectories</key><array><string>{state}/update-queue</string>', plist)
        self.assertTrue((root / 'usr/local/sbin/hope-agent-update').exists())
        self.assertTrue((root / 'Library/Application Support/hope-agent-update/install-agent.sh').exists())
        self.assertIn('launchctl bootstrap system', self.env.log())

    def test_a_request_updates_the_agent(self):
        self.ok(self.env.run())
        self.env.publish('v2')
        self.ok(self.env.updater())
        self.assertEqual('v2', self.version(self.env.paths('system')['binary']))

    def test_uninstall_removes_the_updater(self):
        self.ok(self.env.run())
        self.ok(self.env.run('uninstall', token=None, server=None))
        self.assertFalse((self.env.root / 'Library/LaunchDaemons/io.hope.agent.update.plist').exists())
        self.assertFalse((self.env.root / 'usr/local/sbin/hope-agent-update').exists())
        self.assertIn('launchctl bootout system/io.hope.agent.update', self.env.log())


if __name__ == '__main__':
    unittest.main()
