#!/usr/bin/env python3
"""
Mock OpenAI-compatible LLM server for the self-test rig.
stdlib only: http.server, socketserver, threading, json, queue, time.

P2b extension: scripted responses can be EITHER
- str                 -> plain assistant text reply (legacy, unchanged)
- dict{"tool": name,
       "arguments": {...},
       "id"?: str}    -> OpenAI-style tool_call reply (finish_reason "tool_calls")

Also: per-request log records whether the request's messages contain a
role:"tool" tool-result message (lets tests assert the sandbox tool result
was actually sent back to the model).

Also: a new fail_mode="midstream_drop" — opens the SSE response (sends
Content-Type + a single role-only chunk) then abruptly closes the socket
before finish_reason / [DONE], simulating a broken LLM connection mid-stream.
"""

from __future__ import annotations

import json
import queue
import socketserver
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, Dict, List, Optional, Union
from urllib.parse import urlparse, parse_qs


# Type for a single scripted response. Backward compatible: str still works.
ScriptedResponse = Union[str, Dict[str, Any]]


def _is_tool_call_response(item: Any) -> bool:
    """A scripted response is a tool-call when it's a dict with key 'tool'."""
    return isinstance(item, dict) and "tool" in item


def _normalize_scripted(item: ScriptedResponse) -> Dict[str, Any]:
    """Normalize a scripted response into a uniform internal dict.

    Returned shape:
      {"kind": "text",    "content": str}
      {"kind": "tool_call", "tool": str, "arguments": dict, "id": str}
    """
    if isinstance(item, str):
        return {"kind": "text", "content": item}
    if _is_tool_call_response(item):
        return {
            "kind": "tool_call",
            "tool": item["tool"],
            "arguments": item.get("arguments", {}),
            # Pre-generate the call id so the SAME id is used across both
            # the streamed tool_calls deltas AND the next request's role:"tool"
            # tool_call_id. The model never sees the id back; only the SDK does.
            "id": item.get("id") or f"call_{uuid.uuid4().hex[:24]}",
        }
    raise ValueError(
        f"Invalid scripted response (must be str or dict with 'tool' key): {item!r}"
    )


