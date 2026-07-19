from __future__ import annotations

from bibi_work_agent.runtime.event_normalizer import (
    AgentEventNormalizer,
    file_tool_target_payload,
)


def test_file_tool_target_preserves_virtual_storage_kind() -> None:
    assert file_tool_target_payload("read_file", {"path": "/artifacts/report.txt"})[
        "target"
    ] == {"kind": "artifact", "path": "/artifacts/report.txt"}
    assert file_tool_target_payload("read_file", {"path": "/scratch/report.txt"})[
        "target"
    ] == {"kind": "scratch_file", "path": "/scratch/report.txt"}


def test_tool_call_chunks_emit_argument_delta_events() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    first = normalizer.normalize(
        (
            "messages",
            (
                {
                    "content": "",
                    "tool_call_chunks": [
                        {
                            "id": "call-write",
                            "name": "write_file",
                            "args": '{"path": "/local/main/report.md", ',
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
    second = normalizer.normalize(
        (
            "messages",
            (
                {
                    "content": "",
                    "tool_call_chunks": [
                        {
                            "id": "call-write",
                            "name": "write_file",
                            "args": '"content": "# draft"}',
                        }
                    ],
                },
                {"langgraph_node": "model"},
            ),
        )
    )

    assert [event["type"] for event in first] == ["tool.call.delta"]
    assert [event["type"] for event in second] == ["tool.call.delta"]
    payload = second[0]["payload"]
    assert payload["tool_call_id"] == "call-write"
    assert payload["tool_name"] == "write_file"
    assert payload["subagent_id"] == "sub-1"
    assert payload["subagent_name"] == "writer"
    assert payload["parent_tool_call_id"] == "call-task"
    assert payload["target"]["path"] == "/local/main/report.md"
    assert payload["arguments_text"].endswith('"content": "# draft"}')
