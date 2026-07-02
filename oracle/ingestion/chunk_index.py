import json
import fnmatch
import os
import re
import subprocess
import sys
import time
from collections import OrderedDict
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from oracle.config import (
    CHUNK_BATCH_FILES,
    CHUNK_BATCH_CHUNKS,
    CHUNK_BATCH_CHARS,
    CHUNK_DB_PATH,
    CHUNK_GPU_COOLDOWN_MAX_CYCLES,
    CHUNK_GPU_COOLDOWN_SECONDS,
    CHUNK_GPU_RESUME_TEMP_C,
    CHUNK_LOW_MEMORY_RETRY_CYCLES,
    CHUNK_LOW_MEMORY_RETRY_SECONDS,
    CHUNK_CODE_MAX_CHARS,
    CHUNK_CODE_OVERLAP_CHARS,
    CHUNK_DOC_MAX_CHARS,
    CHUNK_DOC_OVERLAP_CHARS,
    CHUNK_MANIFEST_PATH,
    CHUNK_MAX_GPU_TEMP_C,
    CHUNK_MAX_CHARS,
    CHUNK_MAX_FILE_BYTES,
    CHUNK_MIN_FREE_GB,
    CHUNK_OVERLAP_CHARS,
    CHUNK_STRUCTURED_MAX_CHARS,
    CHUNK_STRUCTURED_OVERLAP_CHARS,
    EMBED_DIMS,
    LANCE_DB_PATH,
    SQLITE_PATH,
)
from oracle.ingestion.embedder import (
    effective_embed_batch_size,
    embed_texts,
    release_embedding_memory,
)
from oracle.ingestion.parser import is_sensitive_relative_path, utc_mtime
from oracle.ingestion.retrieval_text import (
    SEMANTIC_PREFIX_PROFILE_VERSION,
    active_chunk_profile_version,
    chunk_embedding_text,
)
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


EXCLUDED_DIRS = {
    ".cache",
    ".cxx",
    ".agents",
    ".claude",
    ".claude-mimo",
    ".codex",
    ".deepseek",
    ".expo",
    ".expo-export",
    ".expo-export-ios",
    ".expo-export-web",
    ".git",
    ".gradle",
    ".gradle-home",
    ".gradle-home-release",
    ".externalnativebuild",
    ".idea",
    ".mypy_cache",
    ".next",
    ".npm-cache",
    ".pytest_cache",
    ".rnaseq-reference-cache",
    ".ruff_cache",
    ".secrets",
    ".dev.vars",
    "aspis-secrets",
    ".tier1-work",
    ".venv",
    ".wrangler",
    "__pycache__",
    "_archive",
    "_baseline",
    "audit-downloads",
    "cellpose_data",
    "build",
    "codex-runs",
    "codex-sessions",
    "coverage",
    "dist",
    "legacy-graph-out",
    "graph" + "ify-out",
    "logs",
    "mockups",
    "node_modules",
    "oracle-data",
    "out",
    "outputs",
    "playwright-report",
    "target",
    "test-results",
    "tmp",
    "vendor",
    "venv",
}

EXCLUDED_RELATIVE_PREFIXES = {
    "aspis-biovision/aspis-secrets/",
    "aspis-biovision/data/",
    "aspis-biovision/reports/",
    "aspis-biovision/western blot/",
}

# Honored ignore files, in increasing precedence. `.gitignore` is read FIRST so
# the Oracle index sees the same file set as the Rust/Polis scanner (which honors
# `.gitignore` too); the Aspis-specific files override it on a tie. All three are
# parsed with gitignore semantics (root anchoring + `!` negation, last-match-wins).
WORKSPACE_IGNORE_FILES = (".gitignore", ".oracleignore", ".aspisignore")

TEXT_EXTENSIONS = {
    ".css",
    ".gradle",
    ".html",
    ".java",
    ".js",
    ".jsx",
    ".json",
    ".jsonc",
    ".kt",
    ".kts",
    ".md",
    ".mjs",
    ".cjs",
    ".mts",
    ".cts",
    ".properties",
    ".ps1",
    ".py",
    ".r",
    ".rmd",
    ".rs",
    ".sh",
    ".sql",
    ".toml",
    ".ts",
    ".tsx",
    ".xml",
    ".txt",
    ".yaml",
    ".yml",
}

CHUNK_PROFILE_VERSION = SEMANTIC_PREFIX_PROFILE_VERSION

DOC_EXTENSIONS = {
    ".md",
    ".txt",
}

STRUCTURED_EXTENSIONS = {
    ".gradle",
    ".html",
    ".json",
    ".jsonc",
    ".properties",
    ".toml",
    ".xml",
    ".yaml",
    ".yml",
}

CODE_EXTENSIONS = {
    ".css",
    ".java",
    ".js",
    ".jsx",
    ".kt",
    ".kts",
    ".mjs",
    ".cjs",
    ".mts",
    ".cts",
    ".ps1",
    ".py",
    ".r",
    ".rmd",
    ".rs",
    ".sh",
    ".sql",
    ".ts",
    ".tsx",
}

# Non-secret basenames that are noise and should not be indexed. The authoritative
# secret-exclusion decision lives in oracle.ingestion.parser.is_sensitive_relative_path
# (default-deny); this set only suppresses low-value, high-churn files.
NOISE_FILE_NAMES = {
    ".aspis-agents.json",
    ".npmrc",
    "package-lock.json",
}

# Path substrings that should never be indexed even though they are not strictly
# secrets handled by the shared filter (service accounts / private keys).
SENSITIVE_NAME_PARTS = {
    "service-account",
    "private-key",
}


def is_vendored_env_path(relative_path: str) -> bool:
    """Pure, string-only predicate: True if ``relative_path`` lies inside an
    installed-package / vendored Python environment tree, detected by an
    UNAMBIGUOUS path-component signature (no filesystem access).

    Recognized signatures (any path component, case-insensitive):
      - ``site-packages`` (the canonical install root), or
      - a ``*.dist-info`` directory component (PEP 376 / pip wheel install marker), or
      - a ``*.egg-info`` directory component (setuptools install/metadata marker).

    A workspace bundling a vendored env (e.g. ``aspis-biovision/Orasis/``) is
    DATA, not source: indexing its ~15k library ``.py`` balloons + pollutes the
    index. This stays conservative — a real ``*.dist-info``/``*.egg-info``
    DIRECTORY component is required, so a source module merely *named* like one
    (``dist_info_helpers/util.py``) is never dropped. Filesystem-marker
    detection of a sibling-marked package dir lives in
    ``directory_contains_install_marker`` and is applied by the walker.
    """
    text = relative_path.replace("\\", "/").lower()
    for component in text.split("/"):
        if not component:
            continue
        if component == "site-packages":
            return True
        if component.endswith(".dist-info") or component.endswith(".egg-info"):
            return True
    return False


def dir_is_install_root(dirnames: list[str], filenames: list[str]) -> bool:
    """No-IO predicate: True if a directory (described by its already-listed
    ``dirnames``/``filenames``) is the root of an installed-package / vendored env
    tree. Mirrors ``directory_contains_install_marker`` but consumes the
    ``dirnames``/``filenames`` ``os.walk`` ALREADY yields, so the walker needs no
    second ``os.scandir`` of the same directory.

    Markers (any one is sufficient), kept consistent with the Rust side:
      - a child ``*.dist-info`` or ``*.egg-info`` directory (pip/setuptools), or
      - a ``RECORD`` file alongside a ``WHEEL`` or ``METADATA`` file (an unpacked
        wheel install-marker dir).

    No density/file-count heuristic — a real ``*.dist-info``/``*.egg-info`` dir or
    the RECORD+WHEEL/METADATA pair is required, so a real source directory (which
    has none of these) is never pruned. Pure string inspection, never raises.
    """
    for dirname in dirnames:
        lower = dirname.lower()
        if lower.endswith(".dist-info") or lower.endswith(".egg-info"):
            return True
    has_record = "RECORD" in filenames
    if not has_record:
        return False
    return "WHEEL" in filenames or "METADATA" in filenames


