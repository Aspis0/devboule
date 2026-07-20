#!/usr/bin/env bash
# Build release `devboule-mcp` and stage it for Tauri `externalBin`.
#
# Tauri expects: src-tauri/binaries/<name>-<target-triple>
# At runtime the binary is copied next to the app executable (no triple suffix).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRATE="$ROOT/devboule-mcp"
OUT_DIR="$ROOT/src-tauri/binaries"
NAME="devboule-mcp"

TARGET="$(rustc -vV | sed -n 's/^host: //p')"
if [[ -z "${TARGET}" ]]; then
  echo "stage-devboule-mcp: could not detect rustc host triple" >&2
  exit 1
fi

PROFILE="${DEVBOULE_MCP_STAGE_PROFILE:-release}"
echo "stage-devboule-mcp: building ${NAME} (${PROFILE}) for ${TARGET}…"
(
  cd "$CRATE"
  if [[ "$PROFILE" == "release" ]]; then
    cargo build --release --bin devboule-mcp
    SRC="$CRATE/target/release/${NAME}"
  else
    cargo build --bin devboule-mcp
    SRC="$CRATE/target/debug/${NAME}"
  fi
  if [[ ! -f "$SRC" ]]; then
    # Windows
    if [[ -f "${SRC}.exe" ]]; then
      SRC="${SRC}.exe"
    else
      echo "stage-devboule-mcp: binary not found at $SRC" >&2
      exit 1
    fi
  fi

  mkdir -p "$OUT_DIR"
  STAGED="${OUT_DIR}/${NAME}-${TARGET}"
  if [[ "$SRC" == *.exe ]]; then
    STAGED="${STAGED}.exe"
  fi
  cp -f "$SRC" "$STAGED"
  if [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* && "$(uname -s)" != CYGWIN* ]]; then
    chmod +x "$STAGED"
  fi
  if [[ ! -x "$STAGED" && "$(uname -s)" != MINGW* ]]; then
    echo "stage-devboule-mcp: staged binary is not executable: $STAGED" >&2
    exit 1
  fi

  # Convenience un-suffixed copy for local resolve / PATH testing (not used by Tauri).
  LOCAL="${OUT_DIR}/${NAME}"
  if [[ "$SRC" == *.exe ]]; then
    LOCAL="${LOCAL}.exe"
  fi
  cp -f "$SRC" "$LOCAL"
  if [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* && "$(uname -s)" != CYGWIN* ]]; then
    chmod +x "$LOCAL"
  fi
  if [[ ! -x "$LOCAL" && "$(uname -s)" != MINGW* ]]; then
    echo "stage-devboule-mcp: local binary is not executable: $LOCAL" >&2
    exit 1
  fi

  echo "stage-devboule-mcp: staged → $STAGED"
  ls -la "$STAGED" "$LOCAL"
)
