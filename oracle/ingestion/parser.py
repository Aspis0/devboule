from pathlib import Path

from oracle.config import WATCH_EXTENSIONS


MAX_FILE_BYTES = 240_000

# Directory components that must never be indexed when they appear anywhere in a
# path (case-insensitive). These are unconditional: any path that walks through
# one of these folders is rejected.
SENSITIVE_PATH_COMPONENTS = {
    ".secrets",
    "aspis-secrets",
    ".dev.vars",
    "node_modules",
    "oracle-data",
    "src-tauri/target",
    "target",
}

# Structural path substrings that mark the WHOLE path as sensitive regardless of
# the file extension. These are containers/manifests that never hold useful source
# and historically leaked credentials, so they are rejected unconditionally.
SENSITIVE_PATH_SUBSTRINGS = {
    "package-lock.json",
}

# Basename content words that signal a secret DUMP rather than legitimate source.
# Gated to data/text/config extensions (see SECRET_DATA_EXTENSIONS) so that real
# source code such as tokens.ts, SecretsView.tsx, and a scaleway-vault/ folder of
# .sh scripts stays indexed, while secrets.yaml / token.txt / creds.txt / vault.json
# do not. This is the README-mandated "tokens.ts is source" carve-out.
SECRET_CONTENT_WORDS = (
    "secret",
    "credential",
    "creds",
    "vault",
    "token",
    "apikey",
    "api-key",
    "api_key",
    "passwd",
    "password",
)

# Extensions for which the SECRET_CONTENT_WORDS basename rule applies. Code
# extensions are deliberately excluded so legitimate source survives.
SECRET_DATA_EXTENSIONS = (
    ".txt",
    ".json",
    ".yaml",
    ".yml",
    ".toml",
    ".ini",
    ".cfg",
    ".conf",
    ".env",
)

# Suffixes whose basenames are ALWAYS rejected (key/cert material, dev vars),
# regardless of any content word.
SENSITIVE_SUFFIXES = (
    ".key",
    ".pem",
    ".pfx",
    ".p12",
    ".dev.vars",
)

# Backward-compatible alias retained for callers/tests that referenced the old
# broad substring set. Prefer is_sensitive_relative_path for new code.
SENSITIVE_PARTS = {".env", "credential", "credentials", "secret", "secrets", "token", "vault"} | {
    "node_modules",
    "oracle-data",
    "package-lock.json",
    "target",
}


def _basename_is_secret(name: str) -> bool:
    """Default-deny basename check (case-insensitive).

    Rejects obvious secret containers while preserving legitimate source files
    like tokens.ts / tokens.py / tokens.tsx / SecretsView.tsx.
    """
    lower = name.lower()
    dot = lower.rfind(".")
    suffix = lower[dot:] if dot > 0 else ""
    # ".env" has no positive-index dot; handle dotfiles explicitly below.

    # Key / certificate material and Cloudflare dev vars (always rejected).
    if lower.endswith(SENSITIVE_SUFFIXES):
        return True
    # Private SSH keys: id_rsa, id_rsa.pub treated as sensitive too.
    if lower == "id_rsa" or lower.startswith("id_rsa"):
        return True
    # Dotenv files: exactly ".env" or any ".env.*" variant (always rejected).
    if lower == ".env" or lower.startswith(".env."):
        return True
    # Content-word secret dumps, only for data/text/config extensions so source
    # code with the same words in its name stays indexed.
    if suffix in SECRET_DATA_EXTENSIONS and any(word in lower for word in SECRET_CONTENT_WORDS):
        return True
    return False


def is_sensitive_relative_path(relative: str) -> bool:
    """Default-deny secret filter shared by both ingestion paths.

    `relative` is a POSIX-style path relative to the index root. Returns True if
    the path should NEVER be indexed/embedded.
    """
    text = relative.replace("\\", "/").lower()
    parts = [part for part in text.split("/") if part]
    if not parts:
        return False
    # Reject if any path component is an unconditional sensitive folder
    # (.secrets, aspis-secrets, .dev.vars, node_modules, target, oracle-data).
    for component in parts:
        if component in SENSITIVE_PATH_COMPONENTS:
            return True
    # src-tauri/target spans two components; catch it via substring too.
    if any(substring in text for substring in SENSITIVE_PATH_COMPONENTS):
        return True
    # Structural secret/manifest containers anywhere in the path.
    if any(substring in text for substring in SENSITIVE_PATH_SUBSTRINGS):
        return True
    # Finally apply the default-deny basename rule (handles .env, *.key, secrets.*,
    # token.txt, creds.txt, vault.json) while preserving source code.
    return _basename_is_secret(parts[-1])


def parse_file(path: Path | str, project_root: Path | str = ".") -> dict | None:
    root = Path(project_root).resolve()
    file_path = Path(path)
    if not file_path.is_absolute():
        file_path = root / file_path
    file_path = file_path.resolve()

    if not file_path.exists() or not file_path.is_file():
        return None
    if file_path.suffix.lower() not in WATCH_EXTENSIONS:
        return None
    if not path_allowed(file_path, root):
        return None

    raw = file_path.read_bytes()[:MAX_FILE_BYTES]
    if b"\x00" in raw:
        return None
    content = raw.decode("utf-8", errors="replace")
    relative = file_path.relative_to(root).as_posix()
    return {
        "id": relative,
        "label": file_path.name,
        "file_sorgente": relative,
        "content": content,
        "ultima_modifica": utc_mtime(file_path),
    }


def path_allowed(path: Path, root: Path) -> bool:
    try:
        relative = path.relative_to(root).as_posix()
    except ValueError:
        return False
    return not is_sensitive_relative_path(relative)


def utc_mtime(path: Path) -> str:
    from datetime import datetime, timezone

    return datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc).isoformat().replace(
        "+00:00", "Z"
    )
