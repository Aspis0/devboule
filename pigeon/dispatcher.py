import asyncio
import contextlib
import json
import time
import hmac
import os
from pathlib import Path

from fastapi import FastAPI, Request, HTTPException, Query, Depends
from pigeon.db import connect, init_db
from pigeon.models import SendRequest, SendResponse, PollResponse, DoneRequest, FailRequest, Agent

MAX_BODY_BYTES = 256 * 1024
RECLAIM_SWEEP_INTERVAL_SECS = 30


def build_app(
    db_path: str | None = None,
    auth_token: str | None = None,
    visibility_timeout_secs: int = 1920,
    max_attempts: int = 3,
) -> FastAPI:
    if db_path is None:
        from pigeon.config import load_settings
        settings = load_settings(os.environ)
        db_path = settings.sqlite_path
        auth_token = settings.auth_token

    @contextlib.asynccontextmanager
    async def lifespan(app: FastAPI):
        async def _sweeper():
            while True:
                await asyncio.sleep(RECLAIM_SWEEP_INTERVAL_SECS)
                try:
                    await app.state.reclaim_stuck()
                except asyncio.CancelledError:
                    raise
                except BaseException:
                    # One failed sweep must not kill the loop.
                    pass

        sweeper_task = asyncio.create_task(_sweeper())
        try:
            yield
        finally:
            sweeper_task.cancel()
            with contextlib.suppress(asyncio.CancelledError, BaseException):
                await sweeper_task
            conn = app.state.db
            if conn is not None:
                with contextlib.suppress(BaseException):
                    await conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                with contextlib.suppress(BaseException):
                    await conn.close()
                app.state.db = None

    app = FastAPI(lifespan=lifespan)

    app.state.db_path = db_path
    app.state.auth_token = auth_token
    app.state.visibility_timeout_secs = visibility_timeout_secs
    app.state.max_attempts = max_attempts
    app.state.db = None
    app.state.db_lock = asyncio.Lock()
    app.state.tx_lock = asyncio.Lock()

    async def get_db():
        if app.state.db is None:
            async with app.state.db_lock:
                if app.state.db is None:
                    conn = await connect(app.state.db_path)
                    try:
                        await init_db(conn)
                    except BaseException:
                        await conn.close()
                        raise
                    app.state.db = conn
        return app.state.db

    async def require_auth(request: Request):
        token = app.state.auth_token
        if not token:
            return
        header = request.headers.get("x-pigeon-auth-token")
        if header is None or not hmac.compare_digest(header, token):
            raise HTTPException(status_code=401, detail="Unauthorized")

    async def _create_reply(conn, task, now, *, original_ticket, status, result=None, error=None):
        """Auto-create a reply to the original sender. Mirrors the /done reply-creation
        logic; must run inside the caller's open transaction. Returns the reply ticket_no."""
        agent_cur = await conn.execute(
            "SELECT status FROM agents WHERE agent_id = ?", (task["sender_id"],)
        )
        sender_agent = await agent_cur.fetchone()
        sender_status = sender_agent["status"] if sender_agent else "unloaded"
        reply_delivery_mode = "immediate" if sender_status == "loaded" else "queued"
        body = {"type": "task_result", "original_ticket": original_ticket, "status": status}
        if result is not None:
            body["result"] = result
        if error is not None:
            body["error"] = error
        reply_cursor = await conn.execute(
            "INSERT INTO tasks(sender_id, receiver_id, project_id, priority, status, delivery_mode, payload, reply_to_ticket, created_at) "
            "VALUES (?, ?, ?, 10, 'pending', ?, ?, ?, ?)",
            (task["receiver_id"], task["sender_id"], task["project_id"],
             reply_delivery_mode, json.dumps(body), original_ticket, now),
        )
        return reply_cursor.lastrowid

    async def _requeue_or_dead_letter(conn, task, now, error_msg):
        """Shared at-least-once logic for the reclaim sweep and POST /fail. Caller holds
        tx_lock and an open BEGIN IMMEDIATE transaction. Returns 'requeued' or 'failed'.

        DELIVERY SEMANTICS (review S2-2): this is AT-LEAST-ONCE, not exactly-once. After a
        requeue a second worker may run the task; if the original (slow) worker later calls
        /done on the now-reclaimed ticket it can store a stale result. In THIS deployment that
        race is practically unreachable: the visibility timeout (default 1920s) is set ABOVE the
        mini-coder wall-clock cap (~1800s), so the executor has already killed the original mini
        before the sweep reclaims. A claim-generation token (epoch) would close it fully — tracked
        in the go-live hardening backlog, deferred to avoid coupling the Slice-3 wiring protocol.
        The `AND status='claimed'` guards below are defensive (no-op under the single-process
        tx_lock today, correctness-preserving if Pigeon ever runs multi-process)."""
        if task["attempts"] < app.state.max_attempts:
            await conn.execute(
                "UPDATE tasks SET status='pending', attempts=attempts+1, claimed_at=NULL, "
                "visibility_deadline=NULL WHERE ticket_no=? AND status='claimed'",
                (task["ticket_no"],),
            )
            return "requeued"
        await conn.execute(
            "UPDATE tasks SET status='failed', error_msg=?, visibility_deadline=NULL "
            "WHERE ticket_no=? AND status='claimed'",
            (error_msg, task["ticket_no"]),
        )
        await _create_reply(
            conn, task, now,
            original_ticket=task["ticket_no"], status="failed", error=error_msg,
        )
        return "failed"

    async def reclaim_stuck():
        conn = await get_db()
        now = int(time.time())
        requeued = 0
        failed = 0
        async with app.state.tx_lock:
            await conn.execute("BEGIN IMMEDIATE")
            try:
                cur = await conn.execute(
                    "SELECT ticket_no, sender_id, receiver_id, project_id, attempts "
                    "FROM tasks WHERE status='claimed' AND visibility_deadline IS NOT NULL "
                    "AND visibility_deadline <= ?",
                    (now,),
                )
                rows = await cur.fetchall()
                for task in rows:
                    outcome = await _requeue_or_dead_letter(
                        conn, task, now, "reclaimed: visibility timeout exceeded"
                    )
                    if outcome == "requeued":
                        requeued += 1
                    else:
                        failed += 1
                await conn.execute("COMMIT")
            except BaseException:
                # Suppress a ROLLBACK throw (review S2-3): if ROLLBACK itself fails, the single
                # shared connection would stay mid-transaction forever, permanently breaking every
                # later BEGIN IMMEDIATE on /done, /fail and the sweep. Best-effort rollback.
                try:
                    await conn.execute("ROLLBACK")
                except Exception:
                    pass
                raise
        return {"requeued": requeued, "failed": failed}

    app.state.reclaim_stuck = reclaim_stuck

    @app.get("/health")
    async def health():
        return {
            "server_root": str(Path.cwd().resolve()),
            "service": "pigeon",
            "auth": "enabled" if app.state.auth_token else "disabled",
        }

    @app.post("/pigeon/agent")
    async def register_agent(body: Agent, _auth: None = Depends(require_auth)):
        conn = await get_db()
        now = int(time.time())
        async with app.state.tx_lock:
            await conn.execute(
                "INSERT INTO agents(agent_id, agent_type, status, last_seen) VALUES (?, ?, ?, ?) "
                "ON CONFLICT(agent_id) DO UPDATE SET agent_type=excluded.agent_type, status=excluded.status, last_seen=excluded.last_seen",
                (body.agent_id, body.agent_type, body.status, now)
            )
            await conn.commit()
        return {"ok": True}

    @app.post("/pigeon/send")
    async def send_message(body: SendRequest, _auth: None = Depends(require_auth)):
        conn = await get_db()
        payload_json = json.dumps(body.payload)
        if len(payload_json) > MAX_BODY_BYTES:
            raise HTTPException(status_code=413, detail="payload too large")
        async with app.state.tx_lock:
            row = await conn.execute(
                "SELECT status FROM agents WHERE agent_id = ?", (body.receiver_id,)
            )
            agent_row = await row.fetchone()
            receiver_status = agent_row["status"] if agent_row else "unloaded"
            delivery_mode = "immediate" if receiver_status == "loaded" else "queued"

            now = int(time.time())
            cursor = await conn.execute(
                "INSERT INTO tasks(sender_id, receiver_id, project_id, priority, status, delivery_mode, payload, created_at) "
                "VALUES (?, ?, ?, ?, 'pending', ?, ?, ?)",
                (body.sender_id, body.receiver_id, body.project_id, body.priority, delivery_mode, payload_json, now)
            )
            await conn.commit()
        ticket_no = cursor.lastrowid
        return SendResponse(
            ticket_no=ticket_no,
            status="pending",
            delivery_mode=delivery_mode,
            receiver_status=receiver_status,
        )

    @app.get("/pigeon/poll")
    async def poll_task(agent_id: str = Query(...), _auth: None = Depends(require_auth)):
        conn = await get_db()
        now = int(time.time())
        deadline = now + app.state.visibility_timeout_secs
        async with app.state.tx_lock:
            cursor = await conn.execute(
                "UPDATE tasks SET status='claimed', claimed_at=:now, visibility_deadline=:deadline "
                "WHERE ticket_no = (SELECT ticket_no FROM tasks WHERE receiver_id=:agent AND status='pending' "
                "ORDER BY priority ASC, ticket_no ASC LIMIT 1) RETURNING ticket_no, payload",
                {"now": now, "deadline": deadline, "agent": agent_id}
            )
            row = await cursor.fetchone()
            await conn.commit()
        if row:
            return PollResponse(ticket_no=row["ticket_no"], payload=json.loads(row["payload"]))
        return PollResponse(ticket_no=None, payload=None)

    @app.post("/pigeon/done")
    async def done_task(body: DoneRequest, _auth: None = Depends(require_auth)):
        conn = await get_db()
        now = int(time.time())
        result_json = json.dumps(body.result)
        if len(result_json) > MAX_BODY_BYTES:
            raise HTTPException(status_code=413, detail="result too large")
        async with app.state.tx_lock:
            await conn.execute("BEGIN IMMEDIATE")
            try:
                row = await conn.execute(
                    "SELECT sender_id, receiver_id, project_id, status FROM tasks WHERE ticket_no = ?",
                    (body.ticket_no,),
                )
                task = await row.fetchone()
                if task is None:
                    raise HTTPException(status_code=404, detail="Task not found")
                if body.agent_id is not None and body.agent_id != task["receiver_id"]:
                    raise HTTPException(status_code=403, detail="caller is not the task receiver")
                cur = await conn.execute(
                    "UPDATE tasks SET status='done', done_at=?, result=? WHERE ticket_no=? AND status='claimed'",
                    (now, result_json, body.ticket_no),
                )
                if cur.rowcount == 0:
                    raise HTTPException(status_code=409, detail="task is not in a completable (claimed) state")
                reply_ticket_no = await _create_reply(
                    conn, task, now,
                    original_ticket=body.ticket_no, status="done", result=body.result,
                )
                await conn.execute("COMMIT")
                return {"ok": True, "reply_ticket_no": reply_ticket_no}
            except BaseException:
                # Suppress a ROLLBACK throw (review S2-3): if ROLLBACK itself fails, the single
                # shared connection would stay mid-transaction forever, permanently breaking every
                # later BEGIN IMMEDIATE on /done, /fail and the sweep. Best-effort rollback.
                try:
                    await conn.execute("ROLLBACK")
                except Exception:
                    pass
                raise

    @app.post("/pigeon/fail")
    async def fail_task(body: FailRequest, _auth: None = Depends(require_auth)):
        conn = await get_db()
        now = int(time.time())
        # Byte cap (review S2-1): `body.error` is a RAW string (not json.dumps'd like payload/result,
        # which are ASCII-escaped so len()==bytes), so multibyte chars could bypass a codepoint cap.
        if len(body.error.encode("utf-8")) > MAX_BODY_BYTES:
            raise HTTPException(status_code=413, detail="error too large")
        async with app.state.tx_lock:
            await conn.execute("BEGIN IMMEDIATE")
            try:
                row = await conn.execute(
                    "SELECT ticket_no, sender_id, receiver_id, project_id, status, attempts "
                    "FROM tasks WHERE ticket_no = ?",
                    (body.ticket_no,),
                )
                task = await row.fetchone()
                if task is None:
                    raise HTTPException(status_code=404, detail="Task not found")
                if task["receiver_id"] != body.agent_id:
                    raise HTTPException(status_code=403, detail="caller is not the task receiver")
                if task["status"] != "claimed":
                    raise HTTPException(status_code=409, detail="task is not in a claimed state")
                outcome = await _requeue_or_dead_letter(conn, task, now, body.error)
                await conn.execute("COMMIT")
                return {"ok": True, "outcome": outcome}
            except BaseException:
                # Suppress a ROLLBACK throw (review S2-3): if ROLLBACK itself fails, the single
                # shared connection would stay mid-transaction forever, permanently breaking every
                # later BEGIN IMMEDIATE on /done, /fail and the sweep. Best-effort rollback.
                try:
                    await conn.execute("ROLLBACK")
                except Exception:
                    pass
                raise

    @app.get("/pigeon/status/{ticket_no}")
    async def task_status(ticket_no: int, _auth: None = Depends(require_auth)):
        conn = await get_db()
        async with app.state.tx_lock:
            cur = await conn.execute(
                "SELECT ticket_no, status, delivery_mode, result, sender_id, receiver_id, priority "
                "FROM tasks WHERE ticket_no = ?", (ticket_no,)
            )
            row = await cur.fetchone()
        if row is None:
            raise HTTPException(status_code=404, detail="Task not found")
        return {
            "ticket_no": row["ticket_no"],
            "status": row["status"],
            "delivery_mode": row["delivery_mode"],
            "result": json.loads(row["result"]) if row["result"] is not None else None,
            "sender_id": row["sender_id"],
            "receiver_id": row["receiver_id"],
            "priority": row["priority"],
        }

    @app.get("/pigeon/queue/{agent_id}")
    async def agent_queue(agent_id: str, _auth: None = Depends(require_auth)):
        conn = await get_db()
        async with app.state.tx_lock:
            cur = await conn.execute(
                "SELECT ticket_no, priority, delivery_mode, payload, created_at "
                "FROM tasks WHERE receiver_id = ? AND status = 'pending' "
                "ORDER BY priority ASC, ticket_no ASC", (agent_id,)
            )
            rows = await cur.fetchall()
        pending = [
            {
                "ticket_no": r["ticket_no"],
                "priority": r["priority"],
                "delivery_mode": r["delivery_mode"],
                "payload": json.loads(r["payload"]),
                "created_at": r["created_at"],
            }
            for r in rows
        ]
        return {"agent_id": agent_id, "pending": pending}

    return app


if __name__ == "__main__":
    import socket
    import uvicorn
    from pigeon.config import load_settings

    settings = load_settings(os.environ)
    port = settings.port

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", port))
    except OSError:
        raise SystemExit(1)
    sock.listen(128)

    app = build_app()

    config = uvicorn.Config(app, host="127.0.0.1", port=port, log_level="warning")
    server = uvicorn.Server(config)
    server.run(sockets=[sock])
