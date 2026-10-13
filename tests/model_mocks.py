"""Fake provider APIs for ModelRunner tests: httpx2 mock transports, no network, no spend."""

from __future__ import annotations

import json
from collections.abc import Callable

import httpx2


class FakeAPI:
    """Records every request and answers from a queue of responses (or a callable)."""

    def __init__(self, *responses: httpx2.Response | Callable[[httpx2.Request], httpx2.Response]):
        self.responses = list(responses)
        self.requests: list[httpx2.Request] = []

    def handler(self, request: httpx2.Request) -> httpx2.Response:
        self.requests.append(request)
        nxt = self.responses.pop(0) if len(self.responses) > 1 else self.responses[0]
        return nxt(request) if callable(nxt) else nxt

    def client(self) -> httpx2.Client:
        return httpx2.Client(transport=httpx2.MockTransport(self.handler))

    def body(self, i: int = -1) -> dict:
        return json.loads(self.requests[i].content)


def anthropic_message(text="14", *, input_tokens=100, output_tokens=50, cache_read=0,
                      cache_write=0, stop_reason="end_turn") -> httpx2.Response:  # fmt: skip
    return httpx2.Response(
        200,
        json={
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": [
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "text", "text": text},
            ],
            "stop_reason": stop_reason,
            "stop_sequence": None,
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_read_input_tokens": cache_read,
                "cache_creation_input_tokens": cache_write,
            },
        },
    )


def chat_completion(text="14", *, prompt_tokens=120, completion_tokens=30, cached=0,
                    finish_reason="stop") -> httpx2.Response:  # fmt: skip
    return httpx2.Response(
        200,
        json={
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1,
            "model": "test-model",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": text},
                    "finish_reason": finish_reason,
                }
            ],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens + completion_tokens,
                "prompt_tokens_details": {"cached_tokens": cached},
            },
        },
    )


def api_error(status: int, retry_after: str | None = None) -> httpx2.Response:
    headers = {"retry-after": retry_after} if retry_after else {}
    return httpx2.Response(
        status, headers=headers, json={"type": "error", "error": {"type": "x", "message": "boom"}}
    )
