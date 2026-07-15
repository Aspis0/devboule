#!/usr/bin/env python3
"""
Mock OpenAI-compatible LLM server for the self-test rig.
stdlib only: http.server, socketserver, threading, json, queue, time.
"""

from __future__ import annotations

import json
import queue
import socketserver
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, Callable, Dict, List, Optional
from urllib.parse import urlparse, parse_qs


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

        with self.mock_server._lock:
            self.mock_server.request_log.append(
                {
                    "method": "POST",
                    "path": "/v1/chat/completions",
                    "body": body,
                    "body_preview": body[:500] + ("..." if len(body) > 500 else ""),
                    "timestamp": time.time(),
                }
            )

            # Check failure injection (under lock for atomicity)
            if self.mock_server.fail_next_n > 0:
                self.mock_server.fail_next_n -= 1
                if self.mock_server.fail_mode == "500":
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
                elif self.mock_server.fail_mode == "drop":
                    # Abruptly close connection
                    self.connection.close()
                    return

        request = json.loads(body) if body else {}
        try:
            if body:
                request = json.loads(body)
        except json.JSONDecodeError as e:
            self._send_json(
                400,
                {"error": {"message": f"Invalid JSON: {e}", "type": "invalid_request"}},
            )
            return

        stream = request.get("stream", False)
        model = request.get("model", "rig-model")
        messages = request.get("messages", [])

        # Get the next scripted response
        response_text = self.mock_server.get_next_response()
        usage = {
            "prompt_tokens": sum(len(m.get("content", "")) for m in messages) // 4,
            "completion_tokens": len(response_text) // 4,
            "total_tokens": 0,
        }
        usage["total_tokens"] = usage["prompt_tokens"] + usage["completion_tokens"]

        if stream:
            self._send_sse_stream(model, response_text, usage)
        else:
            self._send_json_response(model, response_text, usage)

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
    """

    def __init__(self, host: str = "127.0.0.1", port: int = 0):
        self.host = host
        self.port = port
        self._server: Optional[HTTPServer] = None
        self._thread: Optional[threading.Thread] = None
        self._ready = threading.Event()
        self._responses: queue.Queue[str] = queue.Queue()
        self._default_response = "Ciao! 👋 rig ok"
        self.request_log: List[Dict[str, Any]] = []
        self.fail_next_n: int = 0
        self.fail_mode: str = "500"  # "500" or "drop"
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

    def set_responses(self, responses: List[str]) -> None:
        """Set a queue of scripted responses. Each call to chat/completions consumes one."""
        with self._lock:
            while not self._responses.empty():
                self._responses.get_nowait()
            for r in responses:
                self._responses.put(r)

    def set_default_response(self, response: str) -> None:
        """Set the default response when the queue is empty."""
        with self._lock:
            self._default_response = response

    def get_next_response(self) -> str:
        """Get the next scripted response, or the default."""
        with self._lock:
            try:
                return self._responses.get_nowait()
            except queue.Empty:
                return self._default_response

    def set_fail_mode(self, count: int, mode: str = "500") -> None:
        """
        Inject failures for the next N requests.
        mode: "500" returns HTTP 500, "drop" closes the connection abruptly.
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
    responses: Optional[List[str]] = None, port: int = 0
) -> MockLLMServer:
    """Start a mock server with optional scripted responses."""
    server = MockLLMServer(port=port)
    if responses:
        server.set_responses(responses)
    server.start()
    return server