class MockLLMRequestHandler(BaseHTTPRequestHandler):
    """HTTP request handler for the mock LLM server."""

    def log_message(self, format: str, *args) -> None:
        # Suppress default log_message; we log via server.mock_llm_server.request_log
        pass

    @property
    def mock_server(self) -> "MockLLMServer":
        return self.server.mock_llm_server  # type: ignore[attr-defined]

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/v1/models":
            self._handle_models()
        else:
            self._send_json(
                404, {"error": {"message": "Not found", "type": "not_found"}}
            )

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/v1/chat/completions":
            self._handle_chat_completions()
        else:
            self._send_json(
                404, {"error": {"message": "Not found", "type": "not_found"}}
            )

    def _handle_models(self) -> None:
        with self.mock_server._lock:
            self.mock_server.request_log.append(
                {
                    "method": "GET",
                    "path": "/v1/models",
                    "body": None,
                    "timestamp": time.time(),
                }
            )
        response = {
            "data": [
                {
                    "id": "rig-model",
                    "object": "model",
                    "created": int(time.time()),
                    "owned_by": "rig",
                }
            ],
            "object": "list",
        }
        self._send_json(200, response)

    def _handle_chat_completions(self) -> None:
        content_length = int(self.headers.get("Content-Length", "0"))
        body = (
            self.rfile.read(content_length).decode("utf-8")
            if content_length > 0
            else "{}"
        )

        # Parse JSON body once. Old code had a duplicated dead-code parse block
        # that shadowed the first assignment; cleaned up here.
        try:
            request = json.loads(body) if body else {}
        except json.JSONDecodeError as e:
            with self.mock_server._lock:
                self.mock_server.request_log.append(
                    {
                        "method": "POST",
                        "path": "/v1/chat/completions",
                        "body": body,
                        "body_preview": body[:500] + ("..." if len(body) > 500 else ""),
                        "timestamp": time.time(),
                        "parse_error": str(e),
                    }
                )
            self._send_json(
                400,
                {"error": {"message": f"Invalid JSON: {e}", "type": "invalid_request"}},
            )
            return

        stream = request.get("stream", False)
        model = request.get("model", "rig-model")
        messages = request.get("messages", [])

        # P2b: record whether any tool-result message is in this request —
        # lets tests prove the SDK actually sent the sandbox tool output back.
        has_tool_result_message = any(
            isinstance(m, dict) and m.get("role") == "tool" for m in messages
        )

        with self.mock_server._lock:
            self.mock_server.request_log.append(
                {
                    "method": "POST",
                    "path": "/v1/chat/completions",
                    "body": body,
                    "body_preview": body[:500] + ("..." if len(body) > 500 else ""),
                    "timestamp": time.time(),
                    "has_tool_result_message": has_tool_result_message,
                    "stream": bool(stream),
                }
            )

            # Check failure injection (under lock for atomicity)
            if self.mock_server.fail_next_n > 0:
                self.mock_server.fail_next_n -= 1
                mode = self.mock_server.fail_mode
                if mode == "500":
                    self._send_json(
                        500,
                        {
                            "error": {
                                "message": "Injected 500 error",
                                "type": "server_error",
                            }
                        },
                    )
                    return
                elif mode == "drop":
                    # Abruptly close connection
                    self.connection.close()
                    return
                elif mode == "midstream_drop":
                    # Special: opens SSE headers + role chunk, then closes
                    # without sending finish_reason / [DONE]. Use ONLY for the
                    # next single request — the connection drop is enough.
                    # NOTE: we cannot call _peek_next_scripted() here because it
                    # acquires self.mock_server._lock and we already hold it
                    # (threading.Lock is non-reentrant → deadlock). Peek inline.
                    try:
                        scripted = self.mock_server._responses.get_nowait()
                        self.mock_server._responses.put(scripted)
                    except queue.Empty:
                        scripted = self.mock_server._default_response
                    self._send_sse_stream_midstream_drop(model, scripted)
                    return

        # Get the next scripted response (FIFO from queue, else default)
        scripted = self.mock_server.get_next_response()

        if scripted["kind"] == "tool_call":
            self._send_tool_call_response(model, scripted, stream, messages)
        else:
            # Legacy text path (unchanged shape).
            self._send_text_response(model, scripted["content"], stream, messages)

    # -------------------------------------------------------------------------
    # Response dispatchers
    # -------------------------------------------------------------------------

    def _send_text_response(
        self, model: str, content: str, stream: bool, messages: List[Dict[str, Any]]
    ) -> None:
        usage = self._make_usage(messages, content)
        if stream:
            self._send_sse_stream(model, content, usage)
        else:
            self._send_json_response(model, content, usage)

    def _send_tool_call_response(
        self,
        model: str,
        scripted: Dict[str, Any],
        stream: bool,
        messages: List[Dict[str, Any]],
    ) -> None:
        # Token accounting is best-effort; sum over message contents.
        total_chars = sum(
            len(m.get("content") or "")
            for m in messages
            if isinstance(m, dict)
        )
        usage = {
            "prompt_tokens": total_chars // 4,
            "completion_tokens": 8,  # placeholder; tool calls cost ~ tokens too
            "total_tokens": 0,
        }
        usage["total_tokens"] = usage["prompt_tokens"] + usage["completion_tokens"]

        if stream:
            self._send_sse_stream_tool_call(model, scripted, usage)
        else:
            # Dead code: the pi SDK always streams (stream:true in every
            # chat/completions POST). This branch is unreachable.
            raise RuntimeError(
                "Non-stream tool-call reply is unsupported; the pi SDK always streams"
            )

    # -------------------------------------------------------------------------
    # Non-stream + stream — TEXT (unchanged legacy shape)
    # -------------------------------------------------------------------------

    def _send_json(self, status: int, data: Dict[str, Any]) -> None:
        body = json.dumps(data).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_json_response(
        self, model: str, content: str, usage: Dict[str, int]
    ) -> None:
        response = {
            "id": f"chatcmpl-{uuid.uuid4().hex[:24]}",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": content},
                    "finish_reason": "stop",
                }
            ],
            "usage": usage,
        }
        self._send_json(200, response)

    def _send_sse_stream(self, model: str, content: str, usage: Dict[str, int]) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        chunk_id = f"chatcmpl-{uuid.uuid4().hex[:24]}"
        created = int(time.time())

        # First chunk: role only (no content)
        first_chunk = {
            "id": chunk_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "delta": {"role": "assistant"},
                    "finish_reason": None,
                }
            ],
        }
        self.wfile.write(f"data: {json.dumps(first_chunk)}\n\n".encode("utf-8"))
        self.wfile.flush()

        # Stream content as multiple text-delta chunks (chunk_size ~8 chars)
        chunk_size = 8
        chunks = MockLLMRequestHandler._split_into_chunks(content, chunk_size)
        for i, chunk_text in enumerate(chunks):
            is_last = i == len(chunks) - 1
            chunk_data = {
                "id": chunk_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "delta": {"content": chunk_text},
                        "finish_reason": None if not is_last else "stop",
                    }
                ],
            }
            if is_last:
                chunk_data["usage"] = usage
            self.wfile.write(f"data: {json.dumps(chunk_data)}\n\n".encode("utf-8"))
            self.wfile.flush()

        # Final [DONE] marker
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

        # Close the connection (Connection: close header was sent)
        self.connection.close()

    # -------------------------------------------------------------------------
    def _send_sse_stream_tool_call(
        self,
        model: str,
        scripted: Dict[str, Any],
        usage: Dict[str, int],
    ) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        chunk_id = f"chatcmpl-{uuid.uuid4().hex[:24]}"
        created = int(time.time())
        tool_call_id = scripted["id"]
        tool_name = scripted["tool"]
        args_json = json.dumps(scripted["arguments"])

        # Chunk 1: role only
        first_chunk = {
            "id": chunk_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "delta": {"role": "assistant"},
                    "finish_reason": None,
                }
            ],
        }
        self.wfile.write(f"data: {json.dumps(first_chunk)}\n\n".encode("utf-8"))
        self.wfile.flush()

        # Chunk 2: tool_calls delta — emits the id, type, and function name,
        # and the FIRST slice of arguments (so the SDK starts building the
        # toolCall block with all identifying fields present). Splitting the
        # arguments into multiple chunks is how OpenAI streams them in
        # practice; the SDK accumulates via partialArgs.
        first_args_slice = args_json[:8]
        rest_args = args_json[8:]
        chunk2 = {
            "id": chunk_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": tool_call_id,
                                "type": "function",
                                "function": {
                                    "name": tool_name,
                                    "arguments": first_args_slice,
                                },
                            }
                        ]
                    },
                    "finish_reason": None,
                }
            ],
        }
        self.wfile.write(f"data: {json.dumps(chunk2)}\n\n".encode("utf-8"))
        self.wfile.flush()

        # Subsequent argument-delta chunks (8-char slices, no id/name — just
        # the arguments field with index). Mirrors what real OpenAI streams
        # emit and lets the SDK accumulate via partialArgs.
        for i in range(0, len(rest_args), 8):
            slice_text = rest_args[i : i + 8]
            chunk = {
                "id": chunk_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "function": {"arguments": slice_text},
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ],
            }
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode("utf-8"))
            self.wfile.flush()

        # Final chunk: empty delta + finish_reason "tool_calls" + usage.
        final_chunk = {
            "id": chunk_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "delta": {},
                    "finish_reason": "tool_calls",
                }
            ],
            "usage": usage,
        }
        self.wfile.write(f"data: {json.dumps(final_chunk)}\n\n".encode("utf-8"))
        self.wfile.flush()

        # [DONE] sentinel
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

        # Close the socket (Connection: close header was set).
        self.connection.close()

    def _send_sse_stream_midstream_drop(
        self, model: str, scripted: Optional[Dict[str, Any]]
    ) -> None:
        """Open SSE headers + send a role-only chunk, then abruptly close.

        Used to simulate a provider connection dying mid-stream. The SDK
        should observe: stream ended without finish_reason → "Stream ended
        without finish_reason" error → eventually `response success:false`.
        """
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        chunk_id = f"chatcmpl-{uuid.uuid4().hex[:24]}"
        created = int(time.time())

        # Single role-only chunk, then close. No content/tool_calls/finish/[DONE].
        first_chunk = {
            "id": chunk_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "delta": {"role": "assistant"},
                    "finish_reason": None,
                }
            ],
        }
        self.wfile.write(f"data: {json.dumps(first_chunk)}\n\n".encode("utf-8"))
        self.wfile.flush()

        # Abrupt close — no [DONE], no finish_reason. The SDK's openai-completions
        # stream loop will exit the for-await because the socket is closed.
        try:
            self.connection.close()
        except Exception:
            pass

    # -------------------------------------------------------------------------
    # helpers
    # -------------------------------------------------------------------------

    def _make_usage(self, messages: List[Dict[str, Any]], content: str) -> Dict[str, int]:
        usage = {
            "prompt_tokens": sum(len(m.get("content") or "") for m in messages) // 4,
            "completion_tokens": len(content) // 4,
            "total_tokens": 0,
        }
        usage["total_tokens"] = usage["prompt_tokens"] + usage["completion_tokens"]
        return usage

    @staticmethod
    def _split_into_chunks(text: str, chunk_size: int = 10) -> List[str]:
        """Split text into chunks of approximately chunk_size characters."""
        if not text:
            return [""]
        chunks = []
        for i in range(0, len(text), chunk_size):
            chunks.append(text[i : i + chunk_size])
        return chunks


