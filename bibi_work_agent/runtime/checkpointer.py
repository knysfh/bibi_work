from __future__ import annotations

import base64
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any

from langgraph.checkpoint.base import (
    WRITES_IDX_MAP,
    BaseCheckpointSaver,
    CheckpointTuple,
    get_checkpoint_id,
    get_checkpoint_metadata,
)

from bibi_work_agent.settings import settings


_TYPED_SERDE_MARKER = "__bibi_langgraph_serde_v1__"


@dataclass(frozen=True)
class CheckpointRecord:
    tenant_id: str
    thread_id: str
    checkpoint_ns: str
    checkpoint_id: str
    parent_checkpoint_id: str | None
    type: str
    checkpoint: dict[str, Any]
    metadata: dict[str, Any]


class PlatformCheckpointer(BaseCheckpointSaver[int]):
    """Postgres-backed checkpoint store used by the platform runtime.

    The existing small persistence primitives stay available for platform
    tests and diagnostics. The BaseCheckpointSaver methods map LangGraph's
    checkpoint protocol onto the same tables using typed serializer envelopes
    for values that are not plain JSON.
    """

    def __init__(
        self,
        *,
        thread_id: str | None,
        tenant_id: str | None = None,
        database_url: str | None = None,
        serde: Any | None = None,
    ) -> None:
        super().__init__(serde=serde)
        self.thread_id = thread_id
        self.tenant_id = tenant_id
        self.database_url = database_url or settings.database_url

    def put_checkpoint(
        self,
        *,
        tenant_id: str,
        checkpoint_id: str,
        checkpoint: dict[str, Any],
        metadata: dict[str, Any] | None = None,
        checkpoint_ns: str = "",
        parent_checkpoint_id: str | None = None,
        checkpoint_type: str = "json",
        thread_id: str | None = None,
    ) -> CheckpointRecord:
        thread_id = self._thread_id(thread_id)
        with self._connect() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """
                    INSERT INTO agent_checkpoints (
                        tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                        parent_checkpoint_id, type, checkpoint_json, metadata_json
                    )
                    VALUES (%s, %s, %s, %s, %s, %s, %s, %s)
                    ON CONFLICT (tenant_id, thread_id, checkpoint_ns, checkpoint_id)
                    DO UPDATE SET
                        parent_checkpoint_id = EXCLUDED.parent_checkpoint_id,
                        type = EXCLUDED.type,
                        checkpoint_json = EXCLUDED.checkpoint_json,
                        metadata_json = EXCLUDED.metadata_json
                    RETURNING tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                              parent_checkpoint_id, type, checkpoint_json, metadata_json
                    """,
                    (
                        tenant_id,
                        thread_id,
                        checkpoint_ns,
                        checkpoint_id,
                        parent_checkpoint_id,
                        checkpoint_type,
                        self._jsonb(checkpoint),
                        self._jsonb(metadata or {}),
                    ),
                )
                row = cur.fetchone()
            conn.commit()
        return self._record(row)

    def get_checkpoint(
        self,
        *,
        tenant_id: str,
        checkpoint_id: str | None = None,
        checkpoint_ns: str = "",
        thread_id: str | None = None,
    ) -> CheckpointRecord | None:
        thread_id = self._thread_id(thread_id)
        with self._connect() as conn:
            with conn.cursor() as cur:
                if checkpoint_id is None:
                    cur.execute(
                        """
                        SELECT tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                               parent_checkpoint_id, type, checkpoint_json, metadata_json
                        FROM agent_checkpoints
                        WHERE tenant_id = %s AND thread_id = %s AND checkpoint_ns = %s
                        ORDER BY created_at DESC, checkpoint_id DESC
                        LIMIT 1
                        """,
                        (tenant_id, thread_id, checkpoint_ns),
                    )
                else:
                    cur.execute(
                        """
                        SELECT tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                               parent_checkpoint_id, type, checkpoint_json, metadata_json
                        FROM agent_checkpoints
                        WHERE tenant_id = %s
                          AND thread_id = %s
                          AND checkpoint_ns = %s
                          AND checkpoint_id = %s
                        """,
                        (tenant_id, thread_id, checkpoint_ns, checkpoint_id),
                    )
                row = cur.fetchone()
        return None if row is None else self._record(row)

    def put_writes(self, *args: Any, **kwargs: Any) -> None:
        if args and isinstance(args[0], dict):
            self._put_langgraph_writes(*args, **kwargs)
            return
        self._put_platform_writes(**kwargs)

    def _put_platform_writes(
        self,
        *,
        tenant_id: str,
        checkpoint_id: str,
        task_id: str,
        writes: Iterable[dict[str, Any]],
        checkpoint_ns: str = "",
        thread_id: str | None = None,
    ) -> None:
        thread_id = self._thread_id(thread_id)
        with self._connect() as conn:
            with conn.cursor() as cur:
                for idx, write in enumerate(writes):
                    cur.execute(
                        """
                        INSERT INTO agent_checkpoint_writes (
                            tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                            task_id, idx, channel, type, value_json
                        )
                        VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
                        ON CONFLICT (
                            tenant_id, thread_id, checkpoint_ns, checkpoint_id, task_id, idx
                        )
                        DO UPDATE SET
                            channel = EXCLUDED.channel,
                            type = EXCLUDED.type,
                            value_json = EXCLUDED.value_json
                        """,
                        (
                            tenant_id,
                            thread_id,
                            checkpoint_ns,
                            checkpoint_id,
                            task_id,
                            idx,
                            write.get("channel", ""),
                            write.get("type", "json"),
                            self._jsonb(write.get("value", {})),
                        ),
                    )
            conn.commit()

    def list_writes(
        self,
        *,
        tenant_id: str,
        checkpoint_id: str,
        checkpoint_ns: str = "",
        thread_id: str | None = None,
    ) -> list[dict[str, Any]]:
        thread_id = self._thread_id(thread_id)
        with self._connect() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """
                    SELECT task_id, idx, channel, type, value_json
                    FROM agent_checkpoint_writes
                    WHERE tenant_id = %s
                      AND thread_id = %s
                      AND checkpoint_ns = %s
                      AND checkpoint_id = %s
                    ORDER BY task_id ASC, idx ASC
                    """,
                    (tenant_id, thread_id, checkpoint_ns, checkpoint_id),
                )
                rows = cur.fetchall()
        return [
            {
                "task_id": row[0],
                "idx": row[1],
                "channel": row[2],
                "type": row[3],
                "value": row[4],
            }
            for row in rows
        ]

    def get_tuple(self, config: dict[str, Any]) -> CheckpointTuple | None:
        record = self.get_checkpoint(
            tenant_id=self._tenant_id(),
            checkpoint_id=get_checkpoint_id(config),
            checkpoint_ns=config["configurable"].get("checkpoint_ns", ""),
            thread_id=config["configurable"].get("thread_id"),
        )
        if record is None:
            return None
        return self._checkpoint_tuple(record)

    def list(
        self,
        config: dict[str, Any] | None,
        *,
        filter: dict[str, Any] | None = None,
        before: dict[str, Any] | None = None,
        limit: int | None = None,
    ):
        tenant_id = self._tenant_id()
        thread_id = config["configurable"].get("thread_id") if config else None
        checkpoint_ns = config["configurable"].get("checkpoint_ns") if config else None
        checkpoint_id = get_checkpoint_id(config) if config else None
        before_checkpoint_id = get_checkpoint_id(before) if before else None

        clauses = ["tenant_id = %s"]
        params: list[Any] = [tenant_id]
        if thread_id is not None:
            clauses.append("thread_id = %s")
            params.append(thread_id)
        if checkpoint_ns is not None:
            clauses.append("checkpoint_ns = %s")
            params.append(checkpoint_ns)
        if checkpoint_id is not None:
            clauses.append("checkpoint_id = %s")
            params.append(checkpoint_id)
        if before_checkpoint_id is not None:
            clauses.append("checkpoint_id < %s")
            params.append(before_checkpoint_id)

        sql = f"""
            SELECT tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                   parent_checkpoint_id, type, checkpoint_json, metadata_json
            FROM agent_checkpoints
            WHERE {" AND ".join(clauses)}
            ORDER BY created_at DESC, checkpoint_id DESC
        """
        if limit is not None:
            sql += " LIMIT %s"
            params.append(limit)

        with self._connect() as conn:
            with conn.cursor() as cur:
                cur.execute(sql, tuple(params))
                rows = cur.fetchall()

        for row in rows:
            record = self._record(row)
            metadata = self._unpack_if_typed(record.metadata)
            if filter and not all(
                metadata.get(key) == value for key, value in filter.items()
            ):
                continue
            yield self._checkpoint_tuple(record)

    def put(
        self,
        config: dict[str, Any],
        checkpoint: dict[str, Any],
        metadata: dict[str, Any],
        new_versions: dict[str, Any],
    ) -> dict[str, Any]:
        del new_versions
        configurable = config["configurable"]
        thread_id = configurable["thread_id"]
        checkpoint_ns = configurable.get("checkpoint_ns", "")
        checkpoint_id = str(checkpoint["id"])
        self.put_checkpoint(
            tenant_id=self._tenant_id(),
            thread_id=thread_id,
            checkpoint_ns=checkpoint_ns,
            checkpoint_id=checkpoint_id,
            parent_checkpoint_id=get_checkpoint_id(config),
            checkpoint_type="langgraph-serde",
            checkpoint=self._pack_typed(checkpoint),
            metadata=self._pack_typed(get_checkpoint_metadata(config, metadata)),
        )
        return {
            "configurable": {
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint_id,
            }
        }

    def delete_thread(self, thread_id: str) -> None:
        with self._connect() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """
                    DELETE FROM agent_checkpoint_writes
                    WHERE tenant_id = %s AND thread_id = %s
                    """,
                    (self._tenant_id(), thread_id),
                )
                cur.execute(
                    """
                    DELETE FROM agent_checkpoints
                    WHERE tenant_id = %s AND thread_id = %s
                    """,
                    (self._tenant_id(), thread_id),
                )
            conn.commit()

    def _put_langgraph_writes(
        self,
        config: dict[str, Any],
        writes: Iterable[tuple[str, Any]],
        task_id: str,
        task_path: str = "",
    ) -> None:
        del task_path
        configurable = config["configurable"]
        thread_id = configurable["thread_id"]
        checkpoint_ns = configurable.get("checkpoint_ns", "")
        checkpoint_id = configurable["checkpoint_id"]
        with self._connect() as conn:
            with conn.cursor() as cur:
                for idx, (channel, value) in enumerate(writes):
                    write_idx = WRITES_IDX_MAP.get(channel, idx)
                    if write_idx < 0:
                        cur.execute(
                            """
                            SELECT 1
                            FROM agent_checkpoint_writes
                            WHERE tenant_id = %s
                              AND thread_id = %s
                              AND checkpoint_ns = %s
                              AND checkpoint_id = %s
                              AND task_id = %s
                              AND idx = %s
                            """,
                            (
                                self._tenant_id(),
                                thread_id,
                                checkpoint_ns,
                                checkpoint_id,
                                task_id,
                                write_idx,
                            ),
                        )
                        if cur.fetchone() is not None:
                            continue
                    cur.execute(
                        """
                        INSERT INTO agent_checkpoint_writes (
                            tenant_id, thread_id, checkpoint_ns, checkpoint_id,
                            task_id, idx, channel, type, value_json
                        )
                        VALUES (%s, %s, %s, %s, %s, %s, %s, 'langgraph-serde', %s)
                        ON CONFLICT (
                            tenant_id, thread_id, checkpoint_ns, checkpoint_id, task_id, idx
                        )
                        DO UPDATE SET
                            channel = EXCLUDED.channel,
                            type = EXCLUDED.type,
                            value_json = EXCLUDED.value_json
                        """,
                        (
                            self._tenant_id(),
                            thread_id,
                            checkpoint_ns,
                            checkpoint_id,
                            task_id,
                            write_idx,
                            channel,
                            self._jsonb(self._pack_typed(value)),
                        ),
                    )
            conn.commit()

    def _checkpoint_tuple(self, record: CheckpointRecord) -> CheckpointTuple:
        config = {
            "configurable": {
                "thread_id": record.thread_id,
                "checkpoint_ns": record.checkpoint_ns,
                "checkpoint_id": record.checkpoint_id,
            }
        }
        parent_config = (
            {
                "configurable": {
                    "thread_id": record.thread_id,
                    "checkpoint_ns": record.checkpoint_ns,
                    "checkpoint_id": record.parent_checkpoint_id,
                }
            }
            if record.parent_checkpoint_id
            else None
        )
        return CheckpointTuple(
            config=config,
            checkpoint=self._unpack_if_typed(record.checkpoint),
            metadata=self._unpack_if_typed(record.metadata),
            parent_config=parent_config,
            pending_writes=[
                (
                    write["task_id"],
                    write["channel"],
                    self._unpack_if_typed(write["value"]),
                )
                for write in self.list_writes(
                    tenant_id=record.tenant_id,
                    thread_id=record.thread_id,
                    checkpoint_ns=record.checkpoint_ns,
                    checkpoint_id=record.checkpoint_id,
                )
            ],
        )

    def _tenant_id(self) -> str:
        if not self.tenant_id:
            raise ValueError(
                "tenant_id is required for LangGraph checkpoint operations"
            )
        return self.tenant_id

    def _thread_id(self, thread_id: str | None) -> str:
        thread_id = thread_id or self.thread_id
        if not thread_id:
            raise ValueError("thread_id is required")
        return thread_id

    def _connect(self):
        try:
            import psycopg
        except Exception as exc:  # noqa: BLE001
            raise RuntimeError("psycopg is required for PlatformCheckpointer") from exc

        return psycopg.connect(self.database_url)

    @staticmethod
    def _jsonb(value: dict[str, Any]) -> Any:
        try:
            from psycopg.types.json import Jsonb
        except Exception as exc:  # noqa: BLE001
            raise RuntimeError("psycopg Jsonb support is required") from exc
        return Jsonb(value)

    def _pack_typed(self, value: Any) -> dict[str, Any]:
        type_name, data = self.serde.dumps_typed(value)
        return {
            _TYPED_SERDE_MARKER: True,
            "type": type_name,
            "data": base64.b64encode(data).decode("ascii"),
        }

    def _unpack_if_typed(self, value: Any) -> Any:
        if isinstance(value, dict) and value.get(_TYPED_SERDE_MARKER) is True:
            return self.serde.loads_typed(
                (value["type"], base64.b64decode(value["data"].encode("ascii")))
            )
        return value

    @staticmethod
    def _record(row: Any) -> CheckpointRecord:
        return CheckpointRecord(
            tenant_id=str(row[0]),
            thread_id=row[1],
            checkpoint_ns=row[2],
            checkpoint_id=row[3],
            parent_checkpoint_id=row[4],
            type=row[5],
            checkpoint=row[6],
            metadata=row[7],
        )
