from __future__ import annotations

from bibi_work_agent.runtime.event_normalizer import AgentEventNormalizer


def test_streamed_reasoning_tags_are_hidden_across_chunk_boundaries() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    chunks = [
        "<thi",
        "nk>private reasoning",
        " continues</think>\n\n",
        "visible answer",
    ]
    events = [
        event
        for chunk in chunks
        for event in normalizer.normalize(
            ("messages", ({"content": chunk}, {"langgraph_node": "model"}))
        )
    ]
    completed = normalizer.pending_completed_message()

    assert [event["type"] for event in events] == ["message.delta"]
    assert events[0]["payload"]["content"] == "visible answer"
    assert completed is not None
    assert completed["payload"]["content"] == "visible answer"
    assert "private reasoning" not in str(completed["payload"])


def test_completed_message_redacts_reasoning_from_content_and_result() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    completed = normalizer.completed_message(
        {
            "message": "<think>hidden</think>\n\nfinal answer",
            "messages": [
                {"role": "assistant", "content": "<analysis>secret</analysis>final answer"}
            ],
        }
    )

    assert completed["payload"]["content"] == "final answer"
    assert completed["payload"]["result"]["message"] == "final answer"
    assert completed["payload"]["result"]["messages"][0]["content"] == "final answer"
    assert "hidden" not in str(completed["payload"])
    assert "secret" not in str(completed["payload"])


def test_explicit_message_events_hide_reasoning_and_skip_empty_delta() -> None:
    normalizer = AgentEventNormalizer(run_id="run-1", trace_id="trace")

    hidden = normalizer.normalize(
        {"type": "message.delta", "payload": {"content": "<think>hidden"}}
    )
    visible = normalizer.normalize(
        {
            "type": "message.delta",
            "payload": {"content": "</think>\n\npublic"},
        }
    )
    completed = normalizer.normalize(
        {
            "type": "message.completed",
            "payload": {
                "result": {"message": "<reasoning>hidden</reasoning>public"}
            },
        }
    )

    assert hidden == []
    assert visible[0]["payload"]["content"] == "public"
    assert completed[0]["payload"]["content"] == "public"
    assert completed[0]["payload"]["result"]["message"] == "public"
