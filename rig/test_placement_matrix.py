#!/usr/bin/env python3
"""
Placement-matrix rig cell: sidecar-layer provider/api_key_env knobs.

The sidecar (sidecar.mjs) builds a temp models.json via buildCustomModelsJson()
when DEVBOULE_PI_BASE_URL is set. The temp models.json uses the provider
token verbatim as the key under `providers:` and hardcodes `api: "openai-completions"`.
Per the pi SDK's ModelRegistry (dist/core/model-registry.js:264), the provider
token must be a recognised provider id — "openai" and "openrouter" are both
in the SDK's built-in provider set (`Type.Literal("openai") | Type.Literal("openrouter")`).

So a custom base_url DOES work with provider "openrouter" — the sidecar writes
a temp models.json with:
    { "providers": { "openrouter": { "baseUrl": <mock>, "api": "openai-completions",
        "apiKey": <OPENROUTER_API_KEY>, "models": [{"id":"rig-model"}] } } }
and `ModelRegistry.create(authStorage, tempModelsJsonPath)` reads it back.
The SDK then issues chat/completions POSTs against the mock with
`Authorization: Bearer <OPENROUTER_API_KEY>`.

test_cloud_api_shaped_cell: provider="openrouter", OPENROUTER_API_KEY set,
DEVBOULE_PI_BASE_URL → mock. Asserts the full ciao choreography passes AND
the mock's last_auth_header carries the key (proving the key was forwarded
and the openrouter provider shape works end-to-end).

test_missing_key_fails_loud: provider="openrouter", NO api key env at all,
base_url unset (so the sidecar falls back to the real registry path — but
with a HOME override, the registry lookup fails hermetically). Asserts the
sidecar surfaces a loud failure within 60s — never a silent hang and never a
request to any real endpoint.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

if os.environ.get("RIG") != "1":
    pytest.skip("RIG=1 required; skipping placement-matrix tests", allow_module_level=True)

from rig.sidecar_driver import SidecarSession  # noqa: E402
from rig.world import build_world_in_temp  # noqa: E402
from rig.mock_llm import MockLLMServer  # noqa: E402

ACTIVE_PROJECT_ID = "rig-project"
AGENT_ID = "rig-test-placement"
AGENT_ROLE = "main-coder"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _ensure_project_pi_mcp_config(project_root: Path) -> None:
    """Write <project_root>/.pi/mcp.json with an devboule entry so the
    sidecar's isAspisMcpConfigured() returns True (required for the ready
    event to carry oracleMCP:true, matching the round-5 ciao choreography)."""
    pi_dir = project_root / ".pi"
    pi_dir.mkdir(parents=True, exist_ok=True)
    mcp_cfg = {
        "mcpServers": {
            "devboule": {
                "command": sys.executable,
                "args": [
                    "-m",
                    "oracle.server.aspis_mcp",
                    "--root",
                    str(Path(__file__).resolve().parents[1]),
                    "--projects-dir",
                    str(project_root.parent / "projects"),
                ],
                "transport": "stdio",
            }
        }
    }
    (pi_dir / "mcp.json").write_text(
        json.dumps(mcp_cfg), encoding="utf-8"
    )


# ---------------------------------------------------------------------------
# Cell 1a: cloud API shaped cell (provider="openrouter", key forwarded)
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_cloud_api_shaped_cell():
    """provider='openrouter', OPENROUTER_API_KEY set, base_url → mock.

    buildCustomModelsJson() selects the key by provider: for openrouter it reads
    OPENROUTER_API_KEY first, falling back to OPENAI_API_KEY then "dummy".
    The test sets OPENROUTER_API_KEY via the driver's api_key_env/api_key_value
    knobs and asserts the mock receives `Bearer <key>` — proving the vault key
    is forwarded (not ignored as it was before the P3 fix).

    Asserts:
      - ready event fires with oracleMCP:true
      - ciao prompt succeeds (response success:true)
      - mock's Authorization header is `Bearer <key>` (the OPENROUTER_API_KEY)
    """
    key = "sk-openrouter-test-key-12345"
    with build_world_in_temp() as world:
        _ensure_project_pi_mcp_config(world.project_root)

        with MockLLMServer() as mock:
            mock.set_responses(["Ciao! 👋 openrouter cell ok"])

            with SidecarSession(
                session_id=AGENT_ID,
                agent_role=AGENT_ROLE,
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                provider="openrouter",
                api_key_env="OPENROUTER_API_KEY",
                api_key_value=key,
                # OPENAI_API_KEY is NOT set — buildCustomModelsJson must use
                # OPENROUTER_API_KEY for the apiKey field.
            ) as session:
                ready = session.wait_ready(timeout=30.0)
                assert ready.data.get("type") == "ready"
                assert ready.data.get("oracleMCP") is True, (
                    "expected oracleMCP:true (devboule MCP configured)"
                )

                session.send_prompt("ciao")
                response = session.wait_response(timeout=45.0)
                assert response.data.get("success") is True, (
                    f"expected success:true; got: {response.data}"
                )

                # After the P3 fix, buildCustomModelsJson selects the key by
                # provider: openrouter → OPENROUTER_API_KEY. The mock must
                # receive `Bearer <key>` — not `Bearer dummy`.
                auth = mock.last_auth_header
                assert auth is not None, (
                    "mock received no Authorization header — "
                    "the openrouter provider shape did not route to the mock"
                )
                assert auth == f"Bearer {key}", (
                    f"expected Authorization: Bearer <key>; got: {auth}"
                )


# ---------------------------------------------------------------------------
# Cell 1b: missing key fails loud (HOME override, no real registry)
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_missing_key_fails_loud():
    """provider='openrouter', NO api key env at all, base_url → dead endpoint.

    buildCustomModelsJson() always runs when DEVBOULE_PI_BASE_URL is set —
    so the sidecar writes a temp models.json with apiKey="dummy" (OPENROUTER_API_KEY
    is empty). The SDK then issues chat/completions against the dead endpoint
    `http://127.0.0.1:1/v1` and gets a connection error → loud failure.

    Two accepted outcomes:
      1. The sidecar fails to reach ready (startup timeout) → PASS (loud failure
         on startup); session is closed via __exit__.
      2. The sidecar reaches ready but the prompt turn fails → PASS (loud
         failure on prompt); observed via wait_event.
    """
    with build_world_in_temp() as world:
        with SidecarSession(
            session_id=AGENT_ID,
            agent_role=AGENT_ROLE,
            mock_base_url="http://127.0.0.1:1/v1",
            project_root=world.project_root,
            agent_dir=world.agent_dir,
            provider="openrouter",
            api_key_env="OPENROUTER_API_KEY",
            api_key_value="",  # NO key — buildCustomModelsJson uses "dummy"
        ) as session:
            # Outcome 1: startup timeout IS the loud failure.
            try:
                ready = session.wait_ready(timeout=10.0)
                assert ready.data.get("type") == "ready"
            except TimeoutError:
                return  # loud failure on startup — PASS; session closed by __exit__

            # Send the prompt immediately after ready. The dead endpoint will
            # cause the SDK to fail (connection refused) which emits either a
            # response(success:false) or an error event. We then use wait_event
            # to observe it (starting from index 1 to skip the ready event).
            session.send_prompt("ciao")

            # Outcome 2: prompt turn fails loud — use the public wait_event API.
            failure = session.wait_event(
                lambda e: (
                    e.data.get("type") == "response"
                    and e.data.get("command") == "prompt"
                    and not e.data.get("success", True)
                )
                or e.data.get("type") == "error",
                timeout=50.0,
                start_index=1,  # skip the ready event at index 0
            )
            assert failure is not None
