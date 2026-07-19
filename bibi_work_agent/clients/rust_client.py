from __future__ import annotations

from typing import Any
from uuid import UUID

import httpx

from bibi_work_agent.api.schemas import (
    IngestRunEventsRequest,
    RunEvent,
    ToolAuthorizeRequest,
)
from bibi_work_agent.settings import settings
from bibi_work_agent.telemetry import current_trace_headers


class RustApiError(RuntimeError):
    def __init__(self, message: str, *, status_code: int, code: str | None) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.code = code


class RustClient:
    def __init__(
        self,
        *,
        base_url: str | None = None,
        internal_token: str | None = None,
        timeout_sec: float | None = None,
        transport: httpx.BaseTransport | None = None,
    ) -> None:
        token = settings.internal_token if internal_token is None else internal_token
        self._base_url = base_url or settings.rust_base_url
        self._timeout_sec = (
            settings.request_timeout_sec if timeout_sec is None else timeout_sec
        )
        self._transport = transport
        self._headers = {"Authorization": f"Bearer {token}"}

    def _request_headers(self) -> dict[str, str]:
        return {**self._headers, **current_trace_headers()}

    def post(self, path: str, payload: dict[str, Any]) -> dict[str, Any]:
        with httpx.Client(
            base_url=self._base_url,
            headers=self._request_headers(),
            timeout=self._timeout_sec,
            transport=self._transport,
            trust_env=False,
        ) as client:
            response = client.post(path, json=payload)
            raise_for_status_with_detail(response)
            if not response.content:
                return {}
            return response.json()

    def get(self, path: str, params: dict[str, Any]) -> dict[str, Any]:
        with httpx.Client(
            base_url=self._base_url,
            headers=self._request_headers(),
            timeout=self._timeout_sec,
            transport=self._transport,
            trust_env=False,
        ) as client:
            response = client.get(path, params=compact_params(params))
            raise_for_status_with_detail(response)
            if not response.content:
                return {}
            return response.json()

    def emit_events(
        self,
        *,
        tenant_id: UUID | str,
        conversation_id: UUID | str,
        run_id: UUID | str | None,
        events: list[dict[str, Any]],
    ) -> dict[str, Any]:
        payload = IngestRunEventsRequest(
            tenant_id=tenant_id,
            conversation_id=conversation_id,
            run_id=run_id,
            events=[RunEvent(**event) for event in events],
        )
        return self.post("/internal/run-events", payload.model_dump(mode="json"))

    def authorize_tool(self, payload: ToolAuthorizeRequest) -> dict[str, Any]:
        return self.post(
            "/internal/tool-calls:authorize", payload.model_dump(mode="json")
        )

    def file_read(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/files/read", payload)

    def file_write(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/files/write", payload)

    def file_lock_acquire(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/files/locks/acquire", payload)

    def file_lock_release(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/files/locks/release", payload)

    def file_list(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.get("/internal/files/list", payload)

    def file_search(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/files/search", payload)

    def local_exec_request(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/local-exec/requests", payload)

    def local_exec_wait(
        self,
        *,
        request_id: UUID | str,
        tenant_id: UUID | str,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        timeout_sec = max(
            self._timeout_sec,
            ((timeout_ms or 30_000) / 1_000) + 5,
        )
        with httpx.Client(
            base_url=self._base_url,
            headers=self._request_headers(),
            timeout=timeout_sec,
            transport=self._transport,
            trust_env=False,
        ) as client:
            response = client.get(
                f"/internal/local-exec/requests/{request_id}/wait",
                params=compact_params(
                    {"tenant_id": str(tenant_id), "timeout_ms": timeout_ms}
                ),
            )
            raise_for_status_with_detail(response)
            return response.json() if response.content else {}

    def mcp_tool_call(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/mcp-tools:call", payload)

    def sql_tool_execute(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/sql-tools:execute", payload)

    def third_party_tool_call(self, payload: dict[str, Any]) -> dict[str, Any]:
        return self.post("/internal/third-party-tools:call", payload)

    def runtime_credential(
        self,
        *,
        tenant_id: UUID | str,
        run_id: UUID | str,
        runtime_credential_id: str,
    ) -> dict[str, Any]:
        return self.get(
            f"/internal/runtime-credentials/{runtime_credential_id}",
            {"tenant_id": str(tenant_id), "run_id": str(run_id)},
        )


def raise_for_status_with_detail(response: httpx.Response) -> None:
    try:
        response.raise_for_status()
    except httpx.HTTPStatusError as error:
        code: str | None = None
        message: str | None = None
        try:
            body = response.json()
        except (ValueError, UnicodeDecodeError):
            body = None
        if isinstance(body, dict):
            raw_code = body.get("code")
            raw_message = body.get("error") or body.get("message")
            code = raw_code if isinstance(raw_code, str) else None
            message = raw_message if isinstance(raw_message, str) else None
        summary = f"Rust API returned HTTP {response.status_code}"
        if code:
            summary = f"{summary} {code}"
        if message:
            summary = f"{summary}: {message[:1_000]}"
        raise RustApiError(
            summary,
            status_code=response.status_code,
            code=code,
        ) from error


def compact_params(params: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in params.items() if value is not None}
