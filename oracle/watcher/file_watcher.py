import threading
from collections import deque
from pathlib import Path
from typing import Callable

from oracle.config import WATCH_DEBOUNCE, WATCH_DIRS, WATCH_EXTENSIONS


class OracleWatcher:
    def __init__(self, on_batch_ready: Callable[[list[str]], None], debounce_seconds: int = WATCH_DEBOUNCE):
        self.queue: deque[str] = deque()
        self.timer: threading.Timer | None = None
        self.on_batch_ready = on_batch_ready
        self.debounce_seconds = debounce_seconds
        self.lock = threading.Lock()

    def enqueue(self, path: str) -> None:
        if Path(path).suffix.lower() not in WATCH_EXTENSIONS:
            return
        with self.lock:
            self.queue.append(path)
            if self.timer:
                self.timer.cancel()
            self.timer = threading.Timer(self.debounce_seconds, self.flush)
            self.timer.daemon = True
            self.timer.start()

    def flush(self) -> None:
        with self.lock:
            batch = sorted(set(self.queue))
            self.queue.clear()
            if self.timer:
                self.timer.cancel()
            self.timer = None
        if batch:
            self.on_batch_ready(batch)


def start_watching(on_batch_ready: Callable[[list[str]], None], root: Path | str = "."):
    try:
        from watchdog.events import FileSystemEventHandler
        from watchdog.observers import Observer
    except Exception as exc:  # pragma: no cover
        raise RuntimeError("Install oracle/requirements.txt to use Oracle watcher.") from exc

    root = Path(root)
    debouncer = OracleWatcher(on_batch_ready)

    class Handler(FileSystemEventHandler):
        def on_modified(self, event):  # type: ignore[override]
            if not event.is_directory:
                debouncer.enqueue(event.src_path)

        def on_created(self, event):  # type: ignore[override]
            if not event.is_directory:
                debouncer.enqueue(event.src_path)

    observer = Observer()
    handler = Handler()
    for watch_dir in WATCH_DIRS:
        target = root / watch_dir
        if target.exists():
            observer.schedule(handler, str(target), recursive=True)
    observer.start()
    return observer
