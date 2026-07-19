from __future__ import annotations

from uuid import uuid4

import pytest

from bibi_work_agent.runtime import resume_executor
from bibi_work_agent.runtime.resume_idempotency import ResumeIdempotencyStore


class FakeRust:
    def __init__(self) -> None:
        self.emitted_batches: list[dict] = []

    def emit_events(self, **kwargs):
        self.emitted_batches.append(kwargs)
        return {}


class FakeLease:
    def __init__(self, acquired: bool) -> None:
        self.approval_id = "approval"
        self.acquired = acquired


class FakeIdempotencyStore:
    def __init__(self, acquired: bool = True) -> None:
        self.acquired = acquired
        self.completed: list[str] = []
        self.failed: list[str] = []

    def acquire(self, approval_id: str):
        return FakeLease(self.acquired)

    def mark_completed(self, approval_id: str) -> None:
        self.completed.append(approval_id)

    def mark_failed(self, approval_id: str) -> None:
        self.failed.append(approval_id)


def valid_run_snapshot(
    *,
    tenant_id: str,
    conversation_id: str,
    run_id: str,
    thread_id: str,
) -> dict:
    return {
        "runtime": {"kind": "deepagents"},
        "tenant_id": tenant_id,
        "conversation_id": conversation_id,
        "run_id": run_id,
        "thread_id": thread_id,
        "actor": {"user_id": str(uuid4())},
        "agent": {
            "id": str(uuid4()),
            "model": {
                "provider": "test",
                "model_name": "fake-model",
            },
        },
        "tools": [],
        "skills": [],
        "mcp_tools": [],
        "workspace": {"workspace_id": str(uuid4()), "local_mounts": []},
        "ui": {"client": "biwork"},
    }


def resume_payload(
    *,
    tenant_id: str | None = None,
    conversation_id: str | None = None,
    approval_id: str | None = None,
) -> dict:
    payload_tenant_id = tenant_id or str(uuid4())
    payload_conversation_id = conversation_id or str(uuid4())
    run_id = str(uuid4())
    thread_id = f"thread-{run_id}"
    return {
        "tenant_id": payload_tenant_id,
        "conversation_id": payload_conversation_id,
        "run_id": run_id,
        "approval_id": approval_id or str(uuid4()),
        "trace_id": "trace",
        "thread_id": thread_id,
        "input": {"messages": []},
        "run_config_snapshot": valid_run_snapshot(
            tenant_id=payload_tenant_id,
            conversation_id=payload_conversation_id,
            run_id=run_id,
            thread_id=thread_id,
        ),
        "decision_payload": {"decision": "approved"},
    }


