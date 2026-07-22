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
  # F22: macOS kills an in-place overwrite of a running/mapped signed binary
  # (SIGKILL / invalidated code signature). Always replace via new inode:
  # write to a temp path, then rm destination + mv (never `cp -f` over existing).
  install_new_inode() {
    local src="$1" dest="$2"
    local tmp="${dest}.new.$$"
    # Fail closed: never rm the live dest if the staging copy failed (audit F22).
    if ! cp "$src" "$tmp"; then
      rm -f "$tmp"
      echo "stage-devboule-mcp: cp failed: $src → $tmp" >&2
      return 1
    fi
    if [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* && "$(uname -s)" != CYGWIN* ]]; then
      chmod +x "$tmp" || { rm -f "$tmp"; return 1; }
    fi
    rm -f "$dest"
    mv "$tmp" "$dest" || {
      echo "stage-devboule-mcp: mv failed: $tmp → $dest" >&2
      rm -f "$tmp"
      return 1
    }
  }
  install_new_inode "$SRC" "$STAGED"
  if [[ ! -x "$STAGED" && "$(uname -s)" != MINGW* ]]; then
    echo "stage-devboule-mcp: staged binary is not executable: $STAGED" >&2
    exit 1
  fi

  # Convenience un-suffixed copy for local resolve / PATH testing (not used by Tauri).
  LOCAL="${OUT_DIR}/${NAME}"
  if [[ "$SRC" == *.exe ]]; then
    LOCAL="${LOCAL}.exe"
  fi
  install_new_inode "$SRC" "$LOCAL"
  if [[ ! -x "$LOCAL" && "$(uname -s)" != MINGW* ]]; then
    echo "stage-devboule-mcp: local binary is not executable: $LOCAL" >&2
    exit 1
  fi

  # Optional: refresh src-tauri/target/debug sibling for tools that hardcode it
  # (still prefer crate target via resolve_devboule_mcp_bin — F02).
  if [[ "${DEVBOULE_MCP_SYNC_APP_TARGET:-0}" == "1" ]]; then
    APP_DBG="$ROOT/src-tauri/target/debug/${NAME}"
    if [[ "$SRC" == *.exe ]]; then
      APP_DBG="${APP_DBG}.exe"
    fi
    mkdir -p "$(dirname "$APP_DBG")"
    install_new_inode "$SRC" "$APP_DBG"
    echo "stage-devboule-mcp: also synced → $APP_DBG"
  fi

  echo "stage-devboule-mcp: staged → $STAGED"
  ls -la "$STAGED" "$LOCAL"
)
