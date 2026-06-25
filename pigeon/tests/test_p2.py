"""P2 — visibility endpoints: GET /pigeon/status/{ticket_no}, GET /pigeon/queue/{agent_id}.
Read-only views over the mailbox for the dashboard/debugging.
"""
import contextlib

import httpx

from pigeon.dispatcher import build_app


@contextlib.asynccontextmanager
async def client_for(tmp_path):
    app = build_app(db_path=str(tmp_path / "mb.sqlite"))
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://pigeon") as c:
        try:
            yield c, app
        finally:
            db = getattr(app.state, "db", None)
            if db is not None:
                await db.close()


async def _send(c, sender, receiver, priority, payload, project="p"):
    r = await c.post(
        "/pigeon/send",
        json={
            "sender_id": sender,
            "receiver_id": receiver,
            "project_id": project,
            "priority": priority,
            "payload": payload,
        },
    )
    assert r.status_code == 200, r.text
    return r.json()


async def test_status_tracks_lifecycle(tmp_path):
    async with client_for(tmp_path) as (c, _):
        t = (await _send(c, "S", "R", 50, {"k": 1}))["ticket_no"]
        s = (await c.get(f"/pigeon/status/{t}")).json()
        assert s["status"] == "pending"
        assert s["delivery_mode"] == "queued"
        assert s["result"] is None

        await c.get("/pigeon/poll", params={"agent_id": "R"})
        assert (await c.get(f"/pigeon/status/{t}")).json()["status"] == "claimed"

        await c.post("/pigeon/done", json={"ticket_no": t, "result": {"o": 9}})
        done = (await c.get(f"/pigeon/status/{t}")).json()
        assert done["status"] == "done"
        assert done["result"] == {"o": 9}


async def test_status_404(tmp_path):
    async with client_for(tmp_path) as (c, _):
        assert (await c.get("/pigeon/status/999")).status_code == 404


async def test_queue_lists_pending_in_order(tmp_path):
    async with client_for(tmp_path) as (c, _):
        await _send(c, "S", "R", 70, {"n": "low"})
        await _send(c, "S", "R", 10, {"n": "high"})
        await _send(c, "S", "OTHER", 50, {"n": "other"})  # different receiver, excluded
        q = (await c.get("/pigeon/queue/R")).json()
        pend = q["pending"]
        assert [p["payload"]["n"] for p in pend] == ["high", "low"]  # priority ASC
        assert all("ticket_no" in p for p in pend)

        # polling one removes it from the pending queue
        await c.get("/pigeon/poll", params={"agent_id": "R"})
        q2 = (await c.get("/pigeon/queue/R")).json()
        assert [p["payload"]["n"] for p in q2["pending"]] == ["low"]


async def test_queue_empty(tmp_path):
    async with client_for(tmp_path) as (c, _):
        q = (await c.get("/pigeon/queue/nobody")).json()
        assert q["pending"] == []
