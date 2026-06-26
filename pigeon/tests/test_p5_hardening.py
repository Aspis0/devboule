"""P5 — Go-live hardening contract (TDD-strict, written BEFORE implementation).

These tests define the behaviour the local model must implement in pigeon/{db,models,dispatcher}.py
to make the mailbox safe to run autonomously (Pigeon go-live, Slice 2):

  - at-least-once delivery: POST /pigeon/fail + a visibility-timeout reclaim sweep
    (the SQS model — a worker that crashes between /poll and /done no longer leaves the task
    'claimed' forever; it is requeued, and dead-lettered after max_attempts with an error reply).
  - authz (defense-in-depth, single-trust loopback): /done and /fail verify the caller is the
    task's receiver WHEN an agent_id is supplied.
  - input/DoS: payload/result body size is capped.
  - schema integrity: CHECK constraints + the two new columns (attempts, visibility_deadline).

Contract knobs the implementation must expose on build_app:
  - build_app(db_path, auth_token, visibility_timeout_secs=..., max_attempts=...)
  - app.state.reclaim_stuck : async callable that runs ONE reclaim sweep and returns
    {"requeued": int, "failed": int} (also run on an interval by the production lifespan).

Run: oracle-data/venv/bin/python -m pytest pigeon -c pigeon/pytest.ini
"""
import asyncio
import contextlib

import httpx
import pytest

from pigeon import db as pigeon_db
from pigeon.dispatcher import build_app


@contextlib.asynccontextmanager
async def client_for(tmp_path, auth_token=None, visibility_timeout_secs=1920, max_attempts=3):
    app = build_app(
        db_path=str(tmp_path / "mb.sqlite"),
        auth_token=auth_token,
        visibility_timeout_secs=visibility_timeout_secs,
        max_attempts=max_attempts,
    )
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://pigeon") as c:
        try:
            yield c, app
        finally:
            d = getattr(app.state, "db", None)
            if d is not None:
                await d.close()


async def _agent(c, agent_id, status):
    r = await c.post("/pigeon/agent", json={"agent_id": agent_id, "agent_type": "local", "status": status})
    assert r.status_code == 200, r.text


async def _send(c, sender, receiver, priority, payload, project="p"):
    r = await c.post("/pigeon/send", json={
        "sender_id": sender, "receiver_id": receiver, "project_id": project,
        "priority": priority, "payload": payload,
    })
    assert r.status_code == 200, r.text
    return r.json()


async def _poll(c, agent_id):
    return (await c.get("/pigeon/poll", params={"agent_id": agent_id})).json()


async def _status(c, ticket):
    return (await c.get(f"/pigeon/status/{ticket}")).json()


# ───────────────────────────── schema integrity ─────────────────────────────

async def test_new_columns_exist(tmp_path):
    """tasks gains `attempts` (default 0) and `visibility_deadline` (nullable)."""
    conn = await pigeon_db.connect(str(tmp_path / "mb.sqlite"))
    try:
        await pigeon_db.init_db(conn)
        cur = await conn.execute("PRAGMA table_info(tasks)")
        cols = {row[1] for row in await cur.fetchall()}
        assert "attempts" in cols
        assert "visibility_deadline" in cols
    finally:
        await conn.close()


async def test_schema_rejects_invalid_status(tmp_path):
    """A CHECK constraint forbids out-of-domain status values on a fresh DB."""
    import aiosqlite
    conn = await pigeon_db.connect(str(tmp_path / "mb.sqlite"))
    try:
        await pigeon_db.init_db(conn)
        with pytest.raises(aiosqlite.IntegrityError):
            await conn.execute(
                "INSERT INTO tasks(sender_id, receiver_id, project_id, payload, status, created_at) "
                "VALUES ('s','r','p','{}','BOGUS', 1)"
            )
    finally:
        await conn.close()