def test_resume_run_payload_continues_from_command(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore()
    seen: dict = {}

    class FakeAgent:
        def stream(self, command, config=None):
            seen["command"] = command
            seen["config"] = config
            yield {
                "type": "message.completed",
                "payload": {
                    "result": {
                        "memory_candidates": [
                            {
                                "content": "审批通过后继续执行高风险工具。",
                                "layer": "semantic",
                            }
                        ]
                    }
                },
            }

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        resume_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    payload = resume_payload()
    resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert event_types == ["run.started", "message.completed", "run.completed"]
    assert seen["command"].resume["decisions"] == [{"type": "approve"}]
    assert seen["config"]["configurable"]["thread_id"] == payload["thread_id"]
    assert fake_store.completed == [payload["approval_id"]]
    completed = fake_rust.emitted_batches[-1]["events"][0]
    assert completed["payload"]["result"] == {
        "memory_candidates": [
            {
                "content": "审批通过后继续执行高风险工具。",
                "layer": "semantic",
            }
        ]
    }
    assert completed["payload"]["memory_candidates"] == [
        {"content": "审批通过后继续执行高风险工具。", "layer": "semantic"}
    ]


def test_resume_run_payload_treats_tool_cancel_as_cancelled(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore()

    class FakeAgent:
        def stream(self, _command, config=None):
            raise resume_executor.RunCancelled("run-1", "cancelled_before_tool")
            yield  # pragma: no cover

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        resume_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    payload = resume_payload()
    resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert event_types == ["run.started", "run.cancelled"]
    assert fake_store.completed == [payload["approval_id"]]
    cancelled = fake_rust.emitted_batches[-1]["events"][0]
    assert cancelled["payload"]["reason"] == "cancelled_before_tool"


def test_resume_run_payload_redacts_failure_before_reraising(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore()

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        resume_executor,
        "create_platform_agent",
        lambda _snapshot: (_ for _ in ()).throw(
            RuntimeError("resume failed access_token=plain-secret")
        ),
    )

    payload = resume_payload()

    with pytest.raises(RuntimeError) as exc_info:
        resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    assert "plain-secret" not in str(exc_info.value)
    assert "access_token=[REDACTED]" in str(exc_info.value)
    assert fake_store.failed == [payload["approval_id"]]
    failed = fake_rust.emitted_batches[-1]["events"][0]
    assert failed["type"] == "run.failed"
    assert "plain-secret" not in failed["payload"]["error"]
    assert "access_token=[REDACTED]" in failed["payload"]["error"]


def test_resume_run_payload_rejects_desktop_runtime_before_started(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore()

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        resume_executor,
        "create_platform_agent",
        lambda _snapshot: (_ for _ in ()).throw(
            AssertionError("desktop runtime must not create Python agent")
        ),
    )

    payload = resume_payload()
    payload["run_config_snapshot"] = {"runtime": {"kind": "biwork_cli"}}

    with pytest.raises(RuntimeError, match="runtime.kind=biwork_cli"):
        resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert event_types == ["run.failed"]
    failed = fake_rust.emitted_batches[-1]["events"][0]
    assert failed["payload"]["run_id"] == payload["run_id"]
    assert failed["payload"]["approval_id"] == payload["approval_id"]
    assert "runtime.kind=biwork_cli" in failed["payload"]["error"]
    assert fake_store.failed == [payload["approval_id"]]


def test_resume_run_payload_rejects_invalid_snapshot_before_started(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore()
    created_agent = False

    def create_agent(_snapshot):
        nonlocal created_agent
        created_agent = True
        raise AssertionError("invalid snapshot must not create Python agent")

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(resume_executor, "create_platform_agent", create_agent)

    payload = resume_payload()
    payload["run_config_snapshot"].pop("actor")

    with pytest.raises(RuntimeError, match="actor.user_id"):
        resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert event_types == ["run.failed"]
    assert created_agent is False
    assert fake_store.failed == [payload["approval_id"]]


def test_resume_run_payload_ignores_duplicate_approval(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore(acquired=False)
    created_agent = False

    def create_agent(_snapshot):
        nonlocal created_agent
        created_agent = True
        raise AssertionError("duplicate resume must not create an agent")

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "create_platform_agent", create_agent)

    payload = resume_payload()
    resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    assert created_agent is False
    assert fake_rust.emitted_batches == []


def test_human_approval_resume_executes_high_risk_tool_once(monkeypatch) -> None:
    fake_rust = FakeRust()
    fake_store = FakeIdempotencyStore()
    calls = {"dangerous_tool": 0}

    class FakeAgent:
        def stream(self, command, config=None):
            calls["dangerous_tool"] += 1
            assert command.resume["decisions"] == [{"type": "approve"}]
            assert config["configurable"]["thread_id"]
            yield {
                "type": "tool.call.completed",
                "payload": {
                    "tool_call_id": "tool-call-1",
                    "tool_name": "local_exec",
                    "status": "completed",
                },
            }

    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: fake_store)
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        resume_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    payload = resume_payload()
    resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    duplicate_store = FakeIdempotencyStore(acquired=False)
    monkeypatch.setattr(
        resume_executor, "ResumeIdempotencyStore", lambda: duplicate_store
    )
    resume_executor.resume_run_payload("worker-task", payload["run_id"], payload)

    assert calls["dangerous_tool"] == 1
    assert fake_store.completed == [payload["approval_id"]]
    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert event_types == ["run.started", "tool.call.completed", "run.completed"]


def test_resume_idempotency_allows_failed_retry_but_blocks_completed(
    monkeypatch,
) -> None:
    class FakeRedis:
        def __init__(self) -> None:
            self.values: dict[str, str] = {}

        def eval(self, _script, _key_count, key, _ttl):
            current = self.values.get(key)
            if current is None or current == "failed":
                self.values[key] = "running"
                return 1
            return 0

        def set(self, key, value, ex=None):  # noqa: ARG002
            self.values[key] = value
            return True

    fake_redis = FakeRedis()
    monkeypatch.setattr(
        "bibi_work_agent.runtime.resume_idempotency._redis_client",
        lambda: fake_redis,
    )
    store = ResumeIdempotencyStore(ttl_sec=60)
    approval_id = str(uuid4())

    assert store.acquire(approval_id).acquired is True
    store.mark_failed(approval_id)
    assert store.acquire(approval_id).acquired is True
    store.mark_completed(approval_id)
    assert store.acquire(approval_id).acquired is False
