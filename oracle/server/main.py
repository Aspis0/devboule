import os
import socket
import sys
import threading
import time

from oracle.config import DISABLE_IDLE_EXIT, ORACLE_PORT, QUERY_IDLE_TIMEOUT

# IMPORTANT: heavy imports (FastAPI, the routes/router and the index-job manager)
# are deliberately NOT done at module top. They are pulled in only by `build_app()`,
# which `__main__` calls AFTER `_bind_listen_socket` has successfully claimed the
# fixed session port. This makes a DUPLICATE spawn (two supervisors racing one port)
# collide on the bind and `os._exit(1)` within milliseconds, instead of lingering for
# seconds in slow Python import startup (FastAPI/uvicorn/torch transitively) before it
# ever reaches the bind — the untracked "loser" zombie that motivated this change.
# Importing this module (e.g. a test doing `from oracle.server.main import app`) must
# NOT bind a socket: only `__main__` binds. The lazy `app` accessor below builds the
# app on first attribute access without binding, preserving import-time behavior.

_last_request_at = time.monotonic()

# Cached lazily-built FastAPI app (built by `build_app()` on first use). Kept module-
# level so repeated `build_app()` / `app` accesses return the same instance.
_app = None


def build_app():
    """Construct (once) and return the FastAPI app with routes + middleware wired.

    Lazy by design: the heavy imports happen HERE, not at module import time, so a
    duplicate server process binds (and fails fast on a port collision) before paying
    the import cost. Idempotent — the constructed app is cached in `_app`.
    """
    global _app
    if _app is not None:
        return _app

    try:
        from fastapi import FastAPI
    except Exception as exc:  # pragma: no cover
        raise RuntimeError("Install oracle/requirements.txt to run the Oracle server.") from exc

    from oracle.server.index_jobs import manager as index_job_manager
    from oracle.server.routes import create_router

    app = FastAPI(title="Architecture Oracle", version="1.0")
    app.include_router(create_router())

    @app.middleware("http")
    async def track_activity(request, call_next):
        global _last_request_at
        _last_request_at = time.monotonic()
        response = await call_next(request)
        _last_request_at = time.monotonic()
        return response

    @app.on_event("startup")
    def start_idle_reaper():
        if DISABLE_IDLE_EXIT:
            return

        def reap_when_idle():
            while True:
                time.sleep(1)
                if index_job_manager.keepalive_active():
                    continue
                if time.monotonic() - _last_request_at >= QUERY_IDLE_TIMEOUT:
                    os._exit(0)

        thread = threading.Thread(target=reap_when_idle, daemon=True)
        thread.start()

    _app = app
    return app


def __getattr__(name):
    """PEP 562 module-level attribute hook.

    Lets `from oracle.server.main import app` (or `main.app`) keep working by building
    the app lazily on first access — WITHOUT binding any socket. Only `__main__` binds.
    """
    if name == "app":
        return build_app()
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def _bind_listen_socket(host: str, port: int) -> socket.socket:
    """Bind the loopback listen socket OURSELVES, before handing it to uvicorn.

    The app supervises this process on a fixed per-session port and respawns it
    if it dies. If the port is already held (a leftover/zombie from a prior
    spawn, [Errno 10048]/EADDRINUSE), uvicorn's own bind happens deep inside its
    startup and — depending on version/race — can log "Application startup
    complete" without ever owning a listening socket, leaving a process the Rust
    child-died detector still sees as alive but that never answers /health. That
    is the exact zombie that drives the respawn loop.

    Binding here makes the failure DETERMINISTIC and IMMEDIATE: on any bind error
    we `os._exit(1)` right away (before uvicorn starts), so the Rust supervisor's
    child-exited detection is accurate and no unbound zombie can accumulate.
    On success we pass the already-bound socket to uvicorn via `Server.run(
    sockets=[...])`, so uvicorn never re-binds.

    `__main__` calls this FIRST — before `build_app()` does the heavy FastAPI/route
    imports — so a duplicate spawn collides on the bind and exits in milliseconds
    rather than lingering through import startup.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    # Do NOT set SO_REUSEADDR/SO_REUSEPORT: we WANT the bind to fail hard when the
    # port is genuinely held by a live process, so a collision exits instead of
    # silently sharing/stealing the port and producing a half-bound zombie.
    try:
        sock.bind((host, port))
        sock.listen()
    except OSError as exc:
        # Port already in use (or otherwise unbindable). Exit non-zero IMMEDIATELY
        # so the supervising parent observes a clean child exit and can reconcile,
        # instead of a lingering process with no listener. Flush first so the
        # diagnostic reaches the redirected stderr log before the hard exit.
        try:
            sock.close()
        except OSError:
            pass
        sys.stderr.write(
            f"oracle-server: could not bind 127.0.0.1:{port} ({exc}); exiting\n"
        )
        sys.stderr.flush()
        os._exit(1)
    return sock


if __name__ == "__main__":
    # Bind the fixed session port FIRST, before any heavy import / app construction,
    # so a duplicate process collides on the bind and `os._exit(1)`s in milliseconds.
    listen_socket = _bind_listen_socket("127.0.0.1", ORACLE_PORT)

    # Only after we own the port do we pay the heavy FastAPI/routes import cost.
    import uvicorn

    app = build_app()
    config = uvicorn.Config(app, host="127.0.0.1", port=ORACLE_PORT)
    server = uvicorn.Server(config)
    # Hand uvicorn the socket we already own so it never performs its own bind
    # (which is where the silent half-bound "startup complete" zombie came from).
    server.run(sockets=[listen_socket])
