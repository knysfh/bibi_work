from __future__ import annotations

import json
from uuid import uuid4

import httpx
import pytest

from bibi_work_agent.api.schemas import ActorRef
from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.cancellation import RunCancelled
from bibi_work_agent.runtime.tool_context import (
    clear_file_tool_contexts,
    remember_file_tool_call,
)
from bibi_work_agent.tools import wrapper as wrapper_module
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


def test_file_tool_completion_correlates_with_stream_tool_call_id() -> None:
    clear_file_tool_contexts()
    remember_file_tool_call(
        tool_call_id="call-stream-write",
        tool_name="write_file",
        path="/artifacts/report.txt",
        operation="write_file",
    )
    tool_wrapper = wrapper("allow")
    wrapped = tool_wrapper.wrap(
        "write_file",
        lambda path, content, expected_revision: None,
    )

    wrapped(
        path="/artifacts/report.txt",
        content="done",
        expected_revision=0,
    )

    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["payload"]["tool_call_id"] == tool_wrapper.rust.tool_call_id
    assert event["payload"]["ui_tool_call_id"] == "call-stream-write"
    clear_file_tool_contexts()


def test_tool_wrapper_skips_authorize_when_run_cancelled(monkeypatch) -> None:
    rust = FakeRust("allow")
    called = False
    tool_wrapper = PlatformToolWrapper(
        rust=rust,
        tenant_id=str(uuid4()),
        actor=ActorRef(user_id=uuid4()),
        conversation_id=str(uuid4()),
        run_id=str(uuid4()),
    )

    def mark_called() -> str:
        nonlocal called
        called = True
        return "ok"

    wrapped = tool_wrapper.wrap("write_file", mark_called)
    monkeypatch.setattr(wrapper_module, "is_run_cancelled", lambda _run_id: True)

    with pytest.raises(RunCancelled):
        wrapped()

    assert called is False
    assert rust.payloads == []
    assert rust.emitted_events == []


def test_tool_wrapper_stops_after_tool_when_cancelled_before_artifact(
    monkeypatch,
) -> None:
    rust = FakeRust("allow")
    calls = 0
    tool_wrapper = PlatformToolWrapper(
        rust=rust,
        tenant_id=str(uuid4()),
        actor=ActorRef(user_id=uuid4(), device_id=uuid4(), session_id=uuid4()),
        conversation_id=str(uuid4()),
        run_id=str(uuid4()),
        project_id=str(uuid4()),
    )

    def query_users() -> list[dict[str, str]]:
        nonlocal calls
        calls += 1
        return [{"name": f"user-{index}"} for index in range(25)]

    states = iter([False, False, False, True])
    monkeypatch.setattr(
        wrapper_module,
        "is_run_cancelled",
        lambda _run_id: next(states, True),
    )
    wrapped = tool_wrapper.wrap("query_users", query_users)

    with pytest.raises(RunCancelled):
        wrapped()

    assert calls == 1
    assert rust.file_writes == []
    assert rust.emitted_events == []


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


def test_browser_tool_emits_compact_structured_projection() -> None:
    tool_wrapper = wrapper("allow")
    wrapped = tool_wrapper.wrap(
        "browser_snapshot",
        lambda: {
            "kind": "browser",
            "action": "snapshot",
            "session_id": "browser-session-1",
            "profile": "research",
            "url": "https://www.math.pku.edu.cn/teachers",
            "title": "北京大学数学科学学院",
            "text": "large page text" * 1_000,
            "elements": [{"ref": "e1", "label": "教授名单"}],
            "element_count": 1,
        },
    )

    wrapped()

    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["payload"]["browser"] == {
        "kind": "browser",
        "action": "snapshot",
        "session_id": "browser-session-1",
        "profile": "research",
        "url": "https://www.math.pku.edu.cn/teachers",
        "title": "北京大学数学科学学院",
        "element_count": 1,
    }
    assert "text" not in event["payload"]["browser"]
    assert "elements" not in event["payload"]["browser"]


