from pathlib import Path
from typing import Mapping
from dataclasses import dataclass

@dataclass(frozen=True)
class Settings:
    port: int
    pigeon_dir: Path
    sqlite_path: Path
    auth_token: str | None

def load_settings(env: Mapping[str, str]) -> Settings:
    port_str = env.get("PIGEON_PORT")
    try:
        port = int(port_str) if port_str else 8769
    except (ValueError, TypeError):
        port = 8769
    pigeon_dir = Path(env.get("PIGEON_DIR", "pigeon-data"))

    if "PIGEON_SQLITE_PATH" in env:
        sqlite_path = Path(env["PIGEON_SQLITE_PATH"])
    else:
        sqlite_path = pigeon_dir / "mailbox.sqlite"

    auth_token = env.get("PIGEON_AUTH_TOKEN") or None

    return Settings(port=port, pigeon_dir=pigeon_dir, sqlite_path=sqlite_path, auth_token=auth_token)