def directory_contains_install_marker(directory: Path) -> bool:
    """On-disk check: True if ``directory`` is the root of an installed-package /
    vendored env tree, detected by a pip/setuptools install marker among its
    DIRECT children (one shallow scan, no recursion).

    Markers (any one is sufficient):
      - a child ``*.dist-info`` or ``*.egg-info`` directory (pip/setuptools), or
      - a ``RECORD`` file alongside a ``WHEEL`` or ``METADATA`` file (an unpacked
        wheel install-marker dir).

    This catches the case ``is_vendored_env_path`` cannot see from the path
    string alone: a library package dir (``Orasis/numpy/``) whose only proof of
    being vendored is a SIBLING ``numpy-1.26.4.dist-info`` directory. Pruning at
    the marker root drops the whole bundled tree. Conservative by design: a real
    source directory has none of these markers. Any OS error → False (fail open
    to indexing rather than wrongly hiding source); the secret default-deny in
    ``is_sensitive_relative_path`` still applies regardless.
    """
    try:
        has_record = False
        has_wheel_or_metadata = False
        with os.scandir(directory) as entries:
            for entry in entries:
                name = entry.name
                lower = name.lower()
                try:
                    is_dir = entry.is_dir()
                except OSError:
                    is_dir = False
                if is_dir and (lower.endswith(".dist-info") or lower.endswith(".egg-info")):
                    return True
                if not is_dir:
                    if name == "RECORD":
                        has_record = True
                    elif name in ("WHEEL", "METADATA"):
                        has_wheel_or_metadata = True
        return has_record and has_wheel_or_metadata
    except OSError:
        return False


def collect_text_files(root: Path | str = ".") -> list[Path]:
    root = Path(root).resolve()
    ignore_policy = load_workspace_ignore_policy(root)
    files = []
    for current, dirnames, filenames in os.walk(root, onerror=lambda _error: None):
        current_path = Path(current)
        # Installed-package / vendored env pruning. If THIS directory is the root
        # of an install tree (a sibling *.dist-info/*.egg-info or RECORD+WHEEL/
        # METADATA among its children), drop the whole subtree AND its own files:
        # a workspace's bundled libraries (e.g. aspis-biovision/Orasis/) are data,
        # not source. Detected from os.walk's already-listed dirnames/filenames via
        # dir_is_install_root (no second os.scandir of this directory). The cheap
        # string-signal pruning of child dirs (site-packages / *.dist-info /
        # *.egg-info components) is handled by directory_path_allowed.
        if current_path != root and dir_is_install_root(dirnames, filenames):
            dirnames[:] = []
            continue
        dirnames[:] = [
            dirname
            for dirname in dirnames
            if directory_path_allowed(current_path / dirname, root, ignore_policy)
        ]
        for filename in filenames:
            path = current_path / filename
            if path.suffix.lower() not in TEXT_EXTENSIONS:
                continue
            if not chunk_path_allowed(path, root, ignore_policy):
                continue
            try:
                if path.stat().st_size > CHUNK_MAX_FILE_BYTES:
                    continue
            except OSError:
                continue
            files.append(path)
    return sorted(files, key=lambda path: priority_key(path, root))


def directory_path_allowed(path: Path, root: Path, ignore_policy: dict | None = None) -> bool:
    try:
        relative = path.relative_to(root)
    except ValueError:
        return False
    # AUTHORITATIVE, non-overridable: prune sensitive directory trees (.secrets,
    # aspis-secrets, .dev.vars, ...) and installed-package / vendored env trees
    # (site-packages / *.dist-info / *.egg-info) FIRST. No gitignore negation can
    # re-open these. The on-disk sibling-marker vendored case is pruned by
    # collect_text_files via dir_is_install_root.
    if is_sensitive_relative_path(relative.as_posix().lower()):
        return False
    if is_vendored_env_path(relative.as_posix()):
        return False
    # gitignore re-include: keep walking a directory the user excluded (via the
    # built-in EXCLUDED_DIRS default OR a workspace-ignore rule) IF a `!`-negation
    # targets a path UNDER it, so the rescued file is reachable; the per-file
    # chunk_path_allowed then re-includes only the explicitly rescued children.
    rescue_under = negation_rescues_under(relative, ignore_policy)
    if path.name.lower() in EXCLUDED_DIRS and not rescue_under:
        return False
    if workspace_ignore_matches(relative, is_dir=True, ignore_policy=ignore_policy):
        return rescue_under
    return True


def path_explicitly_rescued(relative: Path, ignore_policy: dict | None = None) -> bool:
    """True if the user's ignore rules EXPLICITLY re-include this path: the last
    rule that matches is a ``!``-negation. Such a rescue overrides the built-in
    default excludes (EXCLUDED_DIRS / prefixes / noise) but never the security
    filters. False when no rule matches or the last match is an exclude.
    """
    if ignore_policy is None:
        return False
    rules = ignore_policy.get("rules", ())
    if not rules:
        return False
    relative_text = relative.as_posix().lower().rstrip("/")
    parts = [part.lower() for part in relative.parts]
    decision: bool | None = None
    for negated, anchored, dir_only, pattern in rules:
        if _ignore_rule_matches(
            negated,
            anchored,
            dir_only,
            pattern,
            relative_text=relative_text,
            parts=parts,
            is_dir=False,
        ):
            decision = negated
    return decision is True


def chunk_path_allowed(path: Path, root: Path, ignore_policy: dict | None = None) -> bool:
    try:
        relative = path.relative_to(root)
    except ValueError:
        return False
    relative_text = relative.as_posix().lower()
    # AUTHORITATIVE security invariant: the built-in default-deny secret filter
    # blocks the documented secret set (token.txt, secrets.yaml, aspis-secrets/...,
    # .env*, *.key/.pem/.pfx/.p12, credentials, vault, ...) regardless of any
    # user-droppable .oracleignore file. This must run first and unconditionally —
    # NOT even a gitignore `!`-negation can re-include a secret.
    if is_sensitive_relative_path(relative_text):
        return False
    # Installed-package / vendored env file (site-packages / *.dist-info /
    # *.egg-info component). Defense-in-depth alongside the walker's subtree prune.
    # Also non-negotiable: bundled library code is data, not source.
    if is_vendored_env_path(relative_text):
        return False
    # gitignore last-match-wins evaluation of the user's ignore files. A `!`-negation
    # whose LAST matching rule is the negation explicitly re-includes the file; that
    # rescue overrides the built-in DEFAULT excludes below (EXCLUDED_DIRS,
    # EXCLUDED_RELATIVE_PREFIXES, NOISE_FILE_NAMES, *.min.*) — but never the
    # security filters above. With no ignore policy, `rescued` is False and the
    # defaults apply unchanged.
    rescued = path_explicitly_rescued(relative, ignore_policy)
    if not rescued:
        lower_parts = [part.lower() for part in relative.parts]
        if any(part in EXCLUDED_DIRS for part in lower_parts):
            return False
    if workspace_ignore_matches(relative, is_dir=False, ignore_policy=ignore_policy):
        return False
    if rescued:
        return True
    name = path.name.lower()
    if name in NOISE_FILE_NAMES:
        return False
    if name.endswith((".min.js", ".min.css")):
        return False
    if any(relative_text.startswith(prefix) for prefix in EXCLUDED_RELATIVE_PREFIXES):
        return False
    return not any(part in relative_text for part in SENSITIVE_NAME_PARTS)


