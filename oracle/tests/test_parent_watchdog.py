"""Parent-death watchdog: the resident server must self-exit when the supervising
app dies WITHOUT a graceful shutdown (SIGKILL, crash, `tauri dev` rebuild). The
Rust supervisor's `on_app_exit` kill only runs on clean exits; this watchdog is
the net for everything else."""

import os
import threading
import time

import pytest

import oracle.server.main as main


class TestParseParentPid:
    def test_valid_pid(self):
        assert main._parse_parent_pid("123") == 123

    def test_strips_whitespace(self):
        assert main._parse_parent_pid(" 42 \n") == 42

    def test_empty_and_none(self):
        assert main._parse_parent_pid("") is None
        assert main._parse_parent_pid(None) is None

    def test_non_numeric(self):
        assert main._parse_parent_pid("abc") is None

    def test_non_positive(self):
        assert main._parse_parent_pid("0") is None
        assert main._parse_parent_pid("-3") is None


class TestParentAlive:
    def test_own_parent_is_alive(self):
        # Our real parent (pytest's parent) is alive by definition while we run.
        assert main._parent_alive(os.getppid()) is True

    def test_mismatched_parent_reads_as_dead_on_posix(self):
        if os.name == "nt":
            return
        # POSIX check is a strict ppid comparison: any pid that is not our
        # current ppid means the original parent is gone (we were re-parented).
        assert main._parent_alive(999999999) is False


class TestStartParentWatchdog:
    def test_no_env_no_thread(self, monkeypatch):
        monkeypatch.delenv("ORACLE_PARENT_PID", raising=False)
        main._start_parent_watchdog()
        names = [t.name for t in threading.enumerate()]
        assert "oracle-parent-watchdog" not in names

    def test_garbage_env_no_thread(self, monkeypatch):
        monkeypatch.setenv("ORACLE_PARENT_PID", "not-a-pid")
        main._start_parent_watchdog()
        names = [t.name for t in threading.enumerate()]
        assert "oracle-parent-watchdog" not in names

    @pytest.mark.filterwarnings("ignore::pytest.PytestUnhandledThreadExceptionWarning")
    def test_exits_when_parent_dead(self, monkeypatch):
        exited = threading.Event()

        def fake_exit(code):
            assert code == 0
            exited.set()
            raise SystemExit  # stop the watchdog loop inside its thread

        monkeypatch.setenv("ORACLE_PARENT_PID", str(os.getppid()))
        monkeypatch.setattr(main, "_PARENT_POLL_SECONDS", 0.01)
        monkeypatch.setattr(main, "_parent_alive", lambda pid: False)
        monkeypatch.setattr(main.os, "_exit", fake_exit)

        main._start_parent_watchdog()
        assert exited.wait(timeout=5.0), "watchdog never called os._exit"

    def test_stays_alive_while_parent_alive(self, monkeypatch):
        exited = threading.Event()

        monkeypatch.setenv("ORACLE_PARENT_PID", str(os.getppid()))
        monkeypatch.setattr(main, "_PARENT_POLL_SECONDS", 0.01)
        monkeypatch.setattr(main, "_parent_alive", lambda pid: True)
        monkeypatch.setattr(main.os, "_exit", lambda code: exited.set())

        main._start_parent_watchdog()
        time.sleep(0.2)
        assert not exited.is_set(), "watchdog exited despite live parent"
