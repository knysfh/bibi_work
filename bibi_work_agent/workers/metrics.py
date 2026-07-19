from __future__ import annotations

import logging
from dataclasses import dataclass
from time import time
from typing import Any

from redis import Redis
from redis.exceptions import RedisError

from bibi_work_agent.settings import settings


logger = logging.getLogger(__name__)

QUEUE_NAME = "celery"
STARTED_TOTAL_KEY = "bibi:celery:tasks_started_total"
SUCCEEDED_TOTAL_KEY = "bibi:celery:tasks_succeeded_total"
FAILED_TOTAL_KEY = "bibi:celery:worker_task_failures_total"
ACTIVE_TASKS_KEY = "bibi:celery:active_tasks"


@dataclass(frozen=True)
class CeleryMetricsSnapshot:
    queue_depth: int
    active_tasks: int
    tasks_started_total: int
    tasks_succeeded_total: int
    worker_task_failures_total: int


def _redis() -> Redis:
    return Redis.from_url(settings.celery_broker_url, decode_responses=True)


def _safe_update(operation: str, callback) -> None:
    client = _redis()
    try:
        callback(client)
    except RedisError as error:
        logger.warning("Celery metric %s failed: %s", operation, error)
    finally:
        client.close()


def record_task_started(task_id: str) -> None:
    def update(client: Redis) -> None:
        pipeline = client.pipeline(transaction=False)
        pipeline.incr(STARTED_TOTAL_KEY)
        pipeline.hset(ACTIVE_TASKS_KEY, task_id, str(time()))
        pipeline.execute()

    _safe_update("start", update)


def record_task_succeeded(task_id: str) -> None:
    def update(client: Redis) -> None:
        pipeline = client.pipeline(transaction=False)
        pipeline.incr(SUCCEEDED_TOTAL_KEY)
        pipeline.hdel(ACTIVE_TASKS_KEY, task_id)
        pipeline.execute()

    _safe_update("success", update)


def record_task_failed(task_id: str) -> None:
    def update(client: Redis) -> None:
        pipeline = client.pipeline(transaction=False)
        pipeline.incr(FAILED_TOTAL_KEY)
        pipeline.hdel(ACTIVE_TASKS_KEY, task_id)
        pipeline.execute()

    _safe_update("failure", update)


def snapshot(client: Any | None = None) -> CeleryMetricsSnapshot:
    owns_client = client is None
    client = client or _redis()
    try:
        pipeline = client.pipeline(transaction=False)
        pipeline.llen(QUEUE_NAME)
        pipeline.hlen(ACTIVE_TASKS_KEY)
        pipeline.mget(STARTED_TOTAL_KEY, SUCCEEDED_TOTAL_KEY, FAILED_TOTAL_KEY)
        queue_depth, active_tasks, totals = pipeline.execute()
        started, succeeded, failed = totals
        return CeleryMetricsSnapshot(
            queue_depth=int(queue_depth),
            active_tasks=int(active_tasks),
            tasks_started_total=int(started or 0),
            tasks_succeeded_total=int(succeeded or 0),
            worker_task_failures_total=int(failed or 0),
        )
    finally:
        if owns_client:
            client.close()


def render_prometheus(metrics: CeleryMetricsSnapshot) -> str:
    gauges = (
        ("bibi_celery_queue_depth", metrics.queue_depth),
        ("bibi_celery_tasks_active", metrics.active_tasks),
    )
    counters = (
        ("bibi_celery_tasks_started_total", metrics.tasks_started_total),
        ("bibi_celery_tasks_succeeded_total", metrics.tasks_succeeded_total),
        (
            "bibi_celery_worker_task_failures_total",
            metrics.worker_task_failures_total,
        ),
    )
    lines: list[str] = []
    for name, value in gauges:
        lines.extend((f"# TYPE {name} gauge", f"{name} {value}"))
    for name, value in counters:
        lines.extend((f"# TYPE {name} counter", f"{name} {value}"))
    return "\n".join(lines) + "\n"
