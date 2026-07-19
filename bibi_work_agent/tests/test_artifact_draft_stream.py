from __future__ import annotations

import sys
from types import ModuleType
from uuid import uuid4

protocol_module = ModuleType("deepagents.backends.protocol")


class _Result(dict):
    def __init__(self, **kwargs):
        super().__init__(**kwargs)

    def __getattr__(self, name):
        try:
            return self[name]
        except KeyError:
            return None


for _name in [
    "EditResult",
    "FileInfo",
    "GlobResult",
    "GrepMatch",
    "GrepResult",
    "LsResult",
    "ReadResult",
    "WriteResult",
]:
    setattr(protocol_module, _name, _Result)

sys.modules.setdefault("deepagents", ModuleType("deepagents"))
sys.modules.setdefault("deepagents.backends", ModuleType("deepagents.backends"))
sys.modules.setdefault("deepagents.backends.protocol", protocol_module)

class FakeRust:
    def __init__(self) -> None:
        self.emitted: list[dict] = []
        self.local_exec_payloads: list[dict] = []

    def emit_events(self, *, tenant_id, conversation_id, run_id, events):
        self.emitted.append(
            {
                "tenant_id": tenant_id,
                "conversation_id": conversation_id,
                "run_id": run_id,
                "events": events,
            }
        )

    def local_exec_request(self, payload: dict) -> dict:
        self.local_exec_payloads.append(payload)
        return {
            "status": "completed",
            "result": {
                "path": payload["virtual_path"],
                "revision": 1,
                "content_hash": "sha256:server",
            },
        }

    def file_write(self, payload: dict) -> dict:
        return {
            "revision": payload["expected_revision"] + 1,
            "content_hash": "sha256:server-artifact",
            "object_reference_id": "00000000-0000-0000-0000-000000000001",
        }


def test_local_write_emits_artifact_draft_events_before_completion() -> None:
    from bibi_work_agent.backends.platform_composite_backend import (
        PlatformCompositeBackend,
    )

    rust = FakeRust()
    run_id = str(uuid4())
    backend = PlatformCompositeBackend(
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        project_id=str(uuid4()),
        run_id=run_id,
        conversation_id=str(uuid4()),
        trace_id="trace-1",
        local_main_mount_id=str(uuid4()),
        rust=rust,
    )

    backend.write_text(
        "/local/main/report.md",
        "# 标题\n正文",
        expected_revision=0,
        operation="write_file",
    )

    started_batch = rust.emitted[0]["events"]
    delta_batch = rust.emitted[1]["events"]
    assert [event["type"] for event in started_batch] == ["artifact.draft.started"]
    assert [event["type"] for event in delta_batch] == ["artifact.draft.delta"]
    assert started_batch[0]["payload"]["path"] == "/local/main/report.md"
    assert started_batch[0]["payload"]["target"] == {
        "kind": "local_file",
        "path": "/local/main/report.md",
    }
    assert started_batch[0]["payload"]["renderer"] == "markdown"
    assert started_batch[0]["payload"]["truncated"] is False
    assert delta_batch[0]["payload"]["delta"] == "# 标题\n正文"
    assert delta_batch[0]["payload"]["target"] == {
        "kind": "local_file",
        "path": "/local/main/report.md",
    }

    completed = rust.emitted[2]["events"][0]
    assert completed["type"] == "artifact.draft.completed"
    assert completed["payload"]["run_id"] == run_id
    assert completed["payload"]["target"] == {
        "kind": "local_file",
        "path": "/local/main/report.md",
    }
    assert completed["payload"]["revision"] == 1
    assert completed["payload"]["content_hash"] == "sha256:server"


def test_artifact_write_emits_artifact_target() -> None:
    from bibi_work_agent.backends.platform_composite_backend import (
        PlatformCompositeBackend,
    )

    rust = FakeRust()
    backend = PlatformCompositeBackend(
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        project_id=str(uuid4()),
        run_id=str(uuid4()),
        conversation_id=str(uuid4()),
        trace_id="trace-1",
        rust=rust,
    )

    backend.write_text(
        "/artifacts/report.md",
        "# Artifact\nbody",
        expected_revision=0,
        operation="write_file",
    )

    started = rust.emitted[0]["events"][0]
    delta = rust.emitted[1]["events"][0]
    completed = rust.emitted[2]["events"][0]
    expected_target = {"kind": "artifact", "path": "/artifacts/report.md"}
    assert started["payload"]["target"] == expected_target
    assert delta["payload"]["target"] == expected_target
    assert completed["payload"]["target"] == expected_target


def test_edit_write_emits_previous_preview_for_diff() -> None:
    from bibi_work_agent.backends.platform_composite_backend import (
        PlatformCompositeBackend,
    )

    rust = FakeRust()
    backend = PlatformCompositeBackend(
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        project_id=str(uuid4()),
        run_id=str(uuid4()),
        conversation_id=str(uuid4()),
        trace_id="trace-1",
        local_main_mount_id=str(uuid4()),
        rust=rust,
    )

    backend.write_text(
        "/local/main/report.md",
        "new",
        expected_revision=1,
        operation="edit_file",
        previous_content="old",
    )

    started = rust.emitted[0]["events"][0]
    assert started["type"] == "artifact.draft.started"
    assert started["payload"]["previous_preview"] == "old"
    assert started["payload"]["previous_size_bytes"] == 3


def test_write_draft_events_keep_tool_call_context() -> None:
    from bibi_work_agent.backends.platform_composite_backend import (
        PlatformCompositeBackend,
    )
    from bibi_work_agent.runtime.event_normalizer import AgentEventNormalizer

    rust = FakeRust()
    run_id = str(uuid4())
    backend = PlatformCompositeBackend(
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        project_id=str(uuid4()),
        run_id=run_id,
        conversation_id=str(uuid4()),
        trace_id="trace-1",
        local_main_mount_id=str(uuid4()),
        rust=rust,
    )
    normalizer = AgentEventNormalizer(run_id=run_id, trace_id="trace-1")
    normalizer.normalize(
        (
            "messages",
            (
                {
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call-write",
                            "name": "write_file",
                            "args": {
                                "file_path": "/local/main/report.md",
                                "content": "# 标题",
                            },
                        }
                    ],
                },
                {
                    "langgraph_node": "model",
                    "subagent_id": "sub-1",
                    "subagent_name": "writer",
                    "parent_tool_call_id": "call-task",
                },
            ),
        )
    )

    backend.write_text(
        "/local/main/report.md",
        "# 标题",
        expected_revision=0,
        operation="write_file",
    )

    started = rust.emitted[0]["events"][0]
    completed = rust.emitted[-1]["events"][0]
    assert started["payload"]["tool_call_id"] == "call-write"
    assert started["payload"]["subagent_id"] == "sub-1"
    assert started["payload"]["subagent_name"] == "writer"
    assert started["payload"]["parent_tool_call_id"] == "call-task"
    assert completed["payload"]["tool_call_id"] == "call-write"
    assert completed["payload"]["subagent_id"] == "sub-1"
    assert completed["payload"]["subagent_name"] == "writer"
    assert completed["payload"]["parent_tool_call_id"] == "call-task"