class MockLLMServer:
    """
    Mock OpenAI-compatible LLM server running in a background thread.

    Scripted responses are a FIFO list; each incoming chat/completions POST
    consumes one. Each entry is EITHER:
      - str            (assistant text reply; legacy behavior)
      - {"tool": name, "arguments": {...}, "id"?: str}  (tool_call reply)

    Backward compatible: existing callers that pass List[str] keep working
    because str entries still produce plain-text assistant replies.
    """

    def __init__(self, host: str = "127.0.0.1", port: int = 0):
        self.host = host
        self.port = port
        self._server: Optional[HTTPServer] = None
        self._thread: Optional[threading.Thread] = None
        self._ready = threading.Event()
        self._responses: queue.Queue[Dict[str, Any]] = queue.Queue()
        self._default_response: Dict[str, Any] = {"kind": "text", "content": "Ciao! 👋 rig ok"}
        self.request_log: List[Dict[str, Any]] = []
        self.fail_next_n: int = 0
        # "500" | "drop" | "midstream_drop"
        self.fail_mode: str = "500"
        self._lock = threading.Lock()
        self._base_url: Optional[str] = None

    def start(self) -> str:
        """Start the server in a background thread. Returns the base URL."""
        self._server = HTTPServer((self.host, self.port), MockLLMRequestHandler)
        # Store reference to MockLLMServer on the HTTPServer instance
        self._server.mock_llm_server = self  # type: ignore[attr-defined]

        self.port = self._server.server_address[1]
        self._base_url = f"http://{self.host}:{self.port}"

        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        self._ready.wait(timeout=5.0)
        if not self._ready.is_set():
            raise RuntimeError("Mock LLM server failed to start")
        return self.base_url

    def _run(self) -> None:
        self._ready.set()
        self._server.serve_forever()

    def stop(self) -> None:
        """Stop the server and wait for thread to finish."""
        if self._server:
            self._server.shutdown()
            self._server.server_close()
        if self._thread:
            self._thread.join(timeout=5.0)

    @property
    def base_url(self) -> str:
        if not self._base_url:
            raise RuntimeError("Server not started")
        return self._base_url

    @property
    def models_url(self) -> str:
        return f"{self.base_url}/v1/models"

    @property
    def chat_completions_url(self) -> str:
        return f"{self.base_url}/v1/chat/completions"

    def set_responses(self, responses: List[ScriptedResponse]) -> None:
        """Set a queue of scripted responses.

        Each call to chat/completions consumes one. Accepts a mixed list of:
          - str  -> plain assistant text reply
          - {"tool": name, "arguments": {...}, "id"?: str}  -> tool_call reply

        Backward compatible: passing List[str] works exactly as before.
        """
        with self._lock:
            while not self._responses.empty():
                self._responses.get_nowait()
            for r in responses:
                self._responses.put(_normalize_scripted(r))

    def set_default_response(self, response: ScriptedResponse) -> None:
        """Set the default response when the queue is empty."""
        with self._lock:
            self._default_response = _normalize_scripted(response)

    def _peek_next_scripted(self) -> Optional[Dict[str, Any]]:
        """Peek the next scripted response without consuming (under lock)."""
        with self._lock:
            try:
                # Queue.queue has no peek; we drain into a one-item view.
                # Simpler: just get+put-back. Single-producer/single-consumer
                # assumption holds because request handler is serialized per
                # BaseHTTPServer.
                item = self._responses.get_nowait()
                self._responses.put(item)
                return item
            except queue.Empty:
                return self._default_response

    def get_next_response(self) -> Dict[str, Any]:
        """Get the next scripted response (FIFO), or the default."""
        with self._lock:
            try:
                return self._responses.get_nowait()
            except queue.Empty:
                return self._default_response

    def set_fail_mode(self, count: int, mode: str = "500") -> None:
        """
        Inject failures for the next N requests.
        mode: "500" returns HTTP 500
              "drop" closes the connection abruptly
              "midstream_drop" opens SSE + sends a role chunk, then closes
        """
        with self._lock:
            self.fail_next_n = count
            self.fail_mode = mode

    def clear_fail_mode(self) -> None:
        with self._lock:
            self.fail_next_n = 0

    def get_request_log(self) -> List[Dict[str, Any]]:
        with self._lock:
            return list(self.request_log)

    def clear_request_log(self) -> None:
        with self._lock:
            self.request_log.clear()

    def __enter__(self) -> "MockLLMServer":
        self.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.stop()


# Convenience function for quick one-off tests
def run_mock_server(
    responses: Optional[List[ScriptedResponse]] = None, port: int = 0
) -> MockLLMServer:
    """Start a mock server with optional scripted responses."""
    server = MockLLMServer(port=port)
    if responses:
        server.set_responses(responses)
    server.start()
    return server