def test_retryable_browser_failure_is_returned_to_the_agent_loop() -> None:
    tool_wrapper = wrapper("allow")
    recoverable = {
        "kind": "browser",
        "action": "click",
        "session_id": "browser-session-1",
        "status": "failed",
        "retryable": True,
        "error": {
            "code": "BROWSER_TARGET_NOT_ACTIONABLE",
            "message": "stale ref",
        },
        "recovery": {
            "attempt": 1,
            "failure_fingerprint": "abcdef0123456789",
        },
        "recovery_snapshot": {
            "kind": "browser",
            "action": "snapshot",
            "session_id": "browser-session-1",
            "url": "https://portal.example.test/home",
            "title": "Home",
            "element_count": 2,
        },
    }
    wrapped = tool_wrapper.wrap("browser_click", lambda: recoverable)

    assert wrapped() == recoverable
    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["type"] == "tool.call.failed"
    assert event["payload"]["retryable"] is True
    assert event["payload"]["error_type"] == "BROWSER_TARGET_NOT_ACTIONABLE"
    assert event["payload"]["recovery_attempt"] == 1
    assert event["payload"]["browser"] == {
        "kind": "browser",
        "action": "snapshot",
        "session_id": "browser-session-1",
        "url": "https://portal.example.test/home",
        "title": "Home",
        "element_count": 2,
    }


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
        ui_hints={"view": "chart", "title": "Metrics chart"},
    )

    wrapped()

    event = tool_wrapper.rust.emitted_events[0]["events"][0]
    assert event["payload"]["views"][0]["kind"] == "chart"
    assert event["payload"]["views"][0]["title"] == "Metrics chart"


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
    assert authorize_payload["trace_id"] == "trace-1"
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


def test_tool_wrapper_maps_local_exec_to_critical_local_exec_resource() -> None:
    device_id = uuid4()
    tool_wrapper = PlatformToolWrapper(
        rust=FakeRust("allow"),
        tenant_id=str(uuid4()),
        actor=ActorRef(user_id=uuid4(), device_id=device_id),
        conversation_id=str(uuid4()),
        run_id=str(uuid4()),
    )
    wrapped = tool_wrapper.wrap("local_exec", lambda command: {"status": "queued"})

    wrapped(["pwd"])

    payload = tool_wrapper.rust.payloads[0]
    assert payload.risk_level == "critical"
    assert payload.resource == {"type": "local_exec", "id": str(device_id)}


def test_tool_wrapper_maps_sql_tool_and_query_resources() -> None:
    sql_tool_id = uuid4()
    tool_wrapper = wrapper("allow")
    sql_tool = tool_wrapper.wrap("sql_execute", lambda **_: {"rows": []})

    sql_tool(sql_tool_id=sql_tool_id)

    assert tool_wrapper.rust.payloads[0].resource == {
        "type": "sql_tool",
        "id": str(sql_tool_id),
    }

    query_hash = "sha256:query"
    sql_query = tool_wrapper.wrap("sql_query", lambda **_: {"rows": []})
    sql_query(query_hash=query_hash)

    assert tool_wrapper.rust.payloads[1].resource == {
        "type": "sql_query",
        "id": query_hash,
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
    tool_wrapper = wrapper("review")
    called = False

    def deploy_site() -> str:
        nonlocal called
        called = True
        return "deployed"

    wrapped = tool_wrapper.wrap("deploy_site", deploy_site)

    with pytest.raises(ToolRequiresApproval) as exc:
        wrapped()

    assert exc.value.approval_id is not None
    assert called is False
    assert len(tool_wrapper.rust.payloads) == 1
    assert tool_wrapper.rust.emitted_events == []


def test_tool_wrapper_denies_tool() -> None:
    tool_wrapper = wrapper("deny")
    called = False

    def delete_file() -> str:
        nonlocal called
        called = True
        return "deleted"

    wrapped = tool_wrapper.wrap("delete_file", delete_file)

    with pytest.raises(ToolDenied):
        wrapped()

    assert called is False
    assert len(tool_wrapper.rust.payloads) == 1
    assert tool_wrapper.rust.emitted_events == []
