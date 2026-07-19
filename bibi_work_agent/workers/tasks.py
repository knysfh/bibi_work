from __future__ import annotations

from typing import Any

import httpx
from opentelemetry.trace import SpanKind

from bibi_work_agent.runtime.resume_executor import resume_run_payload
from bibi_work_agent.runtime.run_executor import execute_run_payload
from bibi_work_agent.runtime.cancellation import clear_active_task, register_active_task
from bibi_work_agent.workers.celery_app import celery_app
from bibi_work_agent.workers.metrics import (
    record_task_failed,
    record_task_started,
    record_task_succeeded,
)
from bibi_work_agent.telemetry import extract_context, tracer


@celery_app.task(
    name="bibi_work_agent.execute_run",
    bind=True,
    autoretry_for=(httpx.HTTPError,),
    retry_backoff=True,
    max_retries=3,
)
def execute_run(self: Any, payload: dict[str, Any]) -> None:
    task_id = str(self.request.id)
    run_id = str(
        payload.get("run_id")
        or (payload.get("run_config_snapshot") or {}).get("run_id")
        or ""
    )
    parent_context = extract_context(self.request.headers or {})
    with tracer.start_as_current_span(
        "celery.execute_run",
        context=parent_context,
        kind=SpanKind.CONSUMER,
        attributes={"messaging.system": "celery", "bibi.run_id": run_id},
    ):
        record_task_started(task_id)
        registered = register_active_task(run_id, task_id)
        try:
            execute_run_payload(task_id, payload)
        except Exception:
            record_task_failed(task_id)
            raise
        else:
            record_task_succeeded(task_id)
        finally:
            if registered:
                clear_active_task(run_id, task_id)


@celery_app.task(
    name="bibi_work_agent.resume_run",
    bind=True,
    autoretry_for=(httpx.HTTPError,),
    retry_backoff=True,
    max_retries=3,
)
def resume_agent_run(self: Any, run_id: str, payload: dict[str, Any]) -> None:
    task_id = str(self.request.id)
    parent_context = extract_context(self.request.headers or {})
    with tracer.start_as_current_span(
        "celery.resume_run",
        context=parent_context,
        kind=SpanKind.CONSUMER,
        attributes={"messaging.system": "celery", "bibi.run_id": run_id},
    ):
        record_task_started(task_id)
        registered = register_active_task(run_id, task_id)
        try:
            resume_run_payload(task_id, run_id, payload)
        except Exception:
            record_task_failed(task_id)
            raise
        else:
            record_task_succeeded(task_id)
        finally:
            if registered:
                clear_active_task(run_id, task_id)
