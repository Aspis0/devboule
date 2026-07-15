#!/usr/bin/env python3
"""
Smoke tests for the self-test rig (Layer A).
Run only when RIG=1 environment variable is set.
"""

import os
import pytest
import uuid
from pathlib import Path

# Skip entire module unless RIG=1 is set
pytestmark = pytest.mark.skipif(
    not os.environ.get("RIG"),
    reason="Self-test rig disabled. Set RIG=1 to run: RIG=1 python -m pytest rig/ -v",
)

from rig.mock_llm import MockLLMServer
from rig.world import build_world_in_temp
from rig.sidecar_driver import SidecarSession


def test_ciao_smoke():
    """
    Round-5 regression guard: orchestrator says "ciao", gets a greeting back.
    Asserts the full event choreography: ready -> prompt -> assistant text -> response success.
    """
    with build_world_in_temp() as world:
        # Start mock LLM with scripted response
        with MockLLMServer() as mock:
            mock.set_responses(["Ciao! 👋 rig ok"])

            # R6: use uuid-based session id instead of id(mock)
            session_id = f"rig-test-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="orchestrator",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                # 1. Wait for ready event with oracleMCP boolean
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"
                assert "oracleMCP" in ready_event.data
                assert isinstance(ready_event.data["oracleMCP"], bool)
                oracle_mcp = ready_event.data["oracleMCP"]
                print(f"[test] ready event: oracleMCP={oracle_mcp}")

                # 2. Send prompt. R10: clear the mock request log right before sending.
                mock.clear_request_log()
                session.send_prompt("ciao")

                # 3. Collect events using the public wait_event API (R3).
                # Track user "ciao" index and first assistant message_start index
                # for R4 ordering invariant.
                user_ciao_index: int | None = None
                first_assistant_msg_start_index: int | None = None
                assistant_text_chunks = []
                response_event = None
                response_event_index: int | None = None

                deadline = 60.0
                import time

                start = time.time()
                scan_index = 0
                while time.time() - start < deadline:
                    try:
                        event = session.wait_event(
                            lambda e: True,  # accept any event
                            timeout=0.5,
                            start_index=scan_index,
                        )
                    except TimeoutError:
                        continue

                    scan_index = session.events.index(event) + 1

                    # R3: check process liveness while waiting
                    if not session.is_alive():
                        raise RuntimeError(
                            f"Sidecar process exited unexpectedly (returncode={session._proc.returncode}).\n"
                            f"{session.dump_state()}"
                        )

                    etype = event.data.get("type")
                    if etype in ("message_update", "assistant", "message_start"):
                        # Track first assistant message_start for R4
                        if first_assistant_msg_start_index is None:
                            msg = event.data.get("message")
                            if isinstance(msg, dict) and msg.get("role") == "assistant":
                                first_assistant_msg_start_index = scan_index - 1

                        # Extract text content from various pi SDK event shapes
                        text = _extract_assistant_text_from_event(event.data)
                        if text:
                            assistant_text_chunks.append(text)
                    elif etype == "response" and event.data.get("command") == "prompt":
                        response_event = event
                        response_event_index = scan_index - 1
                        break

                # 4. Assert response event exists and success=true
                assert response_event is not None, "No response event received"
                assert response_event.data.get("success") == True, (
                    f"Response failed: {response_event.data.get('error')}"
                )

                # 5. Assert concatenated assistant text is non-empty and contains "rig ok"
                full_text = "".join(assistant_text_chunks)
                print(f"[test] Assistant text: {full_text!r}")
                assert full_text, "Assistant produced no text content"
                assert "rig ok" in full_text.lower(), (
                    f"Expected 'rig ok' in response, got: {full_text!r}"
                )

                # R4: USER-ECHO invariant — an event with message.role=="user"
                # and text "ciao" must appear BEFORE the first assistant message_start.
                user_ciao_event = None
                for idx, e in enumerate(session.events):
                    msg = e.data.get("message")
                    if (
                        isinstance(msg, dict)
                        and msg.get("role") == "user"
                        and "content" in msg
                    ):
                        content = msg["content"]
                        # content may be a list of text blocks or a string
                        text_parts = []
                        if isinstance(content, list):
                            for block in content:
                                if isinstance(block, dict) and block.get("type") == "text":
                                    text_parts.append(block.get("text", ""))
                        elif isinstance(content, str):
                            text_parts.append(content)
                        full_user_text = " ".join(text_parts)
                        if "ciao" in full_user_text.lower():
                            user_ciao_event = (idx, e)
                            break
                assert user_ciao_event is not None, (
                    "No user-role 'ciao' event found in event stream"
                )
                user_ciao_idx = user_ciao_event[0]
                assert first_assistant_msg_start_index is not None, (
                    "No assistant message_start event found"
                )
                assert user_ciao_idx < first_assistant_msg_start_index, (
                    f"User 'ciao' event (index {user_ciao_idx}) must appear BEFORE "
                    f"the first assistant message_start (index {first_assistant_msg_start_index})"
                )

                # 6. Assert mock received the POST and the body contains "ciao" (R10)
                requests = mock.get_request_log()
                post_requests = [
                    r
                    for r in requests
                    if r["method"] == "POST" and r["path"] == "/v1/chat/completions"
                ]
                assert post_requests, "Mock did not receive POST /v1/chat/completions"
                print(
                    f"[test] Mock received {len(post_requests)} chat completion request(s)"
                )
                # Verify the prompt body actually contains "ciao"
                body_text = post_requests[0].get("body", "")
                assert "ciao" in body_text.lower(), (
                    f"POST body does not contain 'ciao': {body_text!r}"
                )


