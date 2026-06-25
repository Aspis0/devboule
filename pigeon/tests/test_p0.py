"""P0 — Scaffolding contract: config (env-driven), SQLite schema + WAL, Pydantic models.

These tests define the API surface the implementation must satisfy. They are written
FIRST (TDD-strict); the local oMLX model implements pigeon/{config,db,models}.py to make
them green. Run from repo root:

    oracle-data/venv/bin/python -m pytest pigeon -c pigeon/pytest.ini
"""
from pathlib import Path

import pytest
from pydantic import ValidationError

from pigeon import config, db, models


# --------------------------------------------------------------------------- config

def test_config_defaults_when_env_empty():
    s = config.load_settings({})
    assert s.port == 8769
    assert Path(s.pigeon_dir).name == "pigeon-data"
    # sqlite lives under pigeon_dir by default, named mailbox.sqlite
    assert Path(s.sqlite_path).name == "mailbox.sqlite"
    assert Path(s.sqlite_path).parent == Path(s.pigeon_dir)
    assert s.auth_token is None


def test_config_env_overrides():
    env = {
        "PIGEON_PORT": "23456",
        "PIGEON_DIR": "/tmp/pdir",
        "PIGEON_SQLITE_PATH": "/tmp/custom/mb.sqlite",
        "PIGEON_AUTH_TOKEN": "secret-tok",
    }
    s = config.load_settings(env)
    assert s.port == 23456
    assert str(s.pigeon_dir) == "/tmp/pdir"
    assert str(s.sqlite_path) == "/tmp/custom/mb.sqlite"
    assert s.auth_token == "secret-tok"


def test_config_sqlite_defaults_under_custom_dir():
    s = config.load_settings({"PIGEON_DIR": "/tmp/pdir"})
    assert str(s.sqlite_path) == "/tmp/pdir/mailbox.sqlite"


# ------------------------------------------------------------------------------- db

async def _open(tmp_path):
    conn = await db.connect(str(tmp_path / "mb.sqlite"))
    await db.init_db(conn)
    return conn


async def test_db_init_creates_tables(tmp_path):
    conn = await _open(tmp_path)
    try:
        cur = await conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        )
        names = {row[0] for row in await cur.fetchall()}
        assert {"tasks", "agents"}.issubset(names)
    finally:
        await conn.close()


async def test_db_wal_enabled(tmp_path):
    conn = await _open(tmp_path)
    try:
        cur = await conn.execute("PRAGMA journal_mode")
        mode = (await cur.fetchone())[0]
        assert str(mode).lower() == "wal"
    finally:
        await conn.close()


async def test_db_tasks_columns(tmp_path):
    conn = await _open(tmp_path)
    try:
        cur = await conn.execute("PRAGMA table_info(tasks)")
        cols = {row[1] for row in await cur.fetchall()}
        expected = {
            "ticket_no", "sender_id", "receiver_id", "project_id", "priority",
            "status", "delivery_mode", "payload", "result", "error_msg",
            "reply_to_ticket", "created_at", "claimed_at", "done_at",
        }
        assert expected.issubset(cols)
    finally:
        await conn.close()


async def test_db_agents_columns(tmp_path):
    conn = await _open(tmp_path)
    try:
        cur = await conn.execute("PRAGMA table_info(agents)")
        cols = {row[1] for row in await cur.fetchall()}
        assert {"agent_id", "agent_type", "status", "last_seen"}.issubset(cols)
    finally:
        await conn.close()


async def test_db_poll_index_exists(tmp_path):
    conn = await _open(tmp_path)
    try:
        cur = await conn.execute(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='tasks'"
        )
        idx = {row[0] for row in await cur.fetchall()}
        assert any("poll" in n.lower() for n in idx), idx
    finally:
        await conn.close()


# --------------------------------------------------------------------------- models

def test_model_send_request_defaults():
    r = models.SendRequest(
        sender_id="main-coder:qwen35-27b:projA",
        receiver_id="mini-coder:gemma4:projA",
        project_id="projA",
        payload={"type": "edit_file", "instruction": "x"},
    )
    assert r.priority == 50


def test_model_send_request_requires_fields():
    with pytest.raises(ValidationError):
        models.SendRequest(sender_id="a")  # missing receiver_id/project_id/payload


def test_model_agent_defaults():
    a = models.Agent(agent_id="mini-coder:gemma4:projA")
    assert a.agent_type == "local"
    assert a.status == "unloaded"


def test_model_done_request():
    d = models.DoneRequest(ticket_no=47, result={"output": "ok"})
    assert d.ticket_no == 47


def test_model_poll_response_empty():
    p = models.PollResponse(ticket_no=None)
    assert p.ticket_no is None
    assert p.payload is None


# ----------------------------------------------------- review fixes (P1-forward)

async def test_db_rows_accessible_by_name(tmp_path):
    # P1 /poll maps columns to a Task by name → connect() must set a name-keyed
    # row factory (aiosqlite.Row), else row["ticket_no"] crashes on a tuple.
    conn = await _open(tmp_path)
    try:
        await conn.execute(
            "INSERT INTO tasks (sender_id, receiver_id, project_id, payload, created_at) "
            "VALUES (?, ?, ?, ?, ?)",
            ("s", "r", "p", "{}", 1),
        )
        await conn.commit()
        cur = await conn.execute("SELECT ticket_no, sender_id FROM tasks")
        row = await cur.fetchone()
        assert row["sender_id"] == "s"
        assert row["ticket_no"] == 1
    finally:
        await conn.close()


def test_config_port_empty_falls_back_to_default():
    # PIGEON_PORT set to empty string (common "unset" pattern) must not crash.
    assert config.load_settings({"PIGEON_PORT": ""}).port == 8769
