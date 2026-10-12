"""Offline regressions for the operator guards; every command runs in a temporary project."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class OperatorGuards(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "scripts").mkdir()
        (self.root / "bin").mkdir()
        (self.root / "data/arbitrum").mkdir(parents=True)
        self.env = dict(os.environ, PATH=f"{self.root / 'bin'}:{os.environ['PATH']}")

    def stub(self, path, text):
        p = self.root / path
        p.write_text("#!/bin/bash\n" + text)
        p.chmod(0o700)

    def run_script(self, name, *args):
        shutil.copy(REPO / "scripts" / name, self.root / "scripts" / name)
        return subprocess.run(["bash", str(self.root / "scripts" / name), *args],
                              env=self.env, capture_output=True, text=True, timeout=10)

    def test_seqscope_wrapper_preserves_test_failure(self):
        self.stub("scripts/dev.sh", 'exec "$@"\n')
        self.stub("bin/cargo", '[[ $1 != test ]] || exit 53\nexit 0\n')
        result = self.run_script("test-seqscope.sh")
        self.assertEqual(result.returncode, 53, result.stdout + result.stderr)
        self.assertNotIn("OFFLINE SUITE: OK", result.stdout)

    def test_unhealthy_is_never_accepted_as_healthy(self):
        self.stub("bin/podman", '[[ $1 != inspect ]] || echo "running unhealthy"\n')
        self.stub("bin/df", 'printf "Use%%\\n1%%\\n"\n')
        (self.root / "data/arbitrum/heartbeat").touch()
        result = self.run_script("watchdog.sh")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("container not healthy: running unhealthy", result.stdout)

    def test_shadow_environment_omits_private_key_without_evaluating_values(self):
        self.stub("bin/podman-compose", 'printf "%s\\n" "$MEV_ENV_FILE" >> compose-env\n')
        self.env["NO_BUILD"] = "1"
        (self.root / ".env").write_text('ARBITRUM_HTTP_URL=https://example.test/rpc\n'
            'ARBITRUM_WS_URL=wss://example.test/ws\nPRIVATE_KEY=synthetic-key\n'
            'ARBITRUM_HTTP_URL_2=$(touch must-not-execute)\n')
        result = self.run_script("run.sh", "shadow")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        shadow = self.root / ".env.shadow"
        self.assertNotIn("PRIVATE_KEY", shadow.read_text())
        self.assertIn("$(touch must-not-execute)", shadow.read_text())
        self.assertEqual(shadow.stat().st_mode & 0o777, 0o600)
        self.assertFalse((self.root / "must-not-execute").exists())
        self.assertEqual((self.root / "compose-env").read_text().splitlines(), [".env.shadow"] * 2)


if __name__ == "__main__":
    unittest.main()
