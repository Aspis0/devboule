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
    await conn.execute("PRAGMA foreign_keys=ON")
    return conn

async def init_db(conn) -> None:
    await conn.executescript("""
    CREATE TABLE IF NOT EXISTS tasks (
        ticket_no           INTEGER PRIMARY KEY AUTOINCREMENT,
        sender_id           TEXT    NOT NULL,
        receiver_id         TEXT    NOT NULL,
        project_id          TEXT    NOT NULL,
        priority            INTEGER NOT NULL DEFAULT 50 CHECK (priority BETWEEN 0 AND 100),
        status              TEXT    NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','claimed','done','failed')),
        delivery_mode       TEXT    NOT NULL DEFAULT 'queued' CHECK (delivery_mode IN ('queued','immediate')),
        payload             TEXT    NOT NULL,
        result              TEXT,
        error_msg           TEXT,
        reply_to_ticket     INTEGER REFERENCES tasks(ticket_no),
        attempts            INTEGER NOT NULL DEFAULT 0,
        visibility_deadline INTEGER,
        created_at          INTEGER NOT NULL,
        claimed_at          INTEGER,
        done_at             INTEGER
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
    await _migrate_tasks(conn)


async def _migrate_tasks(conn) -> None:
    """Idempotently bring a pre-existing v0.1 `tasks` table up to the current schema by
    ALTER-ADDing any missing columns. CHECK/FK constraints only apply to freshly-created
    tables; that is acceptable for this default-off, pre-production feature."""
    cur = await conn.execute("PRAGMA table_info(tasks)")
    existing = {row[1] for row in await cur.fetchall()}
    additions = {
        "attempts": "ALTER TABLE tasks ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
        "visibility_deadline": "ALTER TABLE tasks ADD COLUMN visibility_deadline INTEGER",
    }
    migrated = False
    for col, ddl in additions.items():
        if col not in existing:
            await conn.execute(ddl)
            migrated = True
    if migrated:
        await conn.commit()
