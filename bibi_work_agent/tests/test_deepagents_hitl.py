from __future__ import annotations

from uuid import uuid4

from deepagents import create_deep_agent
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from langgraph.checkpoint.memory import InMemorySaver

from bibi_work_agent.runtime import resume_executor, run_executor


class ToolCallingFakeModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, *, tool_choice=None, **kwargs):
        return self


class FakeEmitter:
    def __init__(self) -> None:
        self.events: list[dict] = []

    def emit(self, events: list[dict]) -> None:
        self.events.extend(events)


class FakeRust:
    def __init__(self) -> None:
        self.emitted_batches: list[dict] = []

    def emit_events(self, **kwargs):
        self.emitted_batches.append(kwargs)
        return {}


class FakeLease:
    acquired = True


class FakeIdempotencyStore:
    def __init__(self, *, acquired: bool) -> None:
        self.acquired = acquired
        self.completed: list[str] = []
        self.failed: list[str] = []

    def acquire(self, _approval_id: str):
        lease = FakeLease()
        lease.acquired = self.acquired
        return lease

    def mark_completed(self, approval_id: str) -> None:
        self.completed.append(approval_id)

    def mark_failed(self, approval_id: str) -> None:
        self.failed.append(approval_id)


def test_real_deepagents_hitl_resume_does_not_repeat_high_risk_tool(
    monkeypatch,
) -> None:
    calls: list[str] = []

    def dangerous_tool(value: str) -> str:
        """Execute a high risk test tool."""
        calls.append(value)
        return f"tool:{value}"

    agent = create_deep_agent(
        model=ToolCallingFakeModel(
            responses=[
                AIMessage(
                    content="",
                    tool_calls=[
                        {
                            "name": "dangerous_tool",
                            "args": {"value": "approved"},
                            "id": "call_1",
                        }
                    ],
                ),
                AIMessage(content="done"),
            ]
        ),
        tools=[dangerous_tool],
        interrupt_on={"dangerous_tool": True},
        checkpointer=InMemorySaver(),
    )

    run_id = str(uuid4())
    payload = {
        "tenant_id": str(uuid4()),
        "conversation_id": str(uuid4()),
        "run_id": run_id,
        "trace_id": "trace",
        "thread_id": f"thread-{run_id}",
        "input": {"messages": [{"role": "user", "content": "run dangerous tool"}]},
        "run_config_snapshot": {"agent": {"model": "fake"}},
    }

    monkeypatch.setattr(run_executor, "create_platform_agent", lambda _snapshot: agent)
    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: False)

    first_emitter = FakeEmitter()
    waiting = run_executor.run_deepagent(payload, first_emitter)

    assert waiting is True
    assert calls == []
    assert [event["type"] for event in first_emitter.events] == ["interrupt.requested"]

    fake_rust = FakeRust()
    first_store = FakeIdempotencyStore(acquired=True)
    approval_id = str(uuid4())
    resume_payload = {
        **payload,
        "approval_id": approval_id,
        "decision_payload": {"decision": "approved"},
    }
    monkeypatch.setattr(resume_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(resume_executor, "ResumeIdempotencyStore", lambda: first_store)
    monkeypatch.setattr(
        resume_executor, "create_platform_agent", lambda _snapshot: agent
    )
    monkeypatch.setattr(resume_executor, "is_run_cancelled", lambda _run_id: False)

    resume_executor.resume_run_payload("worker-task", run_id, resume_payload)

    duplicate_store = FakeIdempotencyStore(acquired=False)
    monkeypatch.setattr(
        resume_executor, "ResumeIdempotencyStore", lambda: duplicate_store
    )
    resume_executor.resume_run_payload("worker-task", run_id, resume_payload)

    assert calls == ["approved"]
    assert first_store.completed == [approval_id]
    emitted_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert emitted_types == ["run.started", "message.completed", "run.completed"]
