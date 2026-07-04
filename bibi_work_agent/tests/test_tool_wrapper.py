from __future__ import annotations

import json
from uuid import uuid4

import httpx
import pytest

from bibi_work_agent.api.schemas import ActorRef
from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.tools.wrapper import (
    PlatformToolWrapper,
    ToolDenied,
    ToolRequiresApproval,
)


class FakeRust:
    def __init__(self, decision: str, obligations: dict | None = None) -> None:
        self.decision = decision
        self.obligations = obligations
        self.payloads: list = []
        self.tool_call_id = str(uuid4())
        self.emitted_events: list[dict] = []
        self.file_writes: list[dict] = []
        self.fail_emit = False

    def authorize_tool(self, payload):
        self.payloads.append(payload)
        return {
            "decision": {"decision": self.decision, "obligations": self.obligations},
            "tool_call_id": self.tool_call_id,
            "approval_id": str(uuid4()) if self.decision == "review" else None,
        }

    def emit_events(self, *, tenant_id, conversation_id, run_id, events):
        if self.fail_emit:
            raise RuntimeError("event emit failed")
        self.emitted_events.append(
            {
                "tenant_id": tenant_id,
                "conversation_id": conversation_id,
                "run_id": run_id,
                "events": events,
            }
        )

    def file_write(self, payload: dict):
        self.file_writes.append(payload)
        return {
            "id": str(uuid4()),
            "object_reference_id": str(uuid4()),
            "content_type": payload["content_type"],
            "content_hash": "abc",
            "size_bytes": len(payload["inline_content"].encode("utf-8")),
        }


def wrapper(decision: str, obligations: dict | None = None) -> PlatformToolWrapper:
    return PlatformToolWrapper(
        rust=FakeRust(decision, obligations),
        tenant_id=str(uuid4()),
        actor=ActorRef(user_id=uuid4()),
        conversation_id=str(uuid4()),
        run_id=str(uuid4()),
    )


def test_tool_wrapper_allows_authorized_tool() -> None:
    tool_wrapper = wrapper("allow")
    wrapped = tool_wrapper.wrap("read_file", lambda path: f"read:{path}")

    assert wrapped(path="/workspace/a.txt") == "read:/workspace/a.txt"
    assert (
        tool_wrapper.rust.emitted_events[0]["events"][0]["type"]
        == "tool.call.completed"
    )


def test_tool_wrapper_redacts_input_summary() -> None:
    tool_wrapper = wrapper("allow")
    wrapped = tool_wrapper.wrap("send_secret", lambda **_: "ok")

    assert wrapped(password="plain-password", token="secret-token") == "ok"

    summary = tool_wrapper.rust.payloads[0].input_summary
    assert "plain-password" not in summary
    assert "secret-token" not in summary
    assert "[REDACTED]" in summary


def test_tool_wrapper_applies_output_obligations() -> None:
    tool_wrapper = wrapper(
        "allow",
        {
            "redact_fields": ["token"],
            "max_output_bytes": 10_000,
        },
    )
    wrapped = tool_wrapper.wrap(
        "read_secret",
        lambda: {"token": "secret-token", "status": "ok"},
    )

    assert wrapped() == {"token": "[REDACTED]", "status": "ok"}

    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["event_id"] == f"tool.call.completed.{tool_wrapper.rust.tool_call_id}"
    assert event["payload"]["tool_call_id"] == tool_wrapper.rust.tool_call_id
    assert "secret-token" not in event["payload"]["output_summary"]
    assert "[REDACTED]" in event["payload"]["output_summary"]


def test_tool_wrapper_writes_large_tool_result_artifact() -> None:
    rust = FakeRust("allow")
    tool_wrapper = PlatformToolWrapper(
        rust=rust,
        tenant_id=str(uuid4()),
        actor=ActorRef(user_id=uuid4(), device_id=uuid4(), session_id=uuid4()),
        conversation_id=str(uuid4()),
        run_id=str(uuid4()),
        project_id=str(uuid4()),
    )
    wrapped = tool_wrapper.wrap(
        "query_users",
        lambda: [{"name": f"user-{index}"} for index in range(25)],
    )

    wrapped()

    assert rust.file_writes[0]["path"].startswith("/artifacts/tool-results/")
    assert rust.file_writes[0]["content_type"] == "application/x-ndjson"
    event = rust.emitted_events[0]["events"][0]
    assert event["payload"]["views"][0]["data_ref"]["object_reference_id"]
    assert event["payload"]["views"][0]["data_ref"]["content_hash"] == "sha256:abc"


def test_tool_wrapper_passes_ui_hints_to_presenter() -> None:
    tool_wrapper = wrapper("allow")
    wrapped = tool_wrapper.wrap(
        "metrics",
        lambda: {"spec": {"mark": "bar", "encoding": {}}},
        ui_hints={"view": "chart"},
    )

    wrapped()

    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["payload"]["views"][0]["kind"] == "chart"


def test_tool_wrapper_emits_failed_event_and_reraises() -> None:
    tool_wrapper = wrapper("allow")
    wrapped = tool_wrapper.wrap(
        "write_file",
        lambda: (_ for _ in ()).throw(RuntimeError("password=plain-password")),
    )

    with pytest.raises(RuntimeError, match="plain-password"):
        wrapped()

    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["type"] == "tool.call.failed"
    assert event["payload"]["error_type"] == "RuntimeError"
    assert "plain-password" not in event["payload"]["error_summary"]
    assert "[REDACTED]" in event["payload"]["error_summary"]


