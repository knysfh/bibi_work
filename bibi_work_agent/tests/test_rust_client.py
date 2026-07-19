from __future__ import annotations

from typing import Any

import httpx
import pytest
from opentelemetry import context

from bibi_work_agent.clients import rust_client
from bibi_work_agent.clients.rust_client import RustApiError, RustClient, compact_params
from bibi_work_agent.telemetry import extract_context


def test_compact_params_omits_none_values() -> None:
    assert compact_params({"tenant_id": "t", "run_id": None, "limit": 10}) == {
        "tenant_id": "t",
        "limit": 10,
    }


def test_rust_client_ignores_proxy_environment(monkeypatch) -> None:
    seen: list[dict[str, Any]] = []

    class FakeResponse:
        content = b"{}"

        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict[str, Any]:
            return {}

    class FakeHttpClient:
        def __init__(self, **kwargs: Any) -> None:
            seen.append(kwargs)

        def __enter__(self) -> "FakeHttpClient":
            return self

        def __exit__(self, *args: Any) -> None:
            return None

        def post(self, _path: str, json: dict[str, Any]) -> FakeResponse:  # noqa: A002
            return FakeResponse()

        def get(self, _path: str, params: dict[str, Any]) -> FakeResponse:
            return FakeResponse()

    monkeypatch.setenv("ALL_PROXY", "socks5://127.0.0.1:9999")
    monkeypatch.setattr(rust_client.httpx, "Client", FakeHttpClient)

    client = RustClient(base_url="http://127.0.0.1:8361", internal_token="token")
    client.post("/internal/ping", {})
    client.get("/internal/ping", {})

    assert [kwargs["trust_env"] for kwargs in seen] == [False, False]


def test_rust_client_injects_current_w3c_trace_context(monkeypatch) -> None:
    seen: list[dict[str, Any]] = []

    class FakeResponse:
        content = b"{}"

        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict[str, Any]:
            return {}

    class FakeHttpClient:
        def __init__(self, **kwargs: Any) -> None:
            seen.append(kwargs)

        def __enter__(self) -> "FakeHttpClient":
            return self

        def __exit__(self, *args: Any) -> None:
            return None

        def post(self, _path: str, json: dict[str, Any]) -> FakeResponse:  # noqa: A002
            return FakeResponse()

    monkeypatch.setattr(rust_client.httpx, "Client", FakeHttpClient)
    parent = extract_context(
        {"traceparent": ("00-0123456789abcdeffedcba9876543210-0123456789abcdef-01")}
    )
    token = context.attach(parent)
    try:
        RustClient(internal_token="token").post("/internal/ping", {})
    finally:
        context.detach(token)

    assert seen[0]["headers"]["traceparent"] == (
        "00-0123456789abcdeffedcba9876543210-0123456789abcdef-01"
    )


def test_local_exec_wait_uses_bounded_http_timeout(monkeypatch) -> None:
    seen: list[dict[str, Any]] = []
    gets: list[tuple[str, dict[str, Any]]] = []

    class FakeResponse:
        content = b'{"status":"completed"}'

        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict[str, Any]:
            return {"status": "completed"}

    class FakeHttpClient:
        def __init__(self, **kwargs: Any) -> None:
            seen.append(kwargs)

        def __enter__(self) -> "FakeHttpClient":
            return self

        def __exit__(self, *args: Any) -> None:
            return None

        def get(self, path: str, params: dict[str, Any]) -> FakeResponse:
            gets.append((path, params))
            return FakeResponse()

    monkeypatch.setattr(rust_client.httpx, "Client", FakeHttpClient)

    result = RustClient(internal_token="token", timeout_sec=10).local_exec_wait(
        request_id="request-1",
        tenant_id="tenant-1",
        timeout_ms=120_000,
    )

    assert result == {"status": "completed"}
    assert seen[0]["timeout"] == 125
    assert gets == [
        (
            "/internal/local-exec/requests/request-1/wait",
            {"tenant_id": "tenant-1", "timeout_ms": 120_000},
        )
    ]


def test_rust_client_surfaces_backend_validation_details() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            400,
            request=request,
            json={
                "code": "VALIDATION_ERROR",
                "error": "Invalid input, input: only credential-free HTTP(S) browser URLs are allowed",
            },
        )

    client = RustClient(
        base_url="http://127.0.0.1:8361",
        internal_token="token",
        transport=httpx.MockTransport(handler),
    )

    with pytest.raises(RustApiError) as raised:
        client.post("/internal/local-exec/requests", {})

    assert raised.value.status_code == 400
    assert raised.value.code == "VALIDATION_ERROR"
    assert "only credential-free HTTP(S) browser URLs are allowed" in str(raised.value)