def load_workspace_ignore_policy(root: Path) -> dict:
    """Parse the honored ignore files into an ORDERED rule list with gitignore
    semantics. Each rule is ``(negated, anchored, dir_only, pattern)``:

      - ``negated``  — a leading ``!`` un-excludes a path an earlier rule ignored
        (gitignore last-match-wins). Without negation the old code silently
        dropped ``!pattern`` lines, permanently excluding a user's rescued path.
      - ``anchored`` — a leading ``/`` (or a pattern containing a non-trailing
        ``/``) is root-relative: it matches only at the ignore file's dir root,
        never as a nested name match. The old ``lstrip("/")`` destroyed this.
      - ``dir_only`` — a trailing ``/`` matches directories (and their subtrees)
        only.

    Rules are kept in file order across WORKSPACE_IGNORE_FILES (``.gitignore``
    first, then the Aspis files) so a later file/line wins on a tie, matching the
    Rust scanner's precedence. Patterns are stored lower-cased (the matcher is
    case-insensitive, like the rest of this module).
    """
    rules: list[tuple[bool, bool, bool, str]] = []
    for ignore_name in WORKSPACE_IGNORE_FILES:
        ignore_path = root / ignore_name
        if not ignore_path.is_file():
            continue
        try:
            lines = ignore_path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for raw_line in lines:
            line = raw_line.strip().replace("\\", "/")
            if not line or line.startswith("#"):
                continue
            negated = line.startswith("!")
            if negated:
                line = line[1:]
            if not line:
                continue
            dir_only = line.endswith("/")
            if dir_only:
                line = line.rstrip("/")
            if not line:
                continue
            # Anchored if it starts at root (leading slash) OR contains an interior
            # slash (gitignore: a pattern with a non-trailing `/` is root-relative).
            anchored = line.startswith("/") or "/" in line
            line = line.lstrip("/")
            if not line:
                continue
            rules.append((negated, anchored, dir_only, line.lower()))
    return {"rules": rules}


def _ignore_rule_matches(
    negated: bool,
    anchored: bool,
    dir_only: bool,
    pattern: str,
    *,
    relative_text: str,
    parts: list[str],
    is_dir: bool,
) -> bool:
    """True if a single parsed ignore rule matches the (lower-cased) path.

    ``dir_only`` rules match a directory itself or any path UNDER it. Anchored
    rules match from the root; unanchored single-segment rules match the path by
    any of its path components (gitignore: a pattern with no slash matches at any
    depth).
    """
    if dir_only:
        # `name/` excludes the directory and everything under it.
        if anchored:
            return relative_text == pattern or relative_text.startswith(pattern + "/")
        # Unanchored dir rule: match if any path component equals the pattern OR
        # the pattern (which may itself contain a glob/slash) matches a path prefix.
        if any(fnmatch.fnmatchcase(part, pattern) for part in parts):
            return True
        return _path_prefix_glob_match(relative_text, pattern)

    if anchored:
        return fnmatch.fnmatchcase(relative_text, pattern) or relative_text.startswith(pattern + "/")

    # Unanchored file/glob rule: match any single path component (e.g. `*.secret.txt`
    # against the basename, or a bare `name` against any segment), OR the full path.
    if any(fnmatch.fnmatchcase(part, pattern) for part in parts):
        return True
    return fnmatch.fnmatchcase(relative_text, pattern)


def _path_prefix_glob_match(relative_text: str, pattern: str) -> bool:
    """True if ``pattern`` (possibly with globs/slashes) matches a leading path
    segment-prefix of ``relative_text`` — i.e. the dir the pattern names, or any
    path under it. Used for unanchored ``dir/`` rules that carry a slash/glob.
    """
    parts = relative_text.split("/")
    for end in range(1, len(parts) + 1):
        prefix = "/".join(parts[:end])
        if fnmatch.fnmatchcase(prefix, pattern):
            return True
    return False


def workspace_ignore_matches(
    relative: Path,
    *,
    is_dir: bool,
    ignore_policy: dict | None = None,
) -> bool:
    """gitignore last-match-wins: walk the ordered rules; the LAST rule that
    matches decides. A negated (``!``) rule un-excludes; a normal rule excludes.
    Returns True only if the final deciding rule is a (non-negated) exclude.
    """
    if ignore_policy is None:
        ignore_policy = {}
    rules = ignore_policy.get("rules", ())
    if not rules:
        return False
    relative_text = relative.as_posix().lower()
    relative_text = relative_text.rstrip("/")
    parts = [part.lower() for part in relative.parts]
    ignored = False
    for negated, anchored, dir_only, pattern in rules:
        if _ignore_rule_matches(
            negated,
            anchored,
            dir_only,
            pattern,
            relative_text=relative_text,
            parts=parts,
            is_dir=is_dir,
        ):
            ignored = not negated
    return ignored


def negation_rescues_under(relative: Path, ignore_policy: dict | None = None) -> bool:
    """True if some negation (``!``) rule targets a path strictly UNDER directory
    ``relative`` — so the walker must NOT prune this directory even though an
    exclude rule matched it, or a gitignore re-include (``build/`` + ``!build/keep.md``)
    would be unreachable (the file is never visited). Conservative: any anchored
    negation pattern whose path lies under this dir, or any unanchored negation,
    keeps the directory walkable; the per-file check then decides each child.
    """
    if ignore_policy is None:
        return False
    rules = ignore_policy.get("rules", ())
    if not rules:
        return False
    dir_text = relative.as_posix().lower().rstrip("/")
    prefix = dir_text + "/" if dir_text else ""
    for negated, anchored, _dir_only, pattern in rules:
        if not negated:
            continue
        if not anchored:
            # An unanchored negation (e.g. `!keep.md`) can match at any depth.
            return True
        # Anchored negation: does its target lie under this directory?
        if not prefix or pattern.startswith(prefix) or pattern == dir_text:
            return True
    return False


def priority_key(path: Path, root: Path) -> tuple[int, str]:
    relative = path.relative_to(root).as_posix().lower()
    return (priority_rank(relative), relative)


def priority_rank(relative: str) -> int:
    relative = relative.lower()
    if (
        relative.startswith("aspis-lab/cloudflare/")
        or relative.startswith("aspis-lab/compute/")
        or relative.startswith("aspis-biovision/src/")
        or relative.startswith("aspis-biovision/scripts/")
        or relative.startswith("aspis-biovision/deploy/")
        or "/workers/" in relative
        or "scaleway" in relative
        or "cloudflare" in relative
        or "biovision" in relative and "worker" in relative
    ):
        return 0
    if (
        relative.startswith("aspis-lab/src/")
        or relative.startswith("aspis-lab/android/")
        or relative.startswith("aspis-biovision/orasis/")
        or relative.startswith("aspis-lab/tests/")
        or relative.startswith("aspis-biovision/tests/")
    ):
        return 1
    if relative.endswith((".md", ".txt")) or "/docs/" in relative or relative.startswith("docs/"):
        return 2
    return 3


def read_text_file(path: Path) -> str | None:
    raw = path.read_bytes()
    if b"\x00" in raw:
        return None
    return raw.decode("utf-8", errors="replace")


