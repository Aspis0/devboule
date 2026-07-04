"""Latency fixes for the agent-facing Oracle path (2026-07-02 profiling):
`oracle_ask` burned 26.4s = 20.1s HTTP timeout against a stale resident-server
target (discovery pid never liveness-checked) + 5.0s macOS Keychain shell-out
repeated on EVERY call (the stdio child has no keychain session, so `security`
hangs to its timeout). Fixes under test:

1. `resolve_oracle_http_target` skips the discovery target when its `pid` is
   recorded and dead (falls back to the in-process engine immediately).
2. `app_vault_account_secret` caches lookups (including misses) with a TTL so
   the hang is paid at most once per TTL window, not per call.
"""

import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from oracle.server import aspis_mcp
from oracle.server.aspis_mcp import (
    _pid_alive,
    _reset_app_vault_cache,
    _reset_oracle_target_cache,
    app_vault_account_secret,
    resolve_oracle_http_target,
)


DISCOVERY = {
    "baseUrl": "http://127.0.0.1:36100",
    "authToken": "tok",
    "pid": 12345,
}


class PidAliveTests(unittest.TestCase):
    def test_own_pid_is_alive(self):
        self.assertTrue(_pid_alive(os.getpid()))

    def test_exited_child_pid_is_dead(self):
        proc = subprocess.Popen([sys.executable, "-c", "pass"])
        proc.wait()
        self.assertFalse(_pid_alive(proc.pid))

    def test_garbage_pids_are_dead(self):
        self.assertFalse(_pid_alive(0))
        self.assertFalse(_pid_alive(-1))

    def test_oversized_pid_never_raises(self):
        # Max-recall (reproduced live pre-fix): os.kill raises OverflowError on
        # an int too large for a C long; the probe must degrade to "dead", not
        # crash every oracle_ask until the discovery file is repaired.
        self.assertFalse(_pid_alive(99999999999999999999999999999))


class DiscoveryPidGateTests(unittest.TestCase):
    def setUp(self):
        _reset_oracle_target_cache()
        self.addCleanup(_reset_oracle_target_cache)

    def _resolve(self, discovery):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(
                aspis_mcp, "_read_oracle_discovery_file", return_value=discovery
            ):
                return resolve_oracle_http_target(Path(tmp))

    def test_dead_pid_skips_the_http_target(self):
        with patch.object(aspis_mcp, "_pid_alive", return_value=False):
            self.assertIsNone(self._resolve(dict(DISCOVERY)))

    def test_live_pid_keeps_the_http_target(self):
        with patch.object(aspis_mcp, "_pid_alive", return_value=True):
            self.assertEqual(
                self._resolve(dict(DISCOVERY)), ("http://127.0.0.1:36100", "tok")
            )

    def test_missing_pid_keeps_the_pre_fix_behavior(self):
        discovery = {k: v for k, v in DISCOVERY.items() if k != "pid"}
        self.assertEqual(
            self._resolve(discovery), ("http://127.0.0.1:36100", "tok")
        )

    def test_bool_pid_is_never_probed(self):
        # bool is an int subclass: a corrupt `"pid": true` must not probe pid 1.
        def must_not_be_called(_pid):
            raise AssertionError("_pid_alive must not be called for a bool pid")

        discovery = dict(DISCOVERY)
        discovery["pid"] = True
        with patch.object(aspis_mcp, "_pid_alive", must_not_be_called):
            self.assertEqual(
                self._resolve(discovery), ("http://127.0.0.1:36100", "tok")
            )


class HttpTimeoutEnvTests(unittest.TestCase):
    def _timeout_for(self, env_value):
        with patch.dict(
            os.environ, {"ASPIS_ORACLE_HTTP_TIMEOUT_SECS": env_value}, clear=False
        ):
            return aspis_mcp.HttpOracleEngine("http://127.0.0.1:1", "tok")._timeout

    def test_valid_override_is_honored(self):
        self.assertEqual(self._timeout_for("30"), 30.0)

    def test_non_finite_and_non_positive_values_fall_back(self):
        # inf/nan/zero/negatives parse as floats but would defeat the cap the
        # timeout exists to enforce — all must clamp back to the 8s default.
        for garbage in ("inf", "Infinity", "nan", "0", "-5", "1e9"):
            self.assertEqual(self._timeout_for(garbage), 8.0, garbage)

    def test_non_numeric_value_falls_back(self):
        self.assertEqual(self._timeout_for("fast"), 8.0)

    def test_explicit_timeout_param_wins_over_env(self):
        with patch.dict(
            os.environ, {"ASPIS_ORACLE_HTTP_TIMEOUT_SECS": "30"}, clear=False
        ):
            engine = aspis_mcp.HttpOracleEngine("http://127.0.0.1:1", "tok", timeout=3.0)
        self.assertEqual(engine._timeout, 3.0)


class AppVaultCacheTests(unittest.TestCase):
    def setUp(self):
        _reset_app_vault_cache()
        self.addCleanup(_reset_app_vault_cache)

    def test_lookup_miss_is_cached_within_ttl(self):
        calls = {"n": 0}

        def counting_lookup(*_args, **_kwargs):
            calls["n"] += 1
            return None

        with patch.object(
            aspis_mcp, "read_macos_keychain_password", counting_lookup
        ), patch.object(
            aspis_mcp, "read_windows_credential_password", counting_lookup
        ):
            self.assertIsNone(app_vault_account_secret("acct"))
            self.assertIsNone(app_vault_account_secret("acct"))
        self.assertEqual(calls["n"], 1, "the miss must be cached, not re-probed")

    def test_lookup_hit_is_cached_and_expires(self):
        calls = {"n": 0}

        def counting_lookup(*_args, **_kwargs):
            calls["n"] += 1
            return "secret-value"

        with patch.object(
            aspis_mcp, "read_macos_keychain_password", counting_lookup
        ), patch.object(
            aspis_mcp, "read_windows_credential_password", counting_lookup
        ):
            self.assertEqual(app_vault_account_secret("acct"), "secret-value")
            self.assertEqual(app_vault_account_secret("acct"), "secret-value")
            self.assertEqual(calls["n"], 1)
            # Expire the entry: the next call re-probes.
            key = next(iter(aspis_mcp._APP_VAULT_CACHE))
            stamp, value = aspis_mcp._APP_VAULT_CACHE[key]
            aspis_mcp._APP_VAULT_CACHE[key] = (
                stamp - aspis_mcp._APP_VAULT_TTL_SECONDS - 1,
                value,
            )
            self.assertEqual(app_vault_account_secret("acct"), "secret-value")
            self.assertEqual(calls["n"], 2)

    def test_disable_env_bypasses_lookup_and_cache(self):
        calls = {"n": 0}

        def counting_lookup(*_args, **_kwargs):
            calls["n"] += 1
            return "secret-value"

        with patch.dict(os.environ, {"ASPIS_MCP_DISABLE_APP_VAULT": "1"}), patch.object(
            aspis_mcp, "read_macos_keychain_password", counting_lookup
        ), patch.object(
            aspis_mcp, "read_windows_credential_password", counting_lookup
        ):
            self.assertIsNone(app_vault_account_secret("acct"))
        self.assertEqual(calls["n"], 0)


if __name__ == "__main__":
    unittest.main()