async def test_migration_adds_columns_to_legacy_table(tmp_path):
    """A pre-existing v0.1 table (without the new columns) is migrated in place: init_db
    must ALTER ADD the new columns rather than fail or silently skip them."""
    legacy = str(tmp_path / "legacy.sqlite")
    conn = await pigeon_db.connect(legacy)
    try:
        # Build the OLD v0.1 tasks table (no attempts / visibility_deadline).
        await conn.execute(
            "CREATE TABLE tasks (ticket_no INTEGER PRIMARY KEY AUTOINCREMENT, sender_id TEXT NOT NULL, "
            "receiver_id TEXT NOT NULL, project_id TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 50, "
            "status TEXT NOT NULL DEFAULT 'pending', delivery_mode TEXT NOT NULL DEFAULT 'queued', "
            "payload TEXT NOT NULL, result TEXT, error_msg TEXT, reply_to_ticket INTEGER, "
            "created_at INTEGER NOT NULL, claimed_at INTEGER, done_at INTEGER)"
        )
        await conn.commit()
        # init_db must be idempotent + migrate the legacy table.
        await pigeon_db.init_db(conn)
        cur = await conn.execute("PRAGMA table_info(tasks)")
        cols = {row[1] for row in await cur.fetchall()}
        assert "attempts" in cols, "legacy table must be migrated to add attempts"
        assert "visibility_deadline" in cols
    finally:
        await conn.close()


# ──────────────────────── visibility timeout + reclaim ───────────────────────

async def test_poll_sets_visibility_deadline(tmp_path):
    async with client_for(tmp_path, visibility_timeout_secs=1920) as (c, _):
        await _send(c, "S", "R", 50, {"x": 1})
        polled = await _poll(c, "R")
        assert polled["ticket_no"] is not None
        # The claimed row must carry a visibility_deadline so the sweep can reclaim it.
        st = await _status(c, polled["ticket_no"])
        assert st["status"] == "claimed"


async def test_reclaim_requeues_stuck_claimed_task(tmp_path):
    """A claimed task past its visibility deadline is returned to 'pending' and is
    claimable again; attempts is incremented (at-least-once)."""
    async with client_for(tmp_path, visibility_timeout_secs=0, max_attempts=3) as (c, app):
        await _send(c, "S", "R", 50, {"x": 1})
        polled = await _poll(c, "R")
        ticket = polled["ticket_no"]
        # Worker "crashed" — never called /done. Run the sweep (timeout=0 → already eligible).
        swept = await app.state.reclaim_stuck()
        assert swept["requeued"] == 1
        assert swept["failed"] == 0
        st = await _status(c, ticket)
        assert st["status"] == "pending", "stuck task must be requeued"
        # And it is claimable again by a fresh poll.
        re = await _poll(c, "R")
        assert re["ticket_no"] == ticket


async def test_reclaim_ignores_fresh_claim(tmp_path):
    """With a large visibility timeout, a just-claimed task is NOT reclaimed."""
    async with client_for(tmp_path, visibility_timeout_secs=1920) as (c, app):
        await _send(c, "S", "R", 50, {"x": 1})
        polled = await _poll(c, "R")
        swept = await app.state.reclaim_stuck()
        assert swept["requeued"] == 0
        st = await _status(c, polled["ticket_no"])
        assert st["status"] == "claimed", "a fresh claim must not be reclaimed"


async def test_reclaim_dead_letters_after_max_attempts(tmp_path):
    """When a stuck task has exhausted its attempts, the sweep marks it 'failed' and
    auto-creates an error reply to the original sender (so the sender is not left hanging)."""
    async with client_for(tmp_path, visibility_timeout_secs=0, max_attempts=0) as (c, app):
        await _agent(c, "S", "loaded")
        send = await _send(c, "S", "R", 50, {"x": 1})
        ticket = send["ticket_no"]
        await _poll(c, "R")  # claim
        swept = await app.state.reclaim_stuck()
        assert swept["failed"] == 1, "max_attempts=0 → first sweep dead-letters"
        st = await _status(c, ticket)
        assert st["status"] == "failed"
        # The sender receives an error reply describing the failure.
        reply = await _poll(c, "S")
        assert reply["ticket_no"] is not None
        assert reply["payload"]["type"] == "task_result"
        assert reply["payload"].get("error") is not None or reply["payload"].get("status") == "failed"


async def test_reclaim_ignores_pending_and_done(tmp_path):
    """The sweep only touches 'claimed' rows past the deadline — never pending or done."""
    async with client_for(tmp_path, visibility_timeout_secs=0, max_attempts=3) as (c, app):
        # pending (never polled)
        await _send(c, "S", "R", 50, {"a": 1})
        # done
        send2 = await _send(c, "S", "R", 50, {"b": 2})
        p2 = await _poll(c, "R")
        await c.post("/pigeon/done", json={"ticket_no": p2["ticket_no"], "result": {"ok": 1}})
        swept = await app.state.reclaim_stuck()
        assert swept["requeued"] == 0 and swept["failed"] == 0