def test_provider_failure_surfaces_error():
    """
    Mock returns 500s. Assert the turn ends with response success:false (with error)
    AND at least one auto_retry_start observed.
    Invariant: NO silent success, and failure is visible in event stream.
    """
    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # Fail ALL requests permanently (sidecar retries 3x by default, so need >3 failures)
            mock.set_fail_mode(count=100, mode="500")

            # R6: use uuid-based session id
            session_id = f"rig-fail-test-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="orchestrator",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                # Wait for ready
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # Send prompt (auto-retry is internal to pi SDK; sidecar doesn't expose set_auto_retry)
                session.send_prompt("ciao")

                # Wait for turn completion (response event OR error event)
                response_event = None
                error_events = []
                auto_retry_events = []

                deadline = 60.0
                import time

                start = time.time()
                scan_index = 0
                while time.time() - start < deadline:
                    try:
                        event = session.wait_event(
                            lambda e: True,  # accept any event
                            timeout=0.5,
                            start_index=scan_index,
                        )
                    except TimeoutError:
                        continue

                    scan_index = session.events.index(event) + 1

                    # R3: check process liveness while waiting
                    if not session.is_alive():
                        raise RuntimeError(
                            f"Sidecar process exited unexpectedly (returncode={session._proc.returncode}).\n"
                            f"{session.dump_state()}"
                        )

                    etype = event.data.get("type")
                    if etype == "response" and event.data.get("command") == "prompt":
                        response_event = event
                        break
                    elif etype == "error":
                        error_events.append(event)
                    elif etype and etype.startswith("auto_retry_"):
                        auto_retry_events.append(event)

                # Assert: we got a terminal response event
                assert response_event is not None, (
                    f"No response event received. Errors: {len(error_events)}, "
                    f"Auto-retries: {len(auto_retry_events)}"
                )

                # R5 (unconditional — sidecar is now FIXED): assert response success is False
                # AND error is non-empty AND at least one auto_retry_start observed.
                assert response_event.data.get("success") == False, (
                    f"Expected response success=False, got: {response_event.data}"
                )
                error_msg = response_event.data.get("error")
                assert error_msg, (
                    f"Expected non-empty error message, got: {error_msg!r}"
                )

                # Assert at least one auto_retry_start event observed
                auto_retry_start_events = [
                    e for e in auto_retry_events if e.data.get("type") == "auto_retry_start"
                ]
                assert auto_retry_start_events, (
                    "Failure not visible in event stream — no auto_retry_start events"
                )
                print(
                    f"[test] Failure surfaced correctly in response: {error_msg}"
                )

                # Additional visibility: error events should also exist
                assert error_events or auto_retry_events, (
                    "Failure response had no preceding error/auto_retry events — "
                    "silent failure path detected"
                )


def _extract_assistant_text_from_event(event_data: dict) -> str:
    """
    Extract ASSISTANT text content from various pi SDK event shapes.
    Handles: message_update, assistant, message_start, text_delta, etc.
    Ignores user messages.
    """
    etype = event_data.get("type")

    # message_update events: contain assistantMessageEvent with the actual delta
    if etype == "message_update":
        ame = event_data.get("assistantMessageEvent", {})
        if ame:
            return _extract_assistant_text_from_event(ame)

    # message_start/end events: have message.content with text blocks
    # Only extract if role is assistant
    if "message" in event_data:
        msg = event_data["message"]
        if (
            isinstance(msg, dict)
            and msg.get("role") == "assistant"
            and "content" in msg
        ):
            content = msg["content"]
            if isinstance(content, list):
                parts = []
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        parts.append(block.get("text", ""))
                return "".join(parts)
            elif isinstance(content, str):
                return content

    # text_delta events (these are always assistant)
    if etype == "text_delta":
        return event_data.get("delta", "")

    # assistant events (some SDK versions)
    if etype == "assistant":
        content = event_data.get("content")
        if isinstance(content, list):
            parts = []
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    parts.append(block.get("text", ""))
            return "".join(parts)

    # text_start/text_end events
    if etype in ("text_start", "text_end"):
        # These have partial message but not the delta text itself
        pass

    return ""


if __name__ == "__main__":
    # Allow running directly for quick debugging
    os.environ["RIG"] = "1"
    pytest.main([__file__, "-v", "-s"])
