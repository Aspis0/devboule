import aiosqlite
from pathlib import Path

async def connect(path: str) -> aiosqlite.Connection:
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    conn = await aiosqlite.connect(path, isolation_level=None)
    conn.row_factory = aiosqlite.Row
    cur = await conn.execute("PRAGMA journal_mode=WAL")
    row = await cur.fetchone()
    mode = row[0] if row else None
    if mode and mode.lower() != "wal":
        raise RuntimeError(f"WAL not enabled: {mode}")
    await conn.execute("PRAGMA busy_timeout=5000")
    return conn

async def init_db(conn) -> None:
    await conn.executescript("""
    CREATE TABLE IF NOT EXISTS tasks (
        ticket_no       INTEGER PRIMARY KEY AUTOINCREMENT,
        sender_id       TEXT    NOT NULL,
        receiver_id     TEXT    NOT NULL,
        project_id      TEXT    NOT NULL,
        priority        INTEGER NOT NULL DEFAULT 50,
        status          TEXT    NOT NULL DEFAULT 'pending',
        delivery_mode   TEXT    NOT NULL DEFAULT 'queued',
        payload         TEXT    NOT NULL,
        result          TEXT,
        error_msg       TEXT,
        reply_to_ticket INTEGER,
        created_at      INTEGER NOT NULL,
        claimed_at      INTEGER,
        done_at         INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_tasks_poll ON tasks(receiver_id, status, priority, created_at);
    CREATE TABLE IF NOT EXISTS agents (
        agent_id   TEXT PRIMARY KEY,
        agent_type TEXT NOT NULL DEFAULT 'local',
        status     TEXT NOT NULL DEFAULT 'unloaded',
        last_seen  INTEGER
    );
    """)
    await conn.commit()