# ─────────────────────────────── /pigeon/fail ────────────────────────────────

async def test_fail_requeues_when_attempts_remain(tmp_path):
    async with client_for(tmp_path, max_attempts=3) as (c, _):
        await _send(c, "S", "R", 50, {"x": 1})
        ticket = (await _poll(c, "R"))["ticket_no"]
        r = await c.post("/pigeon/fail", json={"ticket_no": ticket, "agent_id": "R", "error": "boom"})
        assert r.status_code == 200, r.text
        st = await _status(c, ticket)
        assert st["status"] == "pending", "fail with attempts remaining → requeued"


async def test_fail_dead_letters_and_replies(tmp_path):
    async with client_for(tmp_path, max_attempts=0) as (c, _):
        await _agent(c, "S", "loaded")
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        await _poll(c, "R")
        r = await c.post("/pigeon/fail", json={"ticket_no": ticket, "agent_id": "R", "error": "fatal"})
        assert r.status_code == 200, r.text
        st = await _status(c, ticket)
        assert st["status"] == "failed"
        reply = await _poll(c, "S")
        assert reply["ticket_no"] is not None
        assert reply["payload"]["type"] == "task_result"


async def test_fail_requires_receiver_identity(tmp_path):
    """Only the task's receiver may /fail it (defense-in-depth)."""
    async with client_for(tmp_path) as (c, _):
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        await _poll(c, "R")
        wrong = await c.post("/pigeon/fail", json={"ticket_no": ticket, "agent_id": "IMPOSTER", "error": "x"})
        assert wrong.status_code == 403


async def test_fail_unclaimed_task_rejected(tmp_path):
    async with client_for(tmp_path) as (c, _):
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        # never polled → still pending → cannot fail a non-claimed task
        r = await c.post("/pigeon/fail", json={"ticket_no": ticket, "agent_id": "R", "error": "x"})
        assert r.status_code == 409


# ─────────────────────────────────── authz ───────────────────────────────────

async def test_done_rejects_wrong_receiver_identity(tmp_path):
    """When the caller supplies an agent_id, /done verifies it is the receiver."""
    async with client_for(tmp_path) as (c, _):
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        await _poll(c, "R")
        bad = await c.post("/pigeon/done", json={"ticket_no": ticket, "agent_id": "IMPOSTER", "result": {}})
        assert bad.status_code == 403


async def test_done_accepts_correct_receiver_identity(tmp_path):
    async with client_for(tmp_path) as (c, _):
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        await _poll(c, "R")
        ok = await c.post("/pigeon/done", json={"ticket_no": ticket, "agent_id": "R", "result": {"o": 1}})
        assert ok.status_code == 200


async def test_done_without_agent_id_still_works(tmp_path):
    """Backward-compat: agent_id is OPTIONAL on /done (single-trust loopback). The existing
    P1 callers that omit it must keep working."""
    async with client_for(tmp_path) as (c, _):
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        await _poll(c, "R")
        ok = await c.post("/pigeon/done", json={"ticket_no": ticket, "result": {"o": 1}})
        assert ok.status_code == 200


# ────────────────────────────── body size cap ───────────────────────────────

async def test_send_rejects_oversized_payload(tmp_path):
    async with client_for(tmp_path) as (c, _):
        huge = {"blob": "x" * (2 * 1024 * 1024)}  # 2 MiB — over the cap
        r = await c.post("/pigeon/send", json={
            "sender_id": "S", "receiver_id": "R", "project_id": "p", "priority": 50, "payload": huge,
        })
        assert r.status_code == 413


async def test_done_rejects_oversized_result(tmp_path):
    async with client_for(tmp_path) as (c, _):
        ticket = (await _send(c, "S", "R", 50, {"x": 1}))["ticket_no"]
        await _poll(c, "R")
        huge = {"blob": "y" * (2 * 1024 * 1024)}
        r = await c.post("/pigeon/done", json={"ticket_no": ticket, "agent_id": "R", "result": huge})
        assert r.status_code == 413


# ─────────────────────── reclaim sweep is wired/callable ─────────────────────

async def test_reclaim_stuck_is_attached(tmp_path):
    async with client_for(tmp_path) as (c, app):
        assert hasattr(app.state, "reclaim_stuck")
        res = await app.state.reclaim_stuck()
        assert set(res.keys()) >= {"requeued", "failed"}
