from __future__ import annotations

from uuid import uuid4

from langchain_core.messages import AIMessage, ToolMessage

from bibi_work_agent.runtime.event_normalizer import AgentEventNormalizer
from bibi_work_agent.runtime import run_executor


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


def test_call_agent_stream_requests_message_and_value_modes() -> None:
    seen: dict = {}

    class FakeAgent:
        def stream(self, input_payload, config=None, stream_mode=None):
            seen["input"] = input_payload
            seen["config"] = config
            seen["stream_mode"] = stream_mode
            yield {"type": "message.completed", "payload": {"content": "done"}}

    events = list(
        run_executor.call_agent_stream(
            FakeAgent(),
            {"messages": []},
            {"configurable": {"thread_id": "thread-1"}},
        )
    )

    assert seen["input"] == {"messages": []}
    assert seen["config"] == {"configurable": {"thread_id": "thread-1"}}
    assert seen["stream_mode"] == ["messages", "values"]
    assert events[0]["type"] == "message.completed"


def test_completed_message_payload_contains_frontend_content() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    event = normalizer.completed_message(
        {
            "message": "最终回答",
            "memory_candidates": [{"content": "偏好中文回复。"}],
        }
    )

    assert event["payload"]["content"] == "最终回答"
    assert event["payload"]["result"] == {
        "message": "最终回答",
        "memory_candidates": [{"content": "偏好中文回复。"}],
    }


def test_explicit_completed_message_payload_gets_content_from_result() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    events = normalizer.normalize(
        {
            "type": "message.completed",
            "payload": {
                "result": {
                    "messages": [
                        {"content": "用户问题"},
                        {"content": "助手回答"},
                    ]
                }
            },
        }
    )

    assert events[0]["payload"]["content"] == "助手回答"
    assert events[0]["payload"]["result"]["messages"][1]["content"] == "助手回答"


def test_values_stream_state_does_not_duplicate_messages() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    events = normalizer.normalize(
        (
            "values",
            {
                "messages": [{"content": "already emitted by messages stream"}],
                "todos": [{"id": "todo-1", "content": "check"}],
            },
        )
    )

    assert [event["type"] for event in events] == ["task.updated"]


def test_tool_message_stream_projects_tool_event_not_chat_delta() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    events = normalizer.normalize(
        (
            "messages",
            (
                ToolMessage(
                    content="['/local/main/', '/local/main/readme.md']",
                    name="ls",
                    tool_call_id="call-1",
                ),
                {"langgraph_node": "tools"},
            ),
        )
    )

    assert [event["type"] for event in events] == ["tool.call.completed"]
    payload = events[0]["payload"]
    assert payload["tool_name"] == "ls"
    assert payload["tool_call_id"] == "call-1"
    assert payload["views"][0]["kind"] == "table"
    assert payload["views"][0]["rows_preview"] == [
        {"path": "/local/main/", "type": "directory"},
        {"path": "/local/main/readme.md", "type": "file"},
    ]


def test_deepagents_tool_call_start_and_nameless_result_are_joined() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    started = normalizer.normalize(
        (
            "messages",
            (
                AIMessage(
                    content="",
                    tool_calls=[
                        {
                            "id": "call-todos",
                            "name": "write_todos",
                            "args": {
                                "todos": [
                                    {
                                        "content": "检查内置工具渲染",
                                        "status": "in_progress",
                                    }
                                ]
                            },
                        }
                    ],
                ),
                {"langgraph_node": "model"},
            ),
        )
    )

    completed = normalizer.normalize(
        (
            "messages",
            (
                ToolMessage(
                    content=(
                        "Updated todo list to [{'content': '检查内置工具渲染', "
                        "'status': 'in_progress'}]"
                    ),
                    tool_call_id="call-todos",
                ),
                {"langgraph_node": "tools"},
            ),
        )
    )

    assert [event["type"] for event in started] == ["tool.call.started"]
    assert started[0]["payload"]["tool_name"] == "write_todos"
    assert "todos" in started[0]["payload"]["input_summary"]
    assert "in_progress" in started[0]["payload"]["input_summary"]

    assert [event["type"] for event in completed] == ["tool.call.completed"]
    payload = completed[0]["payload"]
    assert payload["tool_name"] == "write_todos"
    assert payload["input_summary"] == started[0]["payload"]["input_summary"]
    assert payload["views"][0]["kind"] == "table"
    assert payload["views"][0]["rows_preview"] == [
        {"content": "检查内置工具渲染", "status": "in_progress"}
    ]


def test_deepagents_task_result_uses_cached_tool_name() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    normalizer.normalize(
        (
            "messages",
            (
                AIMessage(
                    content="",
                    tool_calls=[
                        {
                            "id": "call-task",
                            "name": "task",
                            "args": {
                                "description": "检查仓库结构",
                                "subagent_type": "general-purpose",
                            },
                        }
                    ],
                ),
                {},
            ),
        )
    )
    events = normalizer.normalize(
        (
            "messages",
            (
                ToolMessage(
                    content="仓库包含前端、后端和文档目录。",
                    tool_call_id="call-task",
                ),
                {},
            ),
        )
    )

    assert [event["type"] for event in events] == ["tool.call.completed"]
    assert events[0]["payload"]["tool_name"] == "task"
    assert events[0]["payload"]["views"][0]["kind"] == "markdown"


