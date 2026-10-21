"""Unit checks for the disposable server-install gate's systemd readiness probe."""
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import server_install  # noqa: E402


class Result:
    def __init__(self, stdout="", stderr="", returncode=0):
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode


class WaitSystemdTests(unittest.TestCase):
    def test_running_systemd_manager_is_ready(self):
        with patch.object(server_install, "docker_exec", side_effect=[
            Result("running\n"),
            Result("systemd\n"),
            Result(),
        ]), patch.object(server_install, "log"):
            server_install.wait_systemd("target")

    def test_degraded_systemd_with_unrelated_failed_services_is_ready(self):
        with patch.object(server_install, "docker_exec", side_effect=[
            Result("degraded\n", returncode=1),
            Result("systemd\n"),
            Result(),
            Result("systemd-modules-load.service loaded failed failed Load Kernel Modules\n"),
        ]) as docker_exec, patch.object(server_install, "log"):
            server_install.wait_systemd("target")
        self.assertEqual(docker_exec.call_args_list[-1].args[1:],
                         ("systemctl", "--failed", "--no-legend"))

    def test_degraded_state_without_working_systemd_manager_times_out(self):
        with patch.object(server_install, "docker_exec", side_effect=[
            Result("degraded\n", returncode=1),
            Result("bash\n"),
        ]), patch.object(server_install.time, "monotonic", side_effect=[0, 91]), \
                patch.object(server_install.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "systemd did not become available"):
                server_install.wait_systemd("target")


if __name__ == "__main__":
    unittest.main()
