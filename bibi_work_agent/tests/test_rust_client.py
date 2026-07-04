from __future__ import annotations

from typing import Any

from bibi_work_agent.clients import rust_client
from bibi_work_agent.clients.rust_client import RustClient, compact_params


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