def split_text(text: str, max_chars: int = CHUNK_MAX_CHARS, overlap: int = CHUNK_OVERLAP_CHARS) -> list[tuple[int, int, str]]:
    clean = text.replace("\r\n", "\n")
    if not clean.strip():
        return []
    chunks = []
    start = 0
    length = len(clean)
    step = max(1, max_chars - overlap)
    while start < length:
        end = min(length, start + max_chars)
        if end < length:
            newline = clean.rfind("\n", start + max_chars // 2, end)
            if newline > start:
                end = newline + 1
        piece = clean[start:end].strip()
        if piece:
            chunks.append((start, end, piece))
        if end >= length:
            break
        start = max(0, end - overlap)
        if start >= length:
            break
        if start < end - step:
            start = end - overlap
    return chunks


def chunk_limits_for_file(path: Path) -> tuple[int, int]:
    suffix = path.suffix.lower()
    lower_parts = [part.lower() for part in path.parts]
    if suffix in DOC_EXTENSIONS or "docs" in lower_parts:
        return CHUNK_DOC_MAX_CHARS, CHUNK_DOC_OVERLAP_CHARS
    if suffix in STRUCTURED_EXTENSIONS:
        return CHUNK_STRUCTURED_MAX_CHARS, CHUNK_STRUCTURED_OVERLAP_CHARS
    if suffix in CODE_EXTENSIONS:
        return CHUNK_CODE_MAX_CHARS, CHUNK_CODE_OVERLAP_CHARS
    return CHUNK_MAX_CHARS, CHUNK_OVERLAP_CHARS


def build_chunks_for_file(path: Path, root: Path) -> list[dict]:
    text = read_text_file(path)
    if text is None:
        return []
    file_id = path.relative_to(root).as_posix()
    mtime = utc_mtime(path)
    max_chars, overlap = chunk_limits_for_file(path)
    chunks = []
    for index, (start, end, piece) in enumerate(split_text(text, max_chars=max_chars, overlap=overlap)):
        chunks.append(
            {
                "id": f"{file_id}#chunk-{index:04d}",
                "file_id": file_id,
                "label": f"{path.name} chunk {index + 1}",
                "area": "FileChunk",
                "cluster_semantic": path.suffix.lower().lstrip(".") or "text",
                "chunk_index": index,
                "start_char": start,
                "end_char": end,
                "text": piece,
                "file_sorgente": file_id,
                "ultima_modifica": mtime,
                "embedding_dims": EMBED_DIMS,
            }
        )
    return chunks


def chunk_batches(chunks: list[dict], max_chunks: int, max_chars: int):
    batch = []
    batch_chars = 0
    max_chunks = max(1, max_chunks)
    max_chars = max(1, max_chars)
    for chunk in chunks:
        text_chars = len(chunk_embedding_text(chunk))
        if batch and (len(batch) >= max_chunks or batch_chars + text_chars > max_chars):
            yield batch
            batch = []
            batch_chars = 0
        batch.append(chunk)
        batch_chars += text_chars
    if batch:
        yield batch


def wait_for_gpu_cooldown(max_gpu_temp_c: int, progress: bool = False) -> int | None:
    cooldown_seconds = max(0, CHUNK_GPU_COOLDOWN_SECONDS)
    max_cycles = max(1, CHUNK_GPU_COOLDOWN_MAX_CYCLES)
    resume_temp_c = min(CHUNK_GPU_RESUME_TEMP_C, max_gpu_temp_c - 1)
    if cooldown_seconds <= 0:
        return gpu_temperature_c()

    temp_c = gpu_temperature_c()
    for cycle in range(max_cycles):
        if temp_c is None or temp_c <= resume_temp_c:
            return temp_c
        # Free the CUDA cache (NOT the model — keep it resident so resume is
        # fast) and sleep, then re-read the temperature.
        release_embedding_memory()
        log_progress(
            progress,
            f"chunk-index gpu cooldown temp_c={temp_c} resume_temp_c={resume_temp_c} "
            f"sleep_seconds={cooldown_seconds} cycle={cycle + 1}/{max_cycles}",
        )
        time.sleep(cooldown_seconds)
        temp_c = gpu_temperature_c()
    return temp_c


def effective_chunk_batch_size(batch_chunks: int | None) -> int:
    """Chunks handed to ONE embed_texts() call.

    Tri-state, so an explicit operator value is never second-guessed (an
    operator pinning ``--batch-chunks 8`` must get literally 8 even though 8
    is also the config default):
      * ``batch_chunks`` is an int  -> explicit caller/CLI choice, used as-is;
      * env ORACLE_CHUNK_BATCH_CHUNKS -> operator override via config;
      * ``None`` (nothing chosen)   -> derive from the hardware-sized encode
        batch (embedder.choose_embed_batch_size: up to 64 on CUDA / 32 on
        MPS): 4 encode-batches per call amortizes per-call overhead. The old
        flat default (8) couldn't even fill one encode batch.

    Memory bound honesty: chunk_char_budget caps the AGGREGATE chars of one
    embed_texts() call, NOT the peak forward pass — sentence-transformers
    slices the call into groups of the encode batch size, so the peak pass is
    bounded by encode_batch x largest-chunk-chars (32 x ~2500 chars ≈ 20k
    tokens of Qwen3-0.6B activations on MPS — small next to the tiers'
    free-RAM floors, and the OOM->CPU fallback + between-batch RAM/thermal
    guards in the index loop still own scaling DOWN under pressure).
    """
    if batch_chunks is not None:
        return max(1, batch_chunks)
    if os.getenv("ORACLE_CHUNK_BATCH_CHUNKS", "").strip():
        return max(1, CHUNK_BATCH_CHUNKS)
    return max(CHUNK_BATCH_CHUNKS, 4 * effective_embed_batch_size())


def adaptive_batch_files(
    base: int, current: int, free_gb: float, min_free_gb: float
) -> int:
    """Owner request (2026-06-12): scale the per-iteration FILE batch with the
    same free-RAM reading the pause floor uses — grow while memory is
    plentiful, shrink BEFORE the floor pauses us when it tightens.

      - floor disabled (min_free_gb <= 0): no signal -> hold current;
      - free >= 4x floor: double, capped at 4x base;
      - free <  2x floor: halve, floored at max(2, base // 4);
      - otherwise: hold.

    Pure and stateless over its inputs so the policy is unit-testable; the
    caller threads `current` between iterations.
    """
    if min_free_gb <= 0:
        return max(1, current)
    lo = max(2, base // 4)
    hi = max(base, base * 4)
    if free_gb >= 4 * min_free_gb:
        return min(hi, max(1, current) * 2)
    if free_gb < 2 * min_free_gb:
        return max(lo, max(1, current) // 2)
    return max(1, current)


def wait_for_memory_recovery(min_free_gb: float, progress: bool = False) -> float:
    """Sleep-and-retry while free system RAM is below ``min_free_gb``.

    Low free RAM is treated as TRANSIENT (another process briefly spiked): we
    sleep a few short cycles and re-check, returning as soon as RAM recovers.
    Returns the final observed free-RAM reading. The caller decides whether the
    final value is still below the floor (genuine give-up) or has recovered
    (continue). The model is intentionally NOT unloaded here so a recovered
    run resumes without reloading; only ``empty_cache`` is invoked.
    """
    retry_seconds = max(0, CHUNK_LOW_MEMORY_RETRY_SECONDS)
    max_cycles = max(1, CHUNK_LOW_MEMORY_RETRY_CYCLES)
    free_gb = free_memory_gb()
    if retry_seconds <= 0:
        return free_gb
    for cycle in range(max_cycles):
        if free_gb >= min_free_gb:
            return free_gb
        release_embedding_memory()
        log_progress(
            progress,
            f"chunk-index low-memory retry free_gb={free_gb} min_free_gb={min_free_gb} "
            f"sleep_seconds={retry_seconds} cycle={cycle + 1}/{max_cycles}",
        )
        time.sleep(retry_seconds)
        free_gb = free_memory_gb()
    return free_gb


def index_file_chunks(
    root: Path | str = ".",
    sqlite_path: Path | str = SQLITE_PATH,
    chunk_vector_path: Path | str = CHUNK_DB_PATH,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
    batch_files: int = CHUNK_BATCH_FILES,
    batch_chunks: int | None = None,
    batch_chars: int = CHUNK_BATCH_CHARS,
    min_free_gb: float = CHUNK_MIN_FREE_GB,
    max_gpu_temp_c: int | None = CHUNK_MAX_GPU_TEMP_C,
    max_batches: int | None = None,
    force: bool = False,
    use_sentence_transformer: bool = True,
    require_sentence_transformer: bool = False,
    progress: bool = False,
    on_phase: Callable[[str, dict], None] | None = None,
) -> dict:
    # Live sub-state callback. While the job status stays "running", the loop
    # reports a transient phase ("cooling_gpu"/"waiting_memory") when it pauses on
    # GPU heat / low RAM and "running" again on resume, so the UI can show
    # "working, not stuck". Never raises out of the loop: a faulty callback must
    # not abort an index run. Payloads carry only numbers (temps/free GB), never
    # paths, so the surfaced phase can never leak a filesystem path.
    def emit_phase(phase: str, detail: dict) -> None:
        if on_phase is None:
            return
        try:
            on_phase(phase, detail)
        except Exception:
            # A UI/status sink failure must never abort indexing.
            pass

    root = Path(root).resolve()
    manifest_path = Path(manifest_path)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest = load_manifest(manifest_path)
    manifest_files = manifest_files_for_root(manifest, root, create=True)

    if min_free_gb > 0 and free_memory_gb() < min_free_gb:
        log_progress(progress, f"chunk-index paused_low_memory before scan root={root}")
        return status_payload("paused_low_memory", root, sqlite_path, chunk_vector_path, manifest_path, scanned=0, processed=0)

    output_paths = {
        Path(sqlite_path).resolve(),
        Path(chunk_vector_path).resolve(),
        manifest_path.resolve(),
    }
    files = [
        path
        for path in collect_text_files(root)
        if path.resolve() not in output_paths
    ]
    sqlite = SQLiteStore(sqlite_path)
    vector_store = LanceStore(chunk_vector_path)
    pending = [
        path
        for path in files
        if force or file_needs_index(path, root, manifest_files, sqlite)
    ]
    log_progress(
        progress,
        f"chunk-index start root={root} scanned={len(files)} pending={len(pending)} "
        f"indexed={len(manifest_files)} min_free_ram_gb={min_free_gb}",
    )

    processed_files = 0
    processed_chunks = 0
    files_done_this_run = 0
    base_file_batch_size = max(1, batch_files)
    file_batch_size = base_file_batch_size
    # max_batches semantics stay anchored to the BASE size (a "batch" budget is
    # sized in base units even while the adaptive controller grows/shrinks the
    # actual per-iteration slice).
    max_files_per_run = max_batches * base_file_batch_size if max_batches is not None else None
    chunk_batch_size = effective_chunk_batch_size(batch_chunks)
    chunk_char_budget = max(1, batch_chars)

    pending_index = 0
    while pending_index < len(pending):
        if max_files_per_run is not None and files_done_this_run >= max_files_per_run:
            break
        free_gb = free_memory_gb()
        # Adaptive sizing (owner request): same reading the pause floor uses —
        # grow the slice while RAM is plentiful, shrink it before the floor
        # would pause us.
        file_batch_size = adaptive_batch_files(
            base_file_batch_size, file_batch_size, free_gb, min_free_gb
        )
        remaining_files = (
            file_batch_size
            if max_files_per_run is None
            else min(file_batch_size, max_files_per_run - files_done_this_run)
        )
        batch_paths = pending[pending_index : pending_index + remaining_files]
        if not batch_paths:
            break
        if min_free_gb > 0 and free_gb < min_free_gb:
            # WAIT-AND-RESUME (see the in-batch guard below): treat low RAM as
            # transient — sleep-and-retry, resume if it recovers, give up only on
            # a persistent shortfall.
            save_manifest(manifest_path, manifest)
            emit_phase(
                "waiting_memory",
                {"free_gb": free_gb, "min_free_gb": min_free_gb},
            )
            free_gb = wait_for_memory_recovery(min_free_gb, progress)
            if free_gb < min_free_gb:
                log_progress(progress, f"chunk-index paused_low_memory free_gb={free_gb}")
                return status_payload(
                    "paused_low_memory",
                    root,
                    sqlite_path,
                    chunk_vector_path,
                    manifest_path,
                    scanned=len(files),
                    pending=len(pending) - processed_files,
                    processed=processed_files,
                    chunks=processed_chunks,
                    free_gb=free_gb,
                )
            emit_phase("running", {})

        batch_file_ids = [path.relative_to(root).as_posix() for path in batch_paths]
        batch_chunks_to_index = []
        file_chunks_by_id = {}
        for path, file_id in zip(batch_paths, batch_file_ids):
            file_chunks = build_chunks_for_file(path, root)
            file_chunks_by_id[file_id] = file_chunks
            batch_chunks_to_index.extend(file_chunks)
        old_ids = sqlite.chunk_ids_for_files(batch_file_ids)
        log_progress(
            progress,
            f"chunk-index batch begin files={len(batch_paths)} chunks={len(batch_chunks_to_index)} "
            f"remaining_before={len(pending) - processed_files} free_gb={free_gb}",
        )

        vector_records = []
        for chunk_batch in chunk_batches(batch_chunks_to_index, chunk_batch_size, chunk_char_budget):
            gpu_temp = gpu_temperature_c()
            if max_gpu_temp_c and gpu_temp is not None and gpu_temp >= max_gpu_temp_c:
                # COOL-AND-RESUME: a thermal event must not abort the whole run.
                # Persist progress (durable across a crash during the sleep),
                # then sleep until the GPU cools to the resume threshold. Keep the
                # model resident (wait_for_gpu_cooldown only empties the CUDA
                # cache) so resume is fast. Only if the cooldown EXHAUSTS its
                # cycles and the GPU is still at/above the ceiling do we give up.
                save_manifest(manifest_path, manifest)
                emit_phase(
                    "cooling_gpu",
                    {"gpu_temp_c": gpu_temp, "max_gpu_temp_c": max_gpu_temp_c},
                )
                cooled_temp = wait_for_gpu_cooldown(max_gpu_temp_c, progress)
                if cooled_temp is not None and cooled_temp >= max_gpu_temp_c:
                    release_embedding_memory(unload_model=True)
                    log_progress(
                        progress,
                        f"chunk-index paused_gpu_temperature temp_c={cooled_temp} max_gpu_temp_c={max_gpu_temp_c}",
                    )
                    return status_payload(
                        "paused_gpu_temperature",
                        root,
                        sqlite_path,
                        chunk_vector_path,
                        manifest_path,
                        scanned=len(files),
                        pending=len(pending) - processed_files,
                        processed=processed_files,
                        chunks=processed_chunks,
                        gpu_temp_c=cooled_temp,
                        max_gpu_temp_c=max_gpu_temp_c,
                        free_gb=free_memory_gb(),
                    )
                log_progress(
                    progress,
                    f"chunk-index gpu cooled resume temp_c={cooled_temp} max_gpu_temp_c={max_gpu_temp_c}",
                )
                emit_phase("running", {})
            free_gb = free_memory_gb()
            if min_free_gb > 0 and free_gb < min_free_gb:
                # WAIT-AND-RESUME: low free RAM is usually a transient spike from
                # another process. Sleep-and-retry a few short cycles; resume if
                # RAM recovers. Only a persistent shortfall after the cycles is a
                # genuine give-up.
                save_manifest(manifest_path, manifest)
                emit_phase(
                    "waiting_memory",
                    {"free_gb": free_gb, "min_free_gb": min_free_gb},
                )
                free_gb = wait_for_memory_recovery(min_free_gb, progress)
                if free_gb < min_free_gb:
                    release_embedding_memory(unload_model=True)
                    log_progress(
                        progress,
                        f"chunk-index paused_low_memory before_embed batch_files={len(batch_paths)} free_gb={free_gb}",
                    )
                    return status_payload(
                        "paused_low_memory",
                        root,
                        sqlite_path,
                        chunk_vector_path,
                        manifest_path,
                        scanned=len(files),
                        pending=len(pending) - processed_files,
                        processed=processed_files,
                        chunks=processed_chunks,
                        free_gb=free_gb,
                    )
                emit_phase("running", {})
            texts = [chunk_embedding_text(chunk) for chunk in chunk_batch]
            log_progress(
                progress,
                f"chunk-index embed files={len(batch_paths)} chunks={len(texts)} chars={sum(len(text) for text in texts)}",
            )
            vectors = embed_texts(
                texts,
                use_sentence_transformer=use_sentence_transformer,
                require_sentence_transformer=require_sentence_transformer,
            )
            vector_records.extend(
                {**chunk, "vector": vector}
                for chunk, vector in zip(chunk_batch, vectors)
            )

        committed_file_count = len(batch_paths)
        committed_chunk_count = len(batch_chunks_to_index)
        vector_store.replace_ids(old_ids, vector_records)
        sqlite.replace_chunks_for_files(batch_file_ids, batch_chunks_to_index)
        for path, file_id in zip(batch_paths, batch_file_ids):
            manifest_files[file_id] = file_signature(path, chunks=len(file_chunks_by_id[file_id]))
        sync_legacy_manifest_root(manifest, root)
        save_manifest(manifest_path, manifest)
        processed_files += committed_file_count
        processed_chunks += committed_chunk_count
        files_done_this_run += committed_file_count
        pending_index += committed_file_count
        del vector_records
        del batch_chunks_to_index
        del file_chunks_by_id
        release_embedding_memory(unload_model=min_free_gb > 0 and free_memory_gb() < min_free_gb)
        log_progress(
            progress,
            f"chunk-index batch committed processed_files={processed_files} "
            f"processed_chunks={processed_chunks} indexed_files={len(manifest_files)}",
        )

    status = "complete" if processed_files == len(pending) else "paused_batch_limit"
    sync_legacy_manifest_root(manifest, root)
    save_manifest(manifest_path, manifest)
    release_embedding_memory(unload_model=True)
    log_progress(progress, f"chunk-index {status} processed_files={processed_files} processed_chunks={processed_chunks}")
    return status_payload(
        status,
        root,
        sqlite_path,
        chunk_vector_path,
        manifest_path,
        scanned=len(files),
        pending=max(0, len(pending) - processed_files),
        processed=processed_files,
        chunks=processed_chunks,
        total_files=sqlite.chunk_file_count(),
        total_chunks=sqlite.chunk_count(),
        vector_records=vector_store.count(),
        free_gb=free_memory_gb(),
    )


def text_chunks_up_to_date(
    path: Path, root: Path, manifest_files: dict, sqlite: SQLiteStore
) -> bool:
    """P4: True when a file's TEXT chunks are already current and need no re-chunk.

    Mirrors the negation of `file_needs_index` for the text-sync layer: the file
    must have a manifest entry whose size + mtime + chunk_profile all match the
    file's current signature, AND (defensively) sqlite must already hold its
    chunks — so a manifest/store divergence still re-chunks rather than silently
    skipping a file with no chunks. A manifest entry recording zero chunks (an
    intentionally empty file) is treated as up-to-date without touching sqlite.
    """
    file_id = path.relative_to(root).as_posix()
    previous = manifest_files.get(file_id)
    if not previous:
        return False
    current = file_signature(path)
    if previous.get("size") != current["size"] or previous.get("mtime_ns") != current["mtime_ns"]:
        return False
    if previous.get("chunk_profile") != active_chunk_profile_version():
        return False
    if previous.get("chunks") == 0:
        return True
    return len(sqlite.chunks_for_file(file_id)) > 0


def sync_text_chunks(
    root: Path | str = ".",
    sqlite_path: Path | str = SQLITE_PATH,
    batch_files: int = 100,
    progress: bool = False,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
    force: bool = False,
) -> dict:
    root = Path(root).resolve()
    sqlite = SQLiteStore(sqlite_path)
    files = collect_text_files(root)
    # P4: previously this re-chunked + rewrote EVERY file's text chunks on every
    # `run_once`, even on a completed, unchanged index — burning CPU and churning
    # sqlite (which, before WAL, also starved the reader endpoints). Make it
    # incremental: load the manifest signatures and skip files already chunked at
    # the current chunk_profile with unchanged size + mtime (mirroring
    # `file_needs_index`). `force=True` restores the full unconditional rewrite.
    manifest = load_manifest(Path(manifest_path))
    manifest_files = manifest_files_for_root(manifest, root, create=False)
    pending = (
        files
        if force
        else [
            path
            for path in files
            if not text_chunks_up_to_date(path, root, manifest_files, sqlite)
        ]
    )
    skipped_files = len(files) - len(pending)
    processed_files = 0
    processed_chunks = 0
    for start in range(0, len(pending), max(1, batch_files)):
        batch = pending[start : start + max(1, batch_files)]
        chunks = []
        file_ids = []
        for path in batch:
            file_id = path.relative_to(root).as_posix()
            file_chunks = build_chunks_for_file(path, root)
            file_ids.append(file_id)
            chunks.extend(file_chunks)
        sqlite.replace_chunks_for_files(file_ids, chunks)
        processed_files += len(batch)
        processed_chunks += len(chunks)
        log_progress(
            progress,
            f"chunk-text-sync committed files={processed_files}/{len(pending)} "
            f"chunks={processed_chunks} skipped={skipped_files}",
        )
    return {
        "status": "complete",
        "root": str(root),
        "files": processed_files,
        "skipped": skipped_files,
        "chunks": processed_chunks,
        "sqlite_path": str(sqlite_path),
    }


def prune_excluded_chunks(
    root: Path | str = ".",
    sqlite_path: Path | str = SQLITE_PATH,
    chunk_vector_path: Path | str = CHUNK_DB_PATH,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
    node_vector_path: Path | str = LANCE_DB_PATH,
    progress: bool = False,
) -> dict:
    root = Path(root).resolve()
    sqlite = SQLiteStore(sqlite_path)
    vector_store = LanceStore(chunk_vector_path)
    node_vector_store = LanceStore(node_vector_path)
    manifest_path = Path(manifest_path)
    manifest = load_manifest(manifest_path)
    expected = {path.relative_to(root).as_posix() for path in collect_text_files(root)}
    expected_all_roots = set(expected)
    for root_key in manifest_roots(manifest):
        other_root = Path(root_key)
        if other_root == root or not other_root.is_dir():
            continue
        expected_all_roots.update(
            path.relative_to(other_root).as_posix() for path in collect_text_files(other_root)
        )
    existing = {chunk["file_id"] for chunk in sqlite.all_chunks()}
    removed_files = sorted(existing - expected_all_roots)
    removed_ids = sqlite.chunk_ids_for_files(removed_files)
    if removed_files:
        sqlite.replace_chunks_for_files(removed_files, [])
    valid_chunk_ids = {chunk["id"] for chunk in sqlite.all_chunks()}
    orphan_ids = sorted(set(vector_store.ids()) - valid_chunk_ids)
    removed_vector_ids = sorted(set(removed_ids) | set(orphan_ids))
    if removed_vector_ids:
        vector_store.replace_ids(removed_vector_ids, [])

    existing_nodes = {node["id"]: node for node in sqlite.all_nodes()}
    removed_node_ids = sorted(
        node_id
        for node_id, node in existing_nodes.items()
        if node.get("file_sorgente") not in expected_all_roots
    )
    if removed_node_ids:
        sqlite.delete_nodes(removed_node_ids)
    valid_node_ids = {node["id"] for node in sqlite.all_nodes()}
    orphan_node_vector_ids = sorted(set(node_vector_store.ids()) - valid_node_ids)
    removed_node_vector_ids = sorted(set(removed_node_ids) | set(orphan_node_vector_ids))
    if removed_node_vector_ids:
        node_vector_store.replace_ids(removed_node_vector_ids, [])

    manifest_files = manifest_files_for_root(manifest, root, create=False)
    manifest_removed = 0
    if manifest_files:
        for file_id in list(manifest_files):
            if file_id not in expected:
                del manifest_files[file_id]
                manifest_removed += 1
        sync_legacy_manifest_root(manifest, root)
        save_manifest(manifest_path, manifest)

    log_progress(
        progress,
        f"chunk-prune removed_files={len(removed_files)} removed_vectors={len(removed_vector_ids)} "
        f"orphan_vectors={len(orphan_ids)} removed_nodes={len(removed_node_ids)} "
        f"removed_node_vectors={len(removed_node_vector_ids)} manifest_removed={manifest_removed}",
    )
    return {
        "status": "complete",
        "root": str(root),
        "removed_files": len(removed_files),
        "removed_vectors": len(removed_vector_ids),
        "removed_orphan_vectors": len(orphan_ids),
        "removed_nodes": len(removed_node_ids),
        "removed_node_vectors": len(removed_node_vector_ids),
        "removed_orphan_node_vectors": len(orphan_node_vector_ids),
        "manifest_removed": manifest_removed,
        "sqlite_chunk_files": sqlite.chunk_file_count(),
        "sqlite_chunks": sqlite.chunk_count(),
        "vector_records": vector_store.count(),
        "sqlite_nodes": sqlite.count(),
        "node_vector_records": node_vector_store.count(),
    }


def file_needs_index(path: Path, root: Path, manifest_files: dict, sqlite: SQLiteStore) -> bool:
    file_id = path.relative_to(root).as_posix()
    current = file_signature(path)
    previous = manifest_files.get(file_id)
    if not previous:
        return True
    if previous.get("size") != current["size"] or previous.get("mtime_ns") != current["mtime_ns"]:
        return True
    if previous.get("chunk_profile") != active_chunk_profile_version():
        return True
    if previous.get("chunks") == 0:
        return False
    return len(sqlite.chunks_for_file(file_id)) == 0


def chunk_index_status(
    root: Path | str = ".",
    sqlite_path: Path | str = SQLITE_PATH,
    chunk_vector_path: Path | str = CHUNK_DB_PATH,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
) -> dict:
    root = Path(root).resolve()
    manifest_path = Path(manifest_path)
    manifest = load_manifest(manifest_path)
    manifest_files = manifest_files_for_root(manifest, root, create=False)
    sqlite = SQLiteStore(sqlite_path)
    vector_store = LanceStore(chunk_vector_path)
    output_paths = {
        Path(sqlite_path).resolve(),
        Path(chunk_vector_path).resolve(),
        manifest_path.resolve(),
    }
    files = [
        path
        for path in collect_text_files(root)
        if path.resolve() not in output_paths
    ]
    expected = {path.relative_to(root).as_posix() for path in files}
    indexed = set(manifest_files)
    pending = sorted(expected - indexed, key=lambda item: (priority_rank(item), item))
    stale = []
    for path in files:
        file_id = path.relative_to(root).as_posix()
        if file_id in indexed and file_needs_index(path, root, manifest_files, sqlite):
            stale.append(file_id)
    return {
        "root": str(root),
        "manifest_path": str(manifest_path),
        "expected_files": len(expected),
        "indexed_files": len(indexed & expected),
        "pending_files": len(pending),
        "stale_files": len(stale),
        "sqlite_chunk_files": sqlite.chunk_file_count(),
        "sqlite_chunks": sqlite.chunk_count(),
        "vector_records": vector_store.count(),
        "chunk_profile": active_chunk_profile_version(),
        "first_pending": pending[:12],
        "first_stale": stale[:12],
        "free_gb": free_memory_gb(),
        "free_ram_gb": free_memory_gb(),
    }


MAX_INDEXED_FILES_LIMIT = 500

# Small bounded parse cache for the /index/files READ path only. A search-as-you-
# type UI fires many manifest_indexed_files() calls against the SAME manifest
# version; without this each one re-parses the full JSON from disk. Keyed on
# (resolved manifest path, mtime_ns, size) so any write (which bumps mtime) is
# detected and the entry invalidated automatically. Bounded to the last few
# manifests to cap memory; LRU-evicted by insertion order. This is intentionally
# local to this read path and does NOT touch load_manifest's contract or the
# write path. Thread note: dict ops here are atomic under CPython's GIL and a
# benign cache miss/recompute is harmless, so no extra lock is taken.
_MANIFEST_PARSE_CACHE: "OrderedDict[tuple[str, int, int], dict]" = OrderedDict()
_MANIFEST_PARSE_CACHE_MAX = 2


def _load_manifest_cached(path: Path) -> dict:
    """load_manifest() with a small (path, mtime_ns, size) parse cache.

    On a missing/unreadable manifest, falls back to the uncached load_manifest
    (which returns the empty-manifest default) and caches nothing.
    """
    try:
        stat = path.stat()
    except OSError:
        # No manifest on disk yet (or unreadable): defer to load_manifest, which
        # returns {"files": {}} without raising. Nothing stable to cache.
        return load_manifest(path)

    key = (str(path), stat.st_mtime_ns, stat.st_size)
    cached = _MANIFEST_PARSE_CACHE.get(key)
    if cached is not None:
        _MANIFEST_PARSE_CACHE.move_to_end(key)
        return cached

    manifest = load_manifest(path)
    _MANIFEST_PARSE_CACHE[key] = manifest
    _MANIFEST_PARSE_CACHE.move_to_end(key)
    while len(_MANIFEST_PARSE_CACHE) > _MANIFEST_PARSE_CACHE_MAX:
        _MANIFEST_PARSE_CACHE.popitem(last=False)
    return manifest


def manifest_indexed_files(
    root: Path | str = ".",
    *,
    limit: int = 100,
    offset: int = 0,
    filter_substr: str | None = None,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
) -> dict:
    """Bounded, vector-free listing of the files recorded in the manifest.

    Reads ONLY the manifest (no SQLite/Lance load). File ids are stored
    workspace-relative (see index_file_chunks), so the returned ``path`` is
    relative and never leaks an absolute path. ``limit`` is clamped to
    [1, MAX_INDEXED_FILES_LIMIT]; ``offset`` is clamped to >= 0. ``filter_substr``
    is a case-insensitive substring match on the relative path. Results are
    sorted by path for a stable, paginated UI listing.
    """
    root = Path(root).resolve()
    # Parse-cached read (see _load_manifest_cached): repeated calls on an
    # unchanged manifest version skip re-parsing the JSON. manifest_files_for_root
    # only normalizes idempotently and we read (create=False), so sharing the
    # cached dict across calls is safe.
    manifest = _load_manifest_cached(Path(manifest_path))
    manifest_files = manifest_files_for_root(manifest, root, create=False)

    limit = max(1, min(int(limit), MAX_INDEXED_FILES_LIMIT))
    offset = max(0, int(offset))
    needle = (filter_substr or "").strip().lower()

    file_ids = sorted(manifest_files)
    if needle:
        file_ids = [file_id for file_id in file_ids if needle in file_id.lower()]
    total = len(file_ids)

    window = file_ids[offset : offset + limit]
    files = []
    for file_id in window:
        entry = manifest_files.get(file_id) or {}
        chunks = entry.get("chunks")
        files.append(
            {
                "path": file_id,
                "chunks": int(chunks) if isinstance(chunks, int) else 0,
                "updatedAt": str(entry.get("updated_at") or ""),
            }
        )
    return {"total": total, "files": files, "limit": limit, "offset": offset}


def file_signature(path: Path, chunks: int | None = None) -> dict:
    stat = path.stat()
    payload = {
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "updated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
    }
    if chunks is not None:
        payload["chunks"] = chunks
        payload["chunk_profile"] = active_chunk_profile_version()
    return payload


def load_manifest(path: Path) -> dict:
    if not path.exists():
        return {"files": {}}
    try:
        return json.loads(path.read_text(encoding="utf-8") or "{}")
    except Exception:
        return {"files": {}}


def manifest_roots(manifest: dict) -> dict:
    roots = manifest.setdefault("roots", {})
    legacy_root = manifest.get("root")
    legacy_files = manifest.get("files")
    if legacy_root and isinstance(legacy_files, dict) and legacy_root not in roots:
        roots[legacy_root] = {"files": legacy_files}
    manifest["version"] = 2
    return roots


def strip_verbatim_prefix(value: str) -> str:
    r"""Strip the Windows extended-length / verbatim prefix (``\\?\`` and
    ``\\?\UNC\``) from a path STRING so the same workspace has a single canonical
    manifest identity.

    P4 root cause: ``Path.resolve()`` on Windows can return a ``\\?\C:\…`` form,
    while the app normally passes the plain ``C:\…`` form. Keyed verbatim, the
    manifest ended up with BOTH ``C:\Users\…\aspis bio`` (fully indexed) and a
    stale ``\\?\C:\Users\…\aspis bio`` (32 files) — the Python side treated the
    verbatim form as a brand-new workspace and re-embedded ~1169 "pending" files
    every run ("always indexing"). Collapsing the two forms to one key makes an
    already-indexed workspace look already-indexed regardless of which form a
    given call resolved to. Mirrors the Rust ``strip_windows_verbatim_prefix``.
    No-op on non-Windows path strings.
    """
    if value.startswith("\\\\?\\UNC\\"):
        return "\\\\" + value[len("\\\\?\\UNC\\"):]
    if value.startswith("\\\\?\\"):
        return value[len("\\\\?\\"):]
    return value


def manifest_files_for_root(manifest: dict, root: Path, create: bool) -> dict:
    root_key = strip_verbatim_prefix(str(root))
    roots = manifest_roots(manifest)
    # P4: prune a stale verbatim-prefixed duplicate of THIS root. If a previous run
    # recorded the same workspace under its `\\?\`-prefixed form, merge its file
    # entries into the canonical key (preferring already-present canonical entries)
    # and drop the duplicate, so the workspace is no longer seen as "new" and is not
    # needlessly re-embedded. Only entries that collapse to `root_key` are touched.
    for existing_key in list(roots.keys()):
        if existing_key == root_key:
            continue
        if strip_verbatim_prefix(existing_key) != root_key:
            continue
        duplicate = roots.pop(existing_key)
        if not isinstance(duplicate, dict):
            continue
        canonical = roots.setdefault(root_key, {"files": {}})
        canonical_files = canonical.setdefault("files", {})
        for file_id, record in duplicate.get("files", {}).items():
            canonical_files.setdefault(file_id, record)
    entry = roots.get(root_key)
    if entry is None:
        if not create:
            return {}
        entry = {"files": {}}
        roots[root_key] = entry
    files = entry.setdefault("files", {})
    manifest["root"] = root_key
    manifest["files"] = files
    return files


def sync_legacy_manifest_root(manifest: dict, root: Path) -> None:
    files = manifest_files_for_root(manifest, root, create=True)
    # P4: keep the legacy mirror keyed on the SAME canonical (verbatim-stripped)
    # root as `manifest_files_for_root`, so the two never disagree on identity.
    manifest["root"] = strip_verbatim_prefix(str(root))
    manifest["files"] = files


def save_manifest(path: Path, manifest: dict) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    tmp.replace(path)


def log_progress(enabled: bool, message: str) -> None:
    if enabled:
        print(message, flush=True)


def free_memory_gb() -> float:
    if sys.platform == "win32":
        try:
            import ctypes

            class MemoryStatusEx(ctypes.Structure):
                _fields_ = [
                    ("dwLength", ctypes.c_ulong),
                    ("dwMemoryLoad", ctypes.c_ulong),
                    ("ullTotalPhys", ctypes.c_ulonglong),
                    ("ullAvailPhys", ctypes.c_ulonglong),
                    ("ullTotalPageFile", ctypes.c_ulonglong),
                    ("ullAvailPageFile", ctypes.c_ulonglong),
                    ("ullTotalVirtual", ctypes.c_ulonglong),
                    ("ullAvailVirtual", ctypes.c_ulonglong),
                    ("sullAvailExtendedVirtual", ctypes.c_ulonglong),
                ]

            status = MemoryStatusEx()
            status.dwLength = ctypes.sizeof(status)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
            return round(status.ullAvailPhys / (1024 ** 3), 2)
        except Exception:
            return 0.0
    if sys.platform == "darwin":
        try:
            result = subprocess.run(
                ["vm_stat"],
                capture_output=True,
                text=True,
                timeout=5,
            )
        except Exception:
            return 0.0
        if result.returncode != 0:
            return 0.0
        page_match = re.search(r"page size of (\d+) bytes", result.stdout)
        page_size = int(page_match.group(1)) if page_match else 4096
        pages = 0
        free_only_pages = 0
        for name in ("Pages free", "Pages inactive", "Pages speculative", "Pages purgeable"):
            line_match = re.search(rf"^{name}:\s+(\d+)\.", result.stdout, re.MULTILINE)
            if line_match:
                pages += int(line_match.group(1))
                if name == "Pages free":
                    free_only_pages = int(line_match.group(1))
        vm_stat_gb = round(pages * page_size / (1024 ** 3), 2)
        free_only_gb = round(free_only_pages * page_size / (1024 ** 3), 2)
        return darwin_effective_free_gb(
            vm_stat_gb, free_only_gb, darwin_memory_pressure_level()
        )
    try:
        with open("/proc/meminfo", encoding="utf-8") as handle:
            for line in handle:
                if line.startswith("MemAvailable:"):
                    return round(int(line.split()[1]) / (1024 ** 2), 2)
    except OSError:
        pass
    return 0.0


def darwin_memory_pressure_level() -> int | None:
    """Read the kernel's own pressure verdict (1=normal, 2=warning, 4=critical).

    ``None`` when the sysctl is missing or unreadable — callers then fall back
    to the vm_stat estimate alone.
    """
    try:
        result = subprocess.run(
            ["sysctl", "-n", "kern.memorystatus_vm_pressure_level"],
            capture_output=True,
            text=True,
            timeout=5,
        )
    except Exception:
        return None
    if result.returncode != 0:
        return None
    try:
        return int(result.stdout.strip())
    except ValueError:
        return None


def darwin_effective_free_gb(
    vm_stat_gb: float, free_only_gb: float, pressure_level: int | None
) -> float:
    """vm_stat free+inactive+speculative+purgeable OVERSTATES availability under
    load (observed: ~17GB "free" while swapping 30GB on a thrashing M1 Max).

    LIVE FIX 2026-06-12: the kernel's pressure level is STICKY — it stayed at
    2 (warning) with ~30GB genuinely free long after a thrash episode, which
    zeroed the reading and froze indexing on a healthy machine. Discriminate:
      - critical (>= 4): 0.0 — always pause;
      - warning (2-3): trust only the genuinely FREE pages (during the real
        thrash these were ~0 -> pause; on a recovered machine they are tens of
        GB -> proceed);
      - normal/unknown: the full vm_stat estimate.
    Pure over the parsed values so it is unit-testable.
    """
    if pressure_level is not None and pressure_level >= 4:
        return 0.0
    if pressure_level is not None and pressure_level >= 2:
        return free_only_gb
    return vm_stat_gb


def gpu_temperature_c() -> int | None:
    try:
        result = subprocess.run(
            [
                "nvidia-smi",
                "--query-gpu=temperature.gpu",
                "--format=csv,noheader,nounits",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except Exception:
        return None
    first = result.stdout.strip().splitlines()[0] if result.stdout.strip() else ""
    try:
        return int(float(first.strip()))
    except ValueError:
        return None


def status_payload(
    status: str,
    root: Path,
    sqlite_path: Path | str,
    chunk_vector_path: Path | str,
    manifest_path: Path | str,
    **extra,
) -> dict:
    payload = {
        "status": status,
        "root": str(root),
        "sqlite_path": str(sqlite_path),
        "vector_path": str(chunk_vector_path),
        "manifest_path": str(manifest_path),
        **extra,
    }
    if "free_gb" in payload and "free_ram_gb" not in payload:
        payload["free_ram_gb"] = payload["free_gb"]
    return payload