def test_deepagents_grep_result_projects_table_view() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    events = normalizer.normalize(
        (
            "messages",
            (
                ToolMessage(
                    content="/local/main/app.py:\n  3: import os\n  8: os.getcwd()",
                    name="grep",
                    tool_call_id="call-grep",
                ),
                {"langgraph_node": "tools"},
            ),
        )
    )

    payload = events[0]["payload"]
    assert payload["tool_name"] == "grep"
    assert payload["views"][0]["kind"] == "table"
    assert payload["views"][0]["rows_preview"] == [
        {"path": "/local/main/app.py", "line": 3, "text": "import os"},
        {"path": "/local/main/app.py", "line": 8, "text": "os.getcwd()"},
    ]


def test_non_deepagents_tool_message_is_left_for_platform_wrapper_events() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    events = normalizer.normalize(
        (
            "messages",
            (
                ToolMessage(
                    content='{"rows": []}',
                    name="query_sales",
                    tool_call_id="call-query-sales",
                ),
                {"langgraph_node": "tools"},
            ),
        )
    )

    assert events == []


def test_run_deepagent_stops_when_cancelled_before_agent_creation(monkeypatch) -> None:
    created_agent = False

    def fail_create_agent(_snapshot):
        nonlocal created_agent
        created_agent = True
        raise AssertionError("agent should not be created for a cancelled run")

    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: True)
    monkeypatch.setattr(run_executor, "create_platform_agent", fail_create_agent)

    emitter = FakeEmitter()
    cancelled = run_executor.run_deepagent(
        {
            "run_id": str(uuid4()),
            "trace_id": "trace",
            "input": {},
            "run_config_snapshot": {},
        },
        emitter,
    )

    assert cancelled is True
    assert created_agent is False
    assert emitter.events[0]["type"] == "run.cancelled"


def test_run_deepagent_collects_memory_candidates_from_agent_events(
    monkeypatch,
) -> None:
    class FakeAgent:
        def stream(self, _input):
            yield {
                "type": "message.completed",
                "payload": {
                    "result": {
                        "memory_candidates": [
                            {
                                "content": "销售额分析默认使用净收入。",
                                "layer": "semantic",
                            }
                        ]
                    }
                },
            }

    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        run_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    emitter = FakeEmitter()
    collector = run_executor.MemoryCandidateCollector()
    cancelled = run_executor.run_deepagent(
        {
            "run_id": str(uuid4()),
            "trace_id": "trace",
            "input": {},
            "run_config_snapshot": {},
        },
        emitter,
        memory_candidates=collector,
    )

    assert cancelled is False
    assert emitter.events[0]["type"] == "message.completed"
    assert collector.candidates() == [
        {"content": "销售额分析默认使用净收入。", "layer": "semantic"}
    ]


def test_run_deepagent_stops_on_langgraph_interrupt(monkeypatch) -> None:
    class FakeAgent:
        def stream(self, _input):
            yield {"__interrupt__": [{"value": {"approval_id": "approval-1"}}]}

    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        run_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    emitter = FakeEmitter()
    waiting = run_executor.run_deepagent(
        {
            "run_id": str(uuid4()),
            "trace_id": "trace",
            "input": {},
            "run_config_snapshot": {},
        },
        emitter,
    )

    assert waiting is True
    assert [event["type"] for event in emitter.events] == ["interrupt.requested"]


def test_execute_run_payload_emits_completed_memory_candidates(monkeypatch) -> None:
    class FakeAgent:
        def stream(self, _input):
            yield {
                "type": "message.completed",
                "payload": {
                    "result": {
                        "memory_candidates": [
                            {
                                "content": "用户偏好先看销售额同比变化。",
                                "visibility": "private",
                            }
                        ]
                    }
                },
            }

    fake_rust = FakeRust()
    monkeypatch.setattr(run_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        run_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    run_id = str(uuid4())
    run_executor.execute_run_payload(
        "worker-task",
        {
            "tenant_id": str(uuid4()),
            "conversation_id": str(uuid4()),
            "run_id": run_id,
            "trace_id": "trace",
            "input": {},
            "run_config_snapshot": {},
        },
    )

    completed = fake_rust.emitted_batches[-1]["events"][0]
    assert completed["type"] == "run.completed"
    assert completed["payload"]["memory_candidates"] == [
        {"content": "用户偏好先看销售额同比变化。", "visibility": "private"}
    ]


def test_execute_run_payload_waits_when_tool_requires_approval(monkeypatch) -> None:
    class FakeAgent:
        def stream(self, _input):
            raise run_executor.ToolRequiresApproval("approval-1")
            yield  # pragma: no cover

    fake_rust = FakeRust()
    monkeypatch.setattr(run_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        run_executor, "create_platform_agent", lambda _snapshot: FakeAgent()
    )

    run_id = str(uuid4())
    run_executor.execute_run_payload(
        "worker-task",
        {
            "tenant_id": str(uuid4()),
            "conversation_id": str(uuid4()),
            "run_id": run_id,
            "trace_id": "trace",
            "input": {},
            "run_config_snapshot": {},
        },
    )

    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert "approval.requested" in event_types
    assert "run.failed" not in event_types
    assert "run.completed" not in event_types

    approval_event = fake_rust.emitted_batches[-1]["events"][0]
    assert approval_event["payload"]["approval_id"] == "approval-1"
    assert approval_event["payload"]["status"] == "waiting_approval"
