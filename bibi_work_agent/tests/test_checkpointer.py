from __future__ import annotations

from uuid import uuid4

import pytest

from bibi_work_agent.runtime.checkpointer import PlatformCheckpointer
from bibi_work_agent.settings import settings


def connect_or_skip():
    psycopg = pytest.importorskip("psycopg")
    try:
        return psycopg.connect(settings.database_url)
    except Exception as exc:  # noqa: BLE001
        pytest.skip(f"Postgres is not available: {exc}")


def test_platform_checkpointer_round_trips_checkpoint_and_writes() -> None:
    tenant_slug = f"agent-test-{uuid4()}"
    thread_id = f"thread-{uuid4()}"
    with connect_or_skip() as conn:
        with conn.cursor() as cur:
            cur.execute(
                """
                INSERT INTO tenants (name, slug)
                VALUES (%s, %s)
                RETURNING id
                """,
                ("agent test", tenant_slug),
            )
            tenant_id = str(cur.fetchone()[0])
        conn.commit()

    try:
        checkpointer = PlatformCheckpointer(thread_id=thread_id)
        first = checkpointer.put_checkpoint(
            tenant_id=tenant_id,
            checkpoint_id="cp-1",
            checkpoint={"step": 1},
            metadata={"source": "test"},
        )
        updated = checkpointer.put_checkpoint(
            tenant_id=tenant_id,
            checkpoint_id="cp-1",
            checkpoint={"step": 2},
            metadata={"source": "retry"},
        )
        latest = checkpointer.get_checkpoint(tenant_id=tenant_id)

        assert first.checkpoint == {"step": 1}
        assert updated.checkpoint == {"step": 2}
        assert latest is not None
        assert latest.checkpoint_id == "cp-1"
        assert latest.metadata == {"source": "retry"}

        checkpointer.put_writes(
            tenant_id=tenant_id,
            checkpoint_id="cp-1",
            task_id="task-a",
            writes=[
                {"channel": "messages", "type": "json", "value": {"content": "a"}},
                {"channel": "state", "type": "json", "value": {"done": False}},
            ],
        )
        checkpointer.put_writes(
            tenant_id=tenant_id,
            checkpoint_id="cp-1",
            task_id="task-a",
            writes=[
                {"channel": "messages", "type": "json", "value": {"content": "b"}},
            ],
        )
        writes = checkpointer.list_writes(tenant_id=tenant_id, checkpoint_id="cp-1")

        assert len(writes) == 2
        assert writes[0]["value"] == {"content": "b"}
        assert writes[1]["value"] == {"done": False}

        graph_checkpointer = PlatformCheckpointer(
            thread_id=thread_id, tenant_id=tenant_id
        )
        graph_config = {"configurable": {"thread_id": thread_id, "checkpoint_ns": ""}}
        saved_config = graph_checkpointer.put(
            graph_config,
            {
                "id": "lg-1",
                "channel_values": {"messages": [{"content": "hello"}]},
                "channel_versions": {"messages": 1},
                "versions_seen": {},
                "pending_sends": [],
            },
            {"source": "langgraph"},
            {"messages": 1},
        )
        graph_checkpointer.put_writes(
            saved_config,
            [("messages", {"content": "pending"})],
            "task-lg",
        )
        checkpoint_tuple = graph_checkpointer.get_tuple(saved_config)

        assert checkpoint_tuple is not None
        assert checkpoint_tuple.checkpoint["id"] == "lg-1"
        assert checkpoint_tuple.metadata["source"] == "langgraph"
        assert checkpoint_tuple.pending_writes == [
            ("task-lg", "messages", {"content": "pending"})
        ]
    finally:
        with connect_or_skip() as conn:
            with conn.cursor() as cur:
                cur.execute("DELETE FROM tenants WHERE slug = %s", (tenant_slug,))
            conn.commit()