def test_tool_wrapper_emits_completed_event_through_rust_http() -> None:
    tool_call_id = str(uuid4())
    requests: list[tuple[str, dict, str | None]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        payload = json.loads(request.content.decode("utf-8"))
        requests.append(
            (
                request.url.path,
                payload,
                request.headers.get("authorization"),
            )
        )
        if request.url.path == "/internal/tool-calls:authorize":
            return httpx.Response(
                200,
                json={
                    "decision": {
                        "decision": "allow",
                        "obligations": {
                            "redact_fields": ["token"],
                            "max_output_bytes": 10_000,
                        },
                    },
                    "tool_call_id": tool_call_id,
                },
            )
        if request.url.path == "/internal/run-events":
            return httpx.Response(200, json={"events": []})
        return httpx.Response(404)

    rust = RustClient(
        base_url="http://rust.test",
        internal_token="test-token",
        timeout_sec=1,
        transport=httpx.MockTransport(handler),
    )
    tenant_id = str(uuid4())
    conversation_id = str(uuid4())
    run_id = str(uuid4())
    wrapped = PlatformToolWrapper(
        rust=rust,
        tenant_id=tenant_id,
        actor=ActorRef(user_id=uuid4()),
        conversation_id=conversation_id,
        run_id=run_id,
        trace_id="trace-1",
    ).wrap("read_secret", lambda: {"token": "secret-token", "status": "ok"})

    assert wrapped() == {"token": "[REDACTED]", "status": "ok"}

    assert [request[0] for request in requests] == [
        "/internal/tool-calls:authorize",
        "/internal/run-events",
    ]
    assert {request[2] for request in requests} == {"Bearer test-token"}
    authorize_payload = requests[0][1]
    assert authorize_payload["tenant_id"] == tenant_id
    assert authorize_payload["run_id"] == run_id
    assert authorize_payload["tool_name"] == "read_secret"
    assert json.loads(authorize_payload["input_summary"]) == {"args": [], "kwargs": {}}

    event_payload = requests[1][1]
    event = event_payload["events"][0]
    assert event_payload["conversation_id"] == conversation_id
    assert event_payload["run_id"] == run_id
    assert event["event_id"] == f"tool.call.completed.{tool_call_id}"
    assert event["type"] == "tool.call.completed"
    assert event["trace_id"] == "trace-1"
    assert event["payload"]["tool_call_id"] == tool_call_id
    assert event["payload"]["status"] == "completed"
    assert "secret-token" not in event["payload"]["output_summary"]
    assert "[REDACTED]" in event["payload"]["output_summary"]
    assert event["payload"]["views"][0] == {
        "kind": "json",
        "value_preview": {"status": "ok", "token": "[REDACTED]"},
    }


def test_tool_wrapper_emits_failed_event_through_rust_http() -> None:
    tool_call_id = str(uuid4())
    requests: list[tuple[str, dict]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        payload = json.loads(request.content.decode("utf-8"))
        requests.append((request.url.path, payload))
        if request.url.path == "/internal/tool-calls:authorize":
            return httpx.Response(
                200,
                json={
                    "decision": {"decision": "allow"},
                    "tool_call_id": tool_call_id,
                },
            )
        if request.url.path == "/internal/run-events":
            return httpx.Response(200, json={"events": []})
        return httpx.Response(404)

    rust = RustClient(
        base_url="http://rust.test",
        internal_token="test-token",
        timeout_sec=1,
        transport=httpx.MockTransport(handler),
    )
    wrapped = PlatformToolWrapper(
        rust=rust,
        tenant_id=str(uuid4()),
        actor=ActorRef(user_id=uuid4()),
        conversation_id=str(uuid4()),
        run_id=str(uuid4()),
    ).wrap(
        "write_file",
        lambda: (_ for _ in ()).throw(RuntimeError("token=secret-token")),
    )

    with pytest.raises(RuntimeError, match="secret-token"):
        wrapped()

    assert [request[0] for request in requests] == [
        "/internal/tool-calls:authorize",
        "/internal/run-events",
    ]
    event = requests[1][1]["events"][0]
    assert event["event_id"] == f"tool.call.failed.{tool_call_id}"
    assert event["type"] == "tool.call.failed"
    assert event["payload"]["tool_call_id"] == tool_call_id
    assert event["payload"]["status"] == "failed"
    assert event["payload"]["error_type"] == "RuntimeError"
    assert "secret-token" not in event["payload"]["error_summary"]
    assert "[REDACTED]" in event["payload"]["error_summary"]


def test_tool_wrapper_failed_event_emit_does_not_mask_tool_error() -> None:
    tool_wrapper = wrapper("allow")
    tool_wrapper.rust.fail_emit = True
    wrapped = tool_wrapper.wrap(
        "write_file",
        lambda: (_ for _ in ()).throw(ValueError("bad tool")),
    )

    with pytest.raises(ValueError, match="bad tool"):
        wrapped()


def test_tool_wrapper_raises_for_review() -> None:
    wrapped = wrapper("review").wrap("deploy_site", lambda: "deployed")

    with pytest.raises(ToolRequiresApproval) as exc:
        wrapped()

    assert exc.value.approval_id is not None


def test_tool_wrapper_denies_tool() -> None:
    wrapped = wrapper("deny").wrap("delete_file", lambda: "deleted")

    with pytest.raises(ToolDenied):
        wrapped()
