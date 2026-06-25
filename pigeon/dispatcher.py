import asyncio
import json
import time
import hmac
import os
from pathlib import Path

from fastapi import FastAPI, Request, HTTPException, Query, Depends
from pigeon.db import connect, init_db
from pigeon.models import SendRequest, SendResponse, PollResponse, DoneRequest, Agent
def build_app(db_path: str | None = None, auth_token: str | None = None) -> FastAPI:
    if db_path is None:
        from pigeon.config import load_settings
        settings = load_settings(os.environ)
        db_path = settings.sqlite_path
        auth_token = settings.auth_token

    app = FastAPI()

    app.state.db_path = db_path
    app.state.auth_token = auth_token
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
        async with app.state.tx_lock:
            row = await conn.execute(
                "SELECT status FROM agents WHERE agent_id = ?", (body.receiver_id,)
            )
            agent_row = await row.fetchone()
            receiver_status = agent_row["status"] if agent_row else "unloaded"
            delivery_mode = "immediate" if receiver_status == "loaded" else "queued"

            payload_json = json.dumps(body.payload)
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
        async with app.state.tx_lock:
            cursor = await conn.execute(
                "UPDATE tasks SET status='claimed', claimed_at=:now WHERE ticket_no = (SELECT ticket_no FROM tasks WHERE receiver_id=:agent AND status='pending' ORDER BY priority ASC, ticket_no ASC LIMIT 1) RETURNING ticket_no, payload",
                {"now": now, "agent": agent_id}
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
        async with app.state.tx_lock:
            await conn.execute("BEGIN IMMEDIATE")
            try:
                row = await conn.execute(
                    "SELECT sender_id, receiver_id, project_id FROM tasks WHERE ticket_no = ?",
                    (body.ticket_no,),
                )
                task = await row.fetchone()
                if task is None:
                    raise HTTPException(status_code=404, detail="Task not found")
                cur = await conn.execute(
                    "UPDATE tasks SET status='done', done_at=?, result=? WHERE ticket_no=? AND status='claimed'",
                    (now, json.dumps(body.result), body.ticket_no),
                )
                if cur.rowcount == 0:
                    raise HTTPException(status_code=409, detail="task is not in a completable (claimed) state")
                agent_cur = await conn.execute(
                    "SELECT status FROM agents WHERE agent_id = ?", (task["sender_id"],)
                )
                sender_agent = await agent_cur.fetchone()
                sender_status = sender_agent["status"] if sender_agent else "unloaded"
                reply_delivery_mode = "immediate" if sender_status == "loaded" else "queued"
                reply_payload = json.dumps({
                    "type": "task_result",
                    "original_ticket": body.ticket_no,
                    "result": body.result,
                })
                reply_cursor = await conn.execute(
                    "INSERT INTO tasks(sender_id, receiver_id, project_id, priority, status, delivery_mode, payload, reply_to_ticket, created_at) "
                    "VALUES (?, ?, ?, 10, 'pending', ?, ?, ?, ?)",
                    (task["receiver_id"], task["sender_id"], task["project_id"], reply_delivery_mode, reply_payload, body.ticket_no, now),
                )
                reply_ticket_no = reply_cursor.lastrowid
                await conn.execute("COMMIT")
                return {"ok": True, "reply_ticket_no": reply_ticket_no}
            except BaseException:
                await conn.execute("ROLLBACK")
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